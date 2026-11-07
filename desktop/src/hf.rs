//! Поиск моделей GGUF на Hugging Face и реестр скачанных пользователем моделей.
use crate::catalog::Model;
use serde_json::{json, Value};
use std::time::Duration;

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

pub fn valid_repo(r: &str) -> bool {
    let mut it = r.split('/');
    let (a, b, c) = (it.next(), it.next(), it.next());
    let ok = |s: &str| !s.is_empty() && !s.starts_with('.') && s.len() < 100 && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    matches!((a, b, c), (Some(a), Some(b), None) if ok(a) && ok(b))
}

fn get_json(url: &str) -> Result<Value, String> {
    let mut r = ureq::get(url).set("User-Agent", "Nexora").timeout(Duration::from_secs(20));
    if let Ok(t) = std::env::var("HF_TOKEN") {
        r = r.set("Authorization", &format!("Bearer {t}"));
    }
    r.call()
        .map_err(|e| match e {
            ureq::Error::Status(c, _) => format!("Hugging Face ответил {c}"),
            o => format!("нет связи с Hugging Face: {o}"),
        })?
        .into_json()
        .map_err(|e| e.to_string())
}

/// Поиск репозиториев с GGUF, по скачиваниям.
pub fn search(q: &str) -> Result<Vec<Value>, String> {
    let q = q.trim();
    if q.is_empty() {
        return Err("Введите, что искать".into());
    }
    let url = format!("{}/api/models?search={}&filter=gguf&sort=downloads&direction=-1&limit=24", crate::download::endpoint(), enc(q));
    let v = get_json(&url)?;
    Ok(v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    Some(json!({"id": m["id"].as_str()?, "downloads": m["downloads"].as_u64().unwrap_or(0), "likes": m["likes"].as_u64().unwrap_or(0)}))
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Файлы GGUF репозитория: модели (с размером) и проекторы mmproj. Файлы из нескольких частей пропускаются.
pub fn files(repo: &str) -> Result<Value, String> {
    if !valid_repo(repo) {
        return Err("Некорректное имя репозитория".into());
    }
    let v = get_json(&format!("{}/api/models/{}/tree/main", crate::download::endpoint(), repo))?;
    let mut models = vec![];
    let mut mm = vec![];
    for f in v.as_array().cloned().unwrap_or_default() {
        let name = f["path"].as_str().unwrap_or("").to_string();
        let size = f["size"].as_u64().unwrap_or(0);
        if !name.to_lowercase().ends_with(".gguf") || name.contains('/') {
            continue;
        }
        if name.to_lowercase().contains("mmproj") {
            mm.push(json!({"name": name, "size_gb": size as f64 / 1e9}));
        } else if !name.contains("-of-0") {
            models.push(json!({"name": name, "size_gb": size as f64 / 1e9}));
        }
    }
    Ok(json!({"files": models, "mmproj": mm}))
}

fn reg_path() -> std::path::PathBuf {
    crate::engine::data_dir().join("custom_models.json")
}

pub fn custom_models() -> Vec<Model> {
    std::fs::read_to_string(reg_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_custom(v: &[Model]) -> Result<(), String> {
    std::fs::write(reg_path(), serde_json::to_string_pretty(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

fn slug(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect::<String>().trim_matches('-').to_string()
}

/// Добавляет модель в реестр (параметры неизвестны: считается плотной, оценка осторожная).
pub fn add(repo: &str, file: &str, mmproj: Option<String>, size_gb: f64) -> Result<String, String> {
    if !valid_repo(repo) {
        return Err("Некорректное имя репозитория".into());
    }
    let ok_name = |f: &str| f.to_lowercase().ends_with(".gguf") && !f.contains('/') && !f.contains('\\') && !f.contains("..");
    if !ok_name(file) || mmproj.as_deref().map(|m| !ok_name(m)).unwrap_or(false) {
        return Err("Допустимы только файлы .gguf из корня репозитория".into());
    }
    if !(0.05..400.0).contains(&size_gb) {
        return Err("Неверный размер файла".into());
    }
    let stem = file.trim_end_matches(".gguf").trim_end_matches(".GGUF");
    let id = format!("hf-{}-{}", slug(repo), slug(stem));
    let mut all = custom_models();
    all.retain(|m| m.id != id);
    all.push(Model {
        id: id.clone(),
        name: stem.to_string(),
        repo: repo.to_string(),
        file: file.to_string(),
        size_gb,
        params_b: (size_gb * 1.8 * 10.0).round() / 10.0,
        active_b: (size_gb * 1.8 * 10.0).round() / 10.0,
        moe: false,
        layers: 32,
        kv_mb_per_1k: 64.0,
        quality: 3.0,
        notes: format!("С Hugging Face: {repo}. Параметры неизвестны, оценка осторожная (как у плотной модели)."),
        tags: if mmproj.is_some() { vec!["chat".into(), "vision".into()] } else { vec!["chat".into()] },
        family: String::new(),
        mmproj: mmproj.unwrap_or_default(),
        custom: true,
    });
    save_custom(&all)?;
    Ok(id)
}

pub fn remove(id: &str) -> Result<(), String> {
    let mut all = custom_models();
    all.retain(|m| m.id != id);
    save_custom(&all)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repo_names() {
        assert!(valid_repo("unsloth/gemma-3-4b-it-GGUF"));
        assert!(!valid_repo("a/b/c"));
        assert!(!valid_repo("../x"));
        assert!(!valid_repo("a b/c"));
        assert!(!valid_repo("onlyone"));
    }
    #[test]
    fn url_encoding() {
        assert_eq!(enc("qwen moe"), "qwen%20moe");
        assert_eq!(enc("a-b_c.d"), "a-b_c.d");
    }
    #[test]
    fn slugs() {
        assert_eq!(slug("Qwen/My Model_Q4"), "qwen-my-model-q4");
    }
}
