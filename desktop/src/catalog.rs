//! Каталог моделей и оценка под железо: влезет ли, в каком режиме пойдёт и сколько токенов в секунду.
//! Числа ориентировочные (оценка, а не замер). Порт из Python-прототипа localagent с теми же тестами.
use crate::hw::Hardware;
use serde::{Deserialize, Serialize};

const DISK_RANDOM_GBS: f64 = 1.5; // осторожная скорость случайного чтения NVMe при подкачке через mmap
const GPU_BW_GBS: f64 = 250.0; // предполагаемая пропускная способность видеопамяти (карта среднего класса)
const GPU_MAX_TPS: f64 = 80.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub file: String,
    pub size_gb: f64,
    pub params_b: f64,
    pub active_b: f64,
    pub moe: bool,
    pub layers: u32,
    pub kv_mb_per_1k: f64,
    pub quality: f64,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub tags: Vec<String>, // chat | code | vision | audio
    #[serde(default)]
    pub family: String,
    /// файл проектора для моделей с «зрением» (в том же репозитории); пусто, если картинки не поддерживаются
    #[serde(default)]
    pub mmproj: String,
    /// добавлена пользователем с Hugging Face
    #[serde(default)]
    pub custom: bool,
}

impl Model {
    /// Сколько весов читается на каждый сгенерированный токен.
    pub fn active_gb(&self) -> f64 {
        if self.moe { self.size_gb * self.active_b / self.params_b } else { self.size_gb }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Estimate {
    pub fits: bool,
    pub mode: String, // cpu | cpu+disk | gpu | gpu+cpu-experts | gpu+cpu-layers | metal | none
    pub tok_s: f64,
    pub mem_gb: f64,
    pub ngl: i32,
    pub n_cpu_moe: u32,
    pub note: String,
}

pub fn load() -> Vec<Model> {
    let mut v: Vec<Model> = serde_json::from_str(include_str!("models.json")).expect("models.json");
    v.extend(crate::hf::custom_models());
    v
}

pub fn default_ctx(hw: &Hardware) -> u32 {
    if hw.ram_gb >= 24.0 {
        32768
    } else if hw.ram_gb >= 12.0 {
        16384
    } else {
        8192
    }
}

fn est(fits: bool, mode: &str, tok_s: f64, mem_gb: f64, ngl: i32, n_cpu_moe: u32, note: &str) -> Estimate {
    Estimate { fits, mode: mode.into(), tok_s, mem_gb, ngl, n_cpu_moe, note: note.into() }
}

pub fn estimate(m: &Model, hw: &Hardware, ctx: u32) -> Estimate {
    // кэш в каталоге посчитан для q8_0; движок берёт f16 (вдвое больше), когда памяти хватает (engine::kv_type)
    let kv = m.kv_mb_per_1k * (ctx as f64 / 1024.0) / 1024.0 * crate::engine::kv_factor(m.size_gb, ctx, hw.ram_gb, Some(m.kv_mb_per_1k));
    let need = m.size_gb + kv + 0.5;
    let ram_budget = (hw.ram_gb - 4.0).min(hw.ram_gb * 0.85).max(0.0);
    let cpu_bw = hw.mem_bw_gbs;
    let active = m.active_gb();

    if let Some(gpu) = hw.gpu() {
        if gpu.unified {
            let budget = hw.ram_gb * 0.7;
            if need <= budget {
                return est(true, "metal", GPU_MAX_TPS.min(cpu_bw / active), need, 99, 0, "");
            }
            return est(false, "none", 0.0, need, 0, 0, "не помещается в общую память");
        }
        let vram = (gpu.vram_gb * 0.92 - 0.5).max(0.0);
        if need <= vram {
            return est(true, "gpu", GPU_MAX_TPS.min(GPU_BW_GBS / active), need, 99, 0, "");
        }
        let base = m.size_gb * 0.12 + kv; // внимание, общие веса и KV-кэш всегда на видеокарте
        let experts = m.size_gb * 0.88;
        if m.moe && m.layers > 0 && base <= vram {
            let g = ((vram - base) / experts).min(1.0);
            if (1.0 - g) * experts <= ram_budget {
                let n_cpu = (((1.0 - g) * m.layers as f64).ceil() as u32).max(1);
                let t = (1.0 - g) * active / cpu_bw + g * active / GPU_BW_GBS;
                return est(true, "gpu+cpu-experts", GPU_MAX_TPS.min(1.0 / t), need, 99, n_cpu, "");
            }
        }
        if !m.moe && m.layers > 0 {
            let f = (vram / need).clamp(0.0, 1.0);
            if f > 0.15 && need * (1.0 - f) <= ram_budget {
                let t = f * m.size_gb / GPU_BW_GBS + (1.0 - f) * m.size_gb / cpu_bw;
                return est(true, "gpu+cpu-layers", 1.0 / t, need, ((m.layers as f64 * f) as i32).max(1), 0, "");
            }
        }
    }

    if need <= ram_budget {
        return est(true, "cpu", cpu_bw / active, need, 0, 0, "");
    }
    if need <= ram_budget * 2.5 && m.moe {
        let hit = ram_budget / need;
        let t = hit * active / cpu_bw + (1.0 - hit) * active / DISK_RANDOM_GBS;
        return est(true, "cpu+disk", 1.0 / t, need, 0, 0, "не помещается в ОЗУ: эксперты подгружаются с SSD");
    }
    est(false, "none", 0.0, need, 0, 0, "не помещается")
}

pub fn estimate_all(hw: &Hardware, ctx: u32) -> Vec<(Model, Estimate)> {
    load().into_iter().map(|m| { let e = estimate(&m, hw, ctx); (m, e) }).collect()
}

/// Лучшее качество среди подходящих и достаточно быстрых; иначе самая быстрая из подходящих.
pub fn pick(ests: &[(Model, Estimate)], min_speed: f64) -> Option<&(Model, Estimate)> {
    let fit: Vec<&(Model, Estimate)> = ests.iter().filter(|(_, e)| e.fits).collect();
    if fit.is_empty() {
        return None;
    }
    let ok: Vec<&(Model, Estimate)> = fit.iter().copied().filter(|(_, e)| e.tok_s >= min_speed).collect();
    if !ok.is_empty() {
        return ok.into_iter().max_by(|a, b| {
            (a.0.quality, a.1.tok_s).partial_cmp(&(b.0.quality, b.1.tok_s)).unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    fit.into_iter().max_by(|a, b| a.1.tok_s.partial_cmp(&b.1.tok_s).unwrap_or(std::cmp::Ordering::Equal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::{Gpu, Hardware};

    fn hw(ram: f64, bw: f64, gpus: Vec<Gpu>, system: &str, arch: &str) -> Hardware {
        Hardware { system: system.into(), arch: arch.into(), ram_gb: ram, cores: 8, threads: 16, mem_bw_gbs: bw, gpus, cpu_name: String::new() }
    }
    fn cpu(ram: f64, bw: f64) -> Hardware {
        hw(ram, bw, vec![], "Linux", "x86_64")
    }
    fn gpu(vram: f64) -> Vec<Gpu> {
        vec![Gpu { name: "RTX".into(), vram_gb: vram, backend: "cuda".into(), unified: false }]
    }
    fn choose(h: &Hardware, min_speed: f64) -> Option<(Model, Estimate)> {
        let all = estimate_all(h, default_ctx(h));
        pick(&all, min_speed).cloned()
    }

    #[test]
    fn catalog_is_valid() {
        let mut ids = std::collections::HashSet::new();
        for m in load() {
            assert!(ids.insert(m.id.clone()));
            assert!(m.file.ends_with(".gguf"));
            assert!(m.active_b <= m.params_b);
            assert!(m.size_gb > 0.0);
        }
    }

    #[test]
    fn tiny_machine_gets_nothing() {
        assert!(choose(&cpu(4.0, 10.0), 6.0).is_none());
    }

    #[test]
    fn small_cpu_gets_small_model() {
        let (m, e) = choose(&cpu(8.0, 15.0), 6.0).unwrap();
        assert_eq!(m.id, "qwen3-4b-2507");
        assert_eq!(e.mode, "cpu");
    }

    #[test]
    fn cpu_16gb_prefers_moe() {
        assert_eq!(choose(&cpu(16.0, 25.0), 6.0).unwrap().0.id, "gpt-oss-20b");
    }

    #[test]
    fn cpu_32gb_gets_qwen3_coder() {
        let (m, e) = choose(&cpu(32.0, 27.0), 6.0).unwrap();
        assert_eq!(m.id, "qwen3-coder-30b-a3b");
        assert_eq!(e.ngl, 0);
    }

    #[test]
    fn big_gpu_runs_120b_with_cpu_experts() {
        let (m, e) = choose(&hw(64.0, 40.0, gpu(24.0), "Linux", "x86_64"), 6.0).unwrap();
        assert_eq!(m.id, "gpt-oss-120b");
        assert_eq!(e.mode, "gpu+cpu-experts");
        assert!(e.n_cpu_moe > 0);
    }

    #[test]
    fn gpu_that_fits_uses_full_offload() {
        let (m, e) = choose(&hw(16.0, 25.0, gpu(24.0), "Linux", "x86_64"), 6.0).unwrap();
        assert_eq!(m.id, "qwen3-coder-30b-a3b");
        assert_eq!(e.mode, "gpu");
        assert_eq!((e.ngl, e.n_cpu_moe), (99, 0));
    }

    #[test]
    fn small_gpu_keeps_part_of_experts_on_cpu() {
        let (m, e) = choose(&hw(16.0, 25.0, gpu(12.0), "Linux", "x86_64"), 6.0).unwrap();
        assert_eq!(m.id, "qwen3-coder-30b-a3b");
        assert_eq!(e.mode, "gpu+cpu-experts");
        assert!(e.n_cpu_moe > 0 && e.n_cpu_moe < 48);
    }

    #[test]
    fn cpu_experts_must_fit_in_ram() {
        let h = hw(16.0, 25.0, gpu(12.0), "Linux", "x86_64");
        let all = estimate_all(&h, 16384);
        let e = all.iter().find(|(m, _)| m.id == "gpt-oss-120b").unwrap();
        assert_ne!(e.1.mode, "gpu+cpu-experts");
    }

    #[test]
    fn apple_silicon_uses_metal() {
        let g = vec![Gpu { name: "Apple".into(), vram_gb: 11.2, backend: "metal".into(), unified: true }];
        assert_eq!(choose(&hw(16.0, 100.0, g, "Darwin", "arm64"), 6.0).unwrap().1.mode, "metal");
    }

    #[test]
    fn nothing_fast_enough_falls_back_to_fastest() {
        assert_eq!(choose(&cpu(8.0, 5.0), 50.0).unwrap().0.id, "qwen2.5-coder-1.5b");
    }
}
