//! Модель знает, где работает, и может править исходники самой Nexora, когда открыта их папка.
//! Защита на уровне инструментов (не подсказки): модули безопасности и сами инструменты менять нельзя,
//! перед любой правкой файла в исходниках делается копия, запущенную программу правка не затрагивает.
use std::path::{Path, PathBuf};

/// Файлы, которые модель менять не может: аутентификация, сертификаты, подтверждения, проверка обновлений,
/// сервер/телефон и сами инструменты с проверкой путей. Править их можно только вручную.
const PROTECTED: &[&str] = &["secure.rs", "pki.rs", "totp.rs", "update.rs", "hostmode.rs", "server.rs", "mobile.rs", "tools.rs", "agent.rs", "selfdev.rs"];

/// Одна короткая строка для любого проекта: каждая модель знает, внутри чего она работает (≈30 токенов).
pub const WHERE_AM_I: &str = "Ты работаешь внутри программы Nexora (локальный агент на ПК пользователя); её исходники правишь только если открыта их папка.";

/// Корень исходников (папка с Cargo.toml пакета nexora) для пути внутри проекта.
pub fn source_root(p: &Path) -> Option<PathBuf> {
    for a in p.ancestors() {
        if let Ok(t) = std::fs::read_to_string(a.join("Cargo.toml")) {
            if t.lines().any(|l| l.trim().replace(' ', "") == "name=\"nexora\"") && a.join("src").join("agent.rs").is_file() {
                return Some(a.to_path_buf());
            }
        }
    }
    // открыта папка выше (D:\agents\localcoder): исходники лежат в app
    let app = p.join("app");
    if app.join("src").join("agent.rs").is_file() && app.join("Cargo.toml").is_file() {
        return Some(app);
    }
    None
}

/// Почему менять файл нельзя (None = можно).
pub fn protected(path: &Path) -> Option<String> {
    let root = source_root(path.parent()?)?;
    // Windows отбрасывает хвостовые точки и пробелы и понимает «:» (потоки), поэтому нормализуем имя до сравнения
    let raw = path.file_name()?.to_str()?.to_lowercase();
    let name = raw.split(':').next().unwrap_or("").trim_end_matches(['.', ' ']).to_string();
    let dir = path.parent()?.canonicalize().ok()?;
    let in_src = root.join("src").canonicalize().map(|s| s == dir).unwrap_or(false);
    if in_src && PROTECTED.contains(&name.as_str()) {
        return Some(format!("{name} защищён: безопасность и инструменты Nexora меняются только вручную"));
    }
    None
}

/// Копия файла перед правкой: `<данные>/selfdev-backups/<время>/<имя>`. Ошибка копии не должна ломать правку,
/// но результат возвращается, чтобы её можно было показать.
pub fn backup(path: &Path) -> Option<PathBuf> {
    source_root(path.parent()?)?;
    if !path.is_file() {
        return None;
    }
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let dir = crate::engine::data_dir().join("selfdev-backups").join(ts.to_string());
    std::fs::create_dir_all(&dir).ok()?;
    let dest = dir.join(path.file_name()?);
    std::fs::copy(path, &dest).ok()?;
    Some(dest)
}

/// Короткий блок для подсказки, если открыта папка исходников Nexora (иначе None). Без длинных описаний:
/// каждая 1000 токенов вводной части = больше минуты ожидания на слабом ПК.
pub fn prompt_block(project: &Path) -> Option<String> {
    let root = source_root(project)?;
    Some(format!(
        "\n\nЭТО ИСХОДНИКИ NEXORA, программы, в которой ты работаешь (Tauri 2 + Rust, код {src}, интерфейс app/dist). Подробности: HANDOFF.md. Где что: загрузка моделей download.rs, каталог catalog.rs и models.json, движок engine.rs, подбор инструментов router.rs, настройки prefs.rs. \
         Правь только то, о чём просят. Запущенную программу не перезаписывай: собирай в отдельную папку (CARGO_TARGET_DIR=target-selfdev cargo build --release), проверяй cargo test --release. \
         Защищены от правки: {prot}. Перед правкой файла автоматически делается копия.",
        src = root.join("src").display(),
        prot = PROTECTED.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_source(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nexora-selfdev-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("app").join("src")).unwrap();
        std::fs::write(d.join("app").join("Cargo.toml"), "[package]\nname = \"nexora\"\nversion = \"0.1.0\"\n").unwrap();
        std::fs::write(d.join("app").join("src").join("agent.rs"), "// x").unwrap();
        std::fs::write(d.join("app").join("src").join("download.rs"), "// d").unwrap();
        std::fs::write(d.join("app").join("src").join("secure.rs"), "// s").unwrap();
        d
    }

    #[test]
    fn detects_source_from_parent_and_app_folder() {
        let d = fake_source("detect");
        assert!(source_root(&d).is_some());
        assert!(source_root(&d.join("app")).is_some());
        assert!(source_root(&d.join("app").join("src")).is_some());
        let other = std::env::temp_dir().join(format!("nexora-selfdev-{}-other", std::process::id()));
        std::fs::create_dir_all(&other).unwrap();
        assert!(source_root(&other).is_none());
        assert!(prompt_block(&other).is_none());
        assert!(prompt_block(&d).unwrap().contains("ИСХОДНИКИ NEXORA"));
    }

    #[test]
    fn security_modules_are_protected_but_download_is_not() {
        let d = fake_source("prot");
        let src = d.join("app").join("src");
        assert!(protected(&src.join("secure.rs")).is_some());
        assert!(protected(&src.join("agent.rs")).is_some());
        assert!(protected(&src.join("download.rs")).is_none());
        // одноимённый файл вне src (например, в чужом проекте) не затрагивается
        let other = std::env::temp_dir().join(format!("nexora-selfdev-{}-other2", std::process::id()));
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("secure.rs"), "x").unwrap();
        assert!(protected(&other.join("secure.rs")).is_none());
    }

    #[test]
    fn backup_copies_only_inside_source() {
        let d = fake_source("bak");
        let f = d.join("app").join("src").join("download.rs");
        let b = backup(&f).expect("копия должна создаться");
        assert_eq!(std::fs::read_to_string(b).unwrap(), "// d");
        let other = std::env::temp_dir().join(format!("nexora-selfdev-{}-other3", std::process::id()));
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("a.txt"), "x").unwrap();
        assert!(backup(&other.join("a.txt")).is_none());
    }

    #[test]
    fn windows_name_tricks_do_not_bypass() {
        let d = fake_source("tricks");
        let src = d.join("app").join("src");
        for n in ["SECURE.RS", "secure.rs.", "secure.rs ", "secure.rs::$DATA"] {
            assert!(protected(&src.join(n)).is_some(), "{n}");
        }
    }

    #[test]
    fn prompt_stays_short() {
        let d = fake_source("len");
        let n = prompt_block(&d).unwrap().chars().count();
        assert!(n < 1100, "блок для подсказки вырос до {n} символов");
    }
}
