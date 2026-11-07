//! Определение железа: ОЗУ, ядра, видеокарты и грубая пропускная способность памяти.
//! Любая неудача даёт безопасное значение по умолчанию, а не падение.
use serde::Serialize;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Instant;

#[derive(Clone, Debug, Serialize)]
pub struct Gpu {
    pub name: String,
    pub vram_gb: f64,
    pub backend: String, // cuda | metal | vulkan
    pub unified: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Hardware {
    pub system: String,
    pub arch: String,
    pub ram_gb: f64,
    pub cores: usize,
    pub threads: usize,
    pub mem_bw_gbs: f64,
    pub gpus: Vec<Gpu>,
    pub cpu_name: String,
}

impl Hardware {
    pub fn gpu(&self) -> Option<&Gpu> {
        self.gpus.iter().max_by(|a, b| a.vram_gb.partial_cmp(&b.vram_gb).unwrap_or(std::cmp::Ordering::Equal))
    }
}

fn run(cmd: &str, args: &[&str]) -> String {
    let mut c = Command::new(cmd);
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    match c.output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => String::new(),
    }
}

fn ps(script: &str) -> String {
    run("powershell", &["-NoProfile", "-NonInteractive", "-Command", &format!("[Console]::OutputEncoding=[Text.Encoding]::UTF8; {script}")])
}

#[cfg(windows)]
#[repr(C)]
struct MemStatus {
    length: u32,
    load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page: u64,
    avail_page: u64,
    total_virt: u64,
    avail_virt: u64,
    avail_ext: u64,
}

#[cfg(windows)]
extern "system" {
    fn GlobalMemoryStatusEx(buf: *mut MemStatus) -> i32;
}

fn ram_gb() -> f64 {
    #[cfg(windows)]
    {
        let mut m = MemStatus { length: std::mem::size_of::<MemStatus>() as u32, load: 0, total_phys: 0, avail_phys: 0, total_page: 0, avail_page: 0, total_virt: 0, avail_virt: 0, avail_ext: 0 };
        if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 && m.total_phys > 0 {
            return m.total_phys as f64 / (1u64 << 30) as f64;
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(s) = std::fs::read_to_string("/proc/meminfo") {
            for l in s.lines() {
                if let Some(r) = l.strip_prefix("MemTotal:") {
                    if let Some(kb) = r.split_whitespace().next().and_then(|x| x.parse::<f64>().ok()) {
                        return kb / (1u64 << 20) as f64;
                    }
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(v) = run("sysctl", &["-n", "hw.memsize"]).trim().parse::<f64>() {
            return v / (1u64 << 30) as f64;
        }
    }
    8.0
}

fn cpu_name() -> String {
    #[cfg(windows)]
    {
        return ps("(Get-CimInstance Win32_Processor | Select-Object -First 1).Name").trim().to_string();
    }
    #[allow(unreachable_code)]
    String::new()
}

/// Занято сейчас, ГБ (только Windows; иначе 0).
pub fn ram_used_gb() -> f64 {
    #[cfg(windows)]
    {
        let mut m = MemStatus { length: std::mem::size_of::<MemStatus>() as u32, load: 0, total_phys: 0, avail_phys: 0, total_page: 0, avail_page: 0, total_virt: 0, avail_virt: 0, avail_ext: 0 };
        if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 {
            return (m.total_phys - m.avail_phys) as f64 / (1u64 << 30) as f64;
        }
    }
    0.0
}

fn physical_cores(threads: usize) -> usize {
    #[cfg(windows)]
    {
        let o = ps("(Get-CimInstance Win32_Processor | Measure-Object NumberOfCores -Sum).Sum");
        if let Ok(n) = o.trim().parse::<usize>() {
            if n > 0 {
                return n;
            }
        }
    }
    if threads >= 8 { threads / 2 } else { threads }
}

fn gpus(system: &str, arch: &str, ram: f64) -> Vec<Gpu> {
    let mut out: Vec<Gpu> = vec![];
    let smi = run("nvidia-smi", &["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"]);
    for line in smi.lines() {
        if let Some((name, mem)) = line.rsplit_once(',') {
            if let Ok(mb) = mem.trim().parse::<f64>() {
                out.push(Gpu { name: name.trim().into(), vram_gb: mb / 1024.0, backend: "cuda".into(), unified: false });
            }
        }
    }
    if system == "macos" && arch == "aarch64" {
        out.push(Gpu { name: "Apple GPU (unified memory)".into(), vram_gb: ram * 0.7, backend: "metal".into(), unified: true });
    }
    #[cfg(windows)]
    {
        let key = r"HKLM:\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}\0*";
        let o = ps(&format!(
            "Get-ItemProperty '{key}' -ErrorAction SilentlyContinue | Select-Object DriverDesc,'HardwareInformation.qwMemorySize' | ConvertTo-Json -Compress"
        ));
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(o.trim()) {
            let items: Vec<serde_json::Value> = if v.is_array() { v.as_array().cloned().unwrap_or_default() } else { vec![v] };
            for d in items {
                let name = d["DriverDesc"].as_str().unwrap_or("").to_string();
                if let Some(mem) = d["HardwareInformation.qwMemorySize"].as_f64() {
                    if !name.to_lowercase().contains("nvidia") {
                        out.push(Gpu { name, vram_gb: mem / (1u64 << 30) as f64, backend: "vulkan".into(), unified: false });
                    }
                }
            }
        }
    }
    // «видеопамять» встроенной графики (до 3 ГБ) для выгрузки модели бесполезна
    out.into_iter().filter(|g| g.vram_gb >= 3.0 || g.unified).collect()
}

/// Быстрый тест копирования памяти, пересчитанный в то, что реально читает llama.cpp (оценка).
pub fn measure_bandwidth() -> f64 {
    let n = 128usize << 20;
    let a = vec![1u8; n];
    let mut b = vec![0u8; n];
    let mut best = 0.0f64;
    for _ in 0..5 {
        let t = Instant::now();
        b.copy_from_slice(&a);
        std::hint::black_box(&b);
        let gbs = 2.0 * n as f64 / t.elapsed().as_secs_f64() / 1e9;
        if gbs > best {
            best = gbs;
        }
    }
    (best * 0.85).clamp(5.0, 400.0)
}

pub fn detect() -> Hardware {
    let system = match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "Darwin",
        other => other,
    };
    let sys_key = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let ram = ram_gb();
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    Hardware {
        system: system.into(),
        arch: arch.into(),
        ram_gb: ram,
        cores: physical_cores(threads),
        threads,
        mem_bw_gbs: measure_bandwidth(),
        gpus: gpus(sys_key, arch, ram),
        cpu_name: cpu_name(),
    }
}

static HW: OnceLock<Hardware> = OnceLock::new();

/// Железо определяется один раз за запуск (около 2-3 с).
pub fn get() -> &'static Hardware {
    HW.get_or_init(detect)
}
