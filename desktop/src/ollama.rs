//! Поиск уже скачанной модели в хранилище Ollama (чтобы не качать заново).
use std::path::PathBuf;

fn base() -> PathBuf {
    if let Ok(p) = std::env::var("OLLAMA_MODELS") {
        return PathBuf::from(p);
    }
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    PathBuf::from(home).join(".ollama").join("models")
}

/// `name` вида "qwen3-coder" или "qwen3-coder:latest".
pub fn find_gguf(name: &str) -> Result<PathBuf, String> {
    let (n, tag) = match name.split_once(':') {
        Some((a, b)) => (a, b),
        None => (name, "latest"),
    };
    let base = base();
    let manifests = [
        base.join("manifests-v2").join("ollama.com").join("library").join(n).join(tag),
        base.join("manifests").join("registry.ollama.ai").join("library").join(n).join(tag),
    ];
    let manifest = manifests
        .iter()
        .find(|p| p.exists())
        .ok_or_else(|| format!("В Ollama нет модели {n}:{tag}"))?;
    let text = std::fs::read_to_string(manifest).map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    for layer in v["layers"].as_array().ok_or("в манифесте нет слоёв")? {
        if layer["mediaType"].as_str().unwrap_or("").ends_with("image.model") {
            let digest = layer["digest"].as_str().ok_or("нет digest")?.replace(':', "-");
            let blob = base.join("blobs").join(digest);
            if blob.exists() {
                return Ok(blob);
            }
            return Err(format!("Файл весов не найден: {}", blob.display()));
        }
    }
    Err("В манифесте нет слоя с весами".into())
}

/// Имена моделей, скачанных в Ollama ("имя" или "имя:тег").
pub fn list_models() -> Vec<String> {
    let base = base();
    let mut out: Vec<String> = vec![];
    for root in [
        base.join("manifests-v2").join("ollama.com").join("library"),
        base.join("manifests").join("registry.ollama.ai").join("library"),
    ] {
        let Ok(models) = std::fs::read_dir(&root) else { continue };
        for m in models.flatten() {
            let name = m.file_name().to_string_lossy().to_string();
            let Ok(tags) = std::fs::read_dir(m.path()) else { continue };
            for t in tags.flatten() {
                let tag = t.file_name().to_string_lossy().to_string();
                let full = if tag == "latest" { name.clone() } else { format!("{name}:{tag}") };
                if !out.contains(&full) && find_gguf(&full).is_ok() {
                    out.push(full);
                }
            }
        }
    }
    out.sort();
    out
}
