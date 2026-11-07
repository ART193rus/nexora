//! Переключатели в поле ввода: «Рассуждение» (модель думает перед ответом) и «Веб-поиск» (поиск и чтение страниц).
//! Общие для окна, телефона и задач по расписанию; хранятся в prefs.json.
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};

static THINK: AtomicBool = AtomicBool::new(false);
static WEB: AtomicBool = AtomicBool::new(false);
static TRAY: AtomicBool = AtomicBool::new(true);
static LOADED: OnceLock<()> = OnceLock::new();
static UPDATE_URL: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new(String::new()));

fn path() -> std::path::PathBuf {
    crate::engine::data_dir().join("prefs.json")
}

fn ensure() {
    LOADED.get_or_init(|| {
        if let Some(v) = std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
            THINK.store(v["think"].as_bool().unwrap_or(false), Ordering::SeqCst);
            WEB.store(v["web"].as_bool().unwrap_or(false), Ordering::SeqCst);
            TRAY.store(v["tray"].as_bool().unwrap_or(true), Ordering::SeqCst);
            *UPDATE_URL.lock().unwrap() = v["update_url"].as_str().unwrap_or("").to_string();
        }
    });
}

fn save() {
    let _ = std::fs::write(path(), json!({"think": THINK.load(Ordering::SeqCst), "web": WEB.load(Ordering::SeqCst), "tray": TRAY.load(Ordering::SeqCst), "update_url": UPDATE_URL.lock().unwrap().clone()}).to_string());
}

pub fn think() -> bool {
    ensure();
    THINK.load(Ordering::SeqCst)
}

pub fn web() -> bool {
    ensure();
    WEB.load(Ordering::SeqCst)
}

pub fn set_think(on: bool) {
    ensure();
    THINK.store(on, Ordering::SeqCst);
    save();
}

pub fn set_web(on: bool) {
    ensure();
    WEB.store(on, Ordering::SeqCst);
    save();
}

/// Адрес манифеста обновлений (только https); пусто = не задан.
pub fn update_url() -> String {
    ensure();
    UPDATE_URL.lock().unwrap().clone()
}

pub fn set_update_url(u: &str) {
    ensure();
    *UPDATE_URL.lock().unwrap() = u.to_string();
    save();
}

/// Крестик сворачивает окно в трей, а не закрывает программу (по умолчанию включено).
pub fn tray() -> bool {
    ensure();
    TRAY.load(Ordering::SeqCst)
}

pub fn set_tray(on: bool) {
    ensure();
    TRAY.store(on, Ordering::SeqCst);
    save();
}
