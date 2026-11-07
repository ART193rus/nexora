//! Локальный движок (llama-server). Слушает ТОЛЬКО 127.0.0.1, порт выбирается автоматически,
//! доступ закрыт токеном сессии. Токен остаётся в Rust и не передаётся интерфейсу.
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

pub struct Engine {
    /// None для внешнего движка (LM Studio, Ollama): им управляет не программа
    pub child: Option<Child>,
    pub port: u16,
    pub base: String,
    pub token: String,
    pub alias: String,
    /// движок принимает картинки
    pub vision: bool,
}

pub struct Settings {
    pub server_exe: PathBuf,
    pub model: PathBuf,
    pub alias: String,
    pub ctx: u32,
    /// файл проектора (mmproj) для моделей, понимающих картинки
    pub mmproj: Option<PathBuf>,
    /// Размер кэша контекста на 1000 токенов в МБ при сжатом q8_0 (из каталога моделей); None = модель неизвестна.
    pub kv_mb_per_1k: Option<f64>,
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".into());
    let d = PathBuf::from(base).join("Nexora");
    // перенос данных из прежней папки LocalCoder (однократно)
    let old = d.with_file_name("LocalCoder");
    if old.exists() && !d.exists() {
        let _ = std::fs::rename(&old, &d);
    }
    let _ = std::fs::create_dir_all(d.join("logs"));
    d
}

/// Где искать llama-server: переменная LC_LLAMA_SERVER, рядом с программой, затем старая папка пользователя.
pub fn find_server_exe() -> Result<PathBuf, String> {
    let mut c: Vec<PathBuf> = vec![];
    if let Ok(p) = std::env::var("LC_LLAMA_SERVER") {
        c.push(p.into());
    }
    if let Ok(me) = std::env::current_exe() {
        if let Some(d) = me.parent() {
            c.push(d.join("engine").join("llama-server.exe"));
        }
    }
    c.push(data_dir().join("engine").join("llama-server.exe"));
    c.push(PathBuf::from(r"C:\dev\local_agent\bin\cpu\llama-server.exe"));
    c.into_iter().find(|p| p.exists()).ok_or_else(|| "llama-server.exe не найден".to_string())
}

fn free_port() -> Result<u16, String> {
    let l = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    Ok(l.local_addr().map_err(|e| e.to_string())?.port())
}

fn new_token() -> Result<String, String> {
    let mut b = [0u8; 24];
    getrandom::getrandom(&mut b).map_err(|e| e.to_string())?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// «Задание» Windows с флагом «завершить всё при закрытии»: движок, запущенный внутри него, гибнет вместе с Nexora при любом
/// способе закрытия, в том числе при принудительном завершении зависшего окна (раньше он оставался висеть и занимать память).
#[cfg(windows)]
pub mod job {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;

    #[repr(C)]
    struct Basic {
        per_process_user_time: i64,
        per_job_user_time: i64,
        flags: u32,
        min_ws: usize,
        max_ws: usize,
        active_limit: u32,
        affinity: usize,
        priority: u32,
        scheduling: u32,
    }
    #[repr(C)]
    struct Io([u64; 6]);
    #[repr(C)]
    struct Ext {
        basic: Basic,
        io: Io,
        process_mem: usize,
        job_mem: usize,
        peak_process: usize,
        peak_job: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attrs: *mut c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(job: *mut c_void, class: i32, info: *const c_void, len: u32) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn CloseHandle(h: *mut c_void) -> i32;
    }

    const EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const KILL_ON_JOB_CLOSE: u32 = 0x2000;

    pub struct Job(usize);

    impl Job {
        pub fn new() -> Option<Job> {
            unsafe {
                let h = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                if h.is_null() {
                    return None;
                }
                let mut info: Ext = std::mem::zeroed();
                info.basic.flags = KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(h, EXTENDED_LIMIT_INFORMATION, &info as *const Ext as *const c_void, std::mem::size_of::<Ext>() as u32) == 0 {
                    CloseHandle(h);
                    return None;
                }
                Some(Job(h as usize))
            }
        }

        pub fn adopt(&self, child: &std::process::Child) -> bool {
            unsafe { AssignProcessToJobObject(self.0 as *mut c_void, child.as_raw_handle() as *mut c_void) != 0 }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0 as *mut c_void);
            }
        }
    }

    static GLOBAL: OnceLock<Option<Job>> = OnceLock::new();

    /// Общее задание процесса: дескриптор живёт, пока жив Nexora; когда процесс умирает, закрывается и он.
    pub fn adopt_global(child: &std::process::Child) -> bool {
        GLOBAL.get_or_init(Job::new).as_ref().map_or(false, |j| j.adopt(child))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        #[test]
        fn child_dies_when_job_closes() {
            let mut child = Command::new("ping").args(["-n", "60", "127.0.0.1"]).stdout(Stdio::null()).spawn().expect("ping");
            let job = Job::new().expect("job");
            assert!(job.adopt(&child), "процесс должен войти в задание");
            assert!(child.try_wait().unwrap().is_none(), "пока задание открыто, процесс живёт");
            drop(job);
            let end = Instant::now() + Duration::from_secs(5);
            while Instant::now() < end {
                if child.try_wait().unwrap().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = child.kill();
            panic!("процесс не завершился после закрытия задания");
        }
    }
}

/// Потоки (генерация, чтение вводной части): три четверти логических процессоров, остальные остаются системе и окну.
/// Замер на Ryzen 9 5900HX (8 ядер, 16 потоков), вводная часть на глубине 1500 токенов, кэш q8_0: 8 потоков 21,7 ток/с,
/// 12 потоков 27,2, 16 потоков 26,2 (шумнее). Скорость самого ответа после 12 потоков тоже не растёт.
pub fn threads_for(logical: usize) -> (usize, usize) {
    let l = logical.max(1);
    let t = if l >= 8 { l * 3 / 4 } else { l };
    (t, t)
}

/// Тип кэша контекста. Несжатый f16 заметно быстрее: на глубине 1500 токенов чтение вводной части 55,7 ток/с против
/// 27,2 с q8_0 (сжатый кэш на процессоре тормозит внимание). Но он вдвое больше, поэтому берётся только если памяти
/// заведомо хватает (модель + кэш f16 + запас 3 ГБ не больше 80% ОЗУ); иначе экономный q8_0.
/// `kv_mb_q8`: размер кэша на 1000 токенов в МБ при q8_0 (из каталога); без него берётся 50 МБ (средняя модель).
pub fn kv_type(model_gb: f64, ctx: u32, ram_gb: f64, kv_mb_q8: Option<f64>) -> &'static str {
    let kv_f16_gb = kv_mb_q8.unwrap_or(50.0) * 2.0 * (ctx as f64 / 1024.0) / 1024.0;
    if model_gb + kv_f16_gb + 3.0 <= ram_gb * 0.8 { "f16" } else { "q8_0" }
}

/// Во сколько раз кэш больше значения из каталога (оно считано для q8_0): 2 при f16, иначе 1.
pub fn kv_factor(model_gb: f64, ctx: u32, ram_gb: f64, kv_mb_q8: Option<f64>) -> f64 {
    if kv_type(model_gb, ctx, ram_gb, kv_mb_q8) == "f16" { 2.0 } else { 1.0 }
}

#[cfg(test)]
mod thread_tests {
    use super::{kv_factor, kv_type, threads_for};

    #[test]
    fn thread_split() {
        assert_eq!(threads_for(16), (12, 12));
        assert_eq!(threads_for(8), (6, 6));
        assert_eq!(threads_for(4), (4, 4));
        assert_eq!(threads_for(0), (1, 1));
        assert_eq!(threads_for(32), (24, 24));
    }

    #[test]
    fn kv_type_depends_on_memory() {
        assert_eq!(kv_type(17.3, 32768, 31.4, Some(48.0)), "f16", "30B на 32 ГБ: хватает");
        assert_eq!(kv_type(17.3, 32768, 24.0, Some(48.0)), "q8_0", "та же модель на 24 ГБ: экономно");
        assert_eq!(kv_type(4.0, 8192, 8.0, None), "q8_0", "малый ПК");
        assert_eq!(kv_type(4.0, 8192, 16.0, None), "f16");
        assert_eq!(kv_type(17.3, 131072, 31.4, Some(48.0)), "q8_0", "очень длинный контекст не влезет в f16");
        assert_eq!(kv_type(8.0, 32768, 32.0, Some(300.0)), "q8_0", "модель с тяжёлым кэшем (большая плотная) не берёт f16");
    }

    #[test]
    fn kv_factor_matches_type() {
        assert_eq!(kv_factor(17.3, 32768, 31.4, Some(48.0)), 2.0);
        assert_eq!(kv_factor(17.3, 32768, 24.0, Some(48.0)), 1.0);
    }
}

pub fn start(s: &Settings) -> Result<Engine, String> {
    let port = free_port()?;
    let token = new_token()?;
    let logical = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let (threads, threads_batch) = threads_for(logical);
    let (threads, threads_batch) = (threads.to_string(), threads_batch.to_string());
    let model_gb = std::fs::metadata(&s.model).map(|m| m.len() as f64 / 1e9).unwrap_or(8.0);
    let kv = kv_type(model_gb, s.ctx, crate::hw::get().ram_gb, s.kv_mb_per_1k);
    let log = std::fs::File::create(data_dir().join("logs").join("engine.log")).map_err(|e| e.to_string())?;
    let log2 = log.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(&s.server_exe);
    cmd.arg("-m").arg(&s.model)
        .args(["--alias", &s.alias])
        .args(["--host", "127.0.0.1"])
        .args(["--port", &port.to_string()])
        .args(["--api-key", &token])
        .args(["-c", &s.ctx.to_string()])
        .args(["-t", &threads, "-tb", &threads_batch])
        .args(["-fa", "on", "-ctk", kv, "-ctv", kv])
        .args(["--jinja", "--parallel", "1", "--no-warmup"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log2));
    if let Some(p) = &s.mmproj {
        cmd.arg("--mmproj").arg(p);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let child = cmd.spawn().map_err(|e| format!("не удалось запустить движок: {e}"))?;
    #[cfg(windows)]
    {
        let _ = job::adopt_global(&child); // не вышло (процесс уже в чужом задании): обычный режим, как раньше
    }
    Ok(Engine { child: Some(child), port, base: format!("http://127.0.0.1:{port}"), token, alias: s.alias.clone(), vision: s.mmproj.is_some() })
}

pub fn health(port: u16) -> bool {
    ureq::get(&format!("http://127.0.0.1:{port}/health"))
        .timeout(Duration::from_secs(2))
        .call()
        .map(|r| r.status() == 200)
        .unwrap_or(false)
}

/// Проверка «порт не виден из сети»: подключаемся к порту по адресу сетевой карты, должен быть отказ.
pub fn verify_loopback_only(port: u16) -> Result<(), String> {
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    // UDP connect не отправляет пакетов, только выбирает исходящий интерфейс
    if sock.connect("192.0.2.1:9").is_err() {
        return Ok(()); // сети нет: снаружи не достучаться
    }
    let ip = sock.local_addr().map_err(|e| e.to_string())?.ip();
    if ip.is_loopback() || ip.is_unspecified() {
        return Ok(());
    }
    match TcpStream::connect_timeout(&SocketAddr::new(ip, port), Duration::from_millis(1500)) {
        Ok(_) => Err(format!("ОПАСНО: порт {port} доступен по адресу {ip}")),
        Err(_) => Ok(()),
    }
}

pub fn stop(e: &mut Engine) {
    if let Some(c) = e.child.as_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

impl Engine {
    /// Если встроенный движок завершился, возвращает код выхода текстом.
    pub fn exited(&mut self) -> Option<String> {
        match self.child.as_mut()?.try_wait() {
            Ok(Some(st)) => Some(st.to_string()),
            _ => None,
        }
    }
}

/// Список моделей внешнего движка по OpenAI-совместимому адресу `{base}/v1/models`.
pub fn probe(base: &str, key: &str) -> Result<Vec<String>, String> {
    let mut r = ureq::get(&format!("{base}/v1/models")).timeout(Duration::from_millis(2000));
    if !key.is_empty() {
        r = r.set("Authorization", &format!("Bearer {key}"));
    }
    let v: serde_json::Value = r.call().map_err(|e| e.to_string())?.into_json().map_err(|e| e.to_string())?;
    Ok(v["data"]
        .as_array()
        .map(|a| a.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
        .unwrap_or_default())
}

/// "ollama", "lmstudio" или "openai" (любой другой совместимый сервер).
pub fn kind(base: &str) -> &'static str {
    let ok = |path: &str| ureq::get(&format!("{base}{path}")).timeout(Duration::from_millis(800)).call().is_ok();
    if ok("/api/version") {
        "ollama"
    } else if ok("/api/v0/models") {
        "lmstudio"
    } else {
        "openai"
    }
}

/// Адрес в этом компьютере или в частной сети (192.168.x, 10.x, 172.16-31.x, .local).
pub fn is_private_host(base: &str) -> bool {
    let rest = base.split("://").nth(1).unwrap_or(base);
    let host = rest.split(['/', ':']).next().unwrap_or("");
    if host == "localhost" || host.starts_with("127.") || host.ends_with(".local") || rest.starts_with("[::1]") {
        return true;
    }
    if host.starts_with("192.168.") || host.starts_with("10.") {
        return true;
    }
    if let Some(r) = host.strip_prefix("172.") {
        if let Some(n) = r.split('.').next().and_then(|x| x.parse::<u8>().ok()) {
            return (16..=31).contains(&n);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_hosts() {
        assert!(is_private_host("http://127.0.0.1:11434"));
        assert!(is_private_host("http://localhost:1234"));
        assert!(is_private_host("http://192.168.1.50:8080"));
        assert!(is_private_host("http://172.20.0.5"));
        assert!(!is_private_host("http://172.40.0.5"));
        assert!(!is_private_host("https://api.example.com"));
        assert!(!is_private_host("http://8.8.8.8:80"));
    }
}
