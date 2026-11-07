//! Оболочка рабочего стола: значок в трее, сворачивание в трей по крестику, запуск вместе с Windows.
//! Автозапуск: значение в HKCU\Software\Microsoft\Windows\CurrentVersion\Run (права администратора не нужны).
//! Запущенная при входе в Windows программа стартует свёрнутой в трей (`--background`), сама поднимает модель и,
//! если включено, сервер для устройств.
use std::path::PathBuf;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

pub const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
pub const RUN_VALUE: &str = "Nexora";

/// Показать главное окно (из трея или при повторном запуске программы).
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Где искать свой значок трея: рядом с программой и в папке данных; png или ico.
pub fn tray_icon_candidates() -> Vec<PathBuf> {
    let mut v = vec![];
    let mut dirs: Vec<PathBuf> = vec![];
    if let Some(d) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        dirs.push(d);
    }
    dirs.push(crate::engine::data_dir());
    for d in dirs {
        for n in ["tray.png", "tray.ico"] {
            v.push(d.join(n));
        }
    }
    v
}

/// Значок трея: свой файл, если положен; иначе значок программы.
pub fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Открыть Nexora", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выход (остановить модель и сервер)", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &sep, &quit])?;
    let mut b = TrayIconBuilder::with_id("main").tooltip("Nexora").menu(&menu).show_menu_on_left_click(false);
    let custom = tray_icon_candidates().into_iter().find(|p| p.is_file()).and_then(|p| tauri::image::Image::from_path(p).ok());
    if let Some(img) = custom {
        b = b.icon(img);
    } else if let Some(img) = app.default_window_icon() {
        b = b.icon(img.clone());
    }
    b.on_menu_event(|app, ev| match ev.id.as_ref() {
        "open" => show_main(app),
        "quit" => app.exit(0), // RunEvent::Exit останавливает движок, сервер и всё запущенное программой
        _ => {}
    })
    .on_tray_icon_event(|tray, ev| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
            show_main(tray.app_handle());
        }
    })
    .build(app)?;
    Ok(())
}

// ---------- автозапуск с Windows ----------
/// Команда для автозапуска: путь к этой программе в кавычках и флаг фонового запуска.
pub fn autostart_command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" --background", exe.display()))
}

/// Значение из вывода `reg query ... /v имя`.
pub fn parse_run_value(out: &str, name: &str) -> Option<String> {
    for line in out.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix(name) {
            let rest = rest.trim_start();
            if let Some(v) = rest.strip_prefix("REG_SZ") {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

#[cfg(windows)]
fn reg(args: &[&str]) -> Option<std::process::Output> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("reg").args(args).creation_flags(0x0800_0000).output().ok()
}

#[cfg(not(windows))]
fn reg(_: &[&str]) -> Option<std::process::Output> {
    None
}

/// (включён ли автозапуск, указывает ли он на эту программу).
pub fn autostart_get_named(name: &str) -> (bool, bool) {
    let Some(o) = reg(&["query", RUN_KEY, "/v", name]) else { return (false, false) };
    if !o.status.success() {
        return (false, false);
    }
    let v = parse_run_value(&String::from_utf8_lossy(&o.stdout), name);
    let here = autostart_command();
    (v.is_some(), v.is_some() && v == here)
}

pub fn autostart_set_named(name: &str, on: bool) -> Result<(), String> {
    let o = if on {
        let cmd = autostart_command().ok_or("не удалось определить путь программы")?;
        reg(&["add", RUN_KEY, "/v", name, "/t", "REG_SZ", "/d", &cmd, "/f"])
    } else {
        // удаление несуществующего значения не ошибка
        if !autostart_get_named(name).0 {
            return Ok(());
        }
        reg(&["delete", RUN_KEY, "/v", name, "/f"])
    };
    match o {
        Some(o) if o.status.success() => Ok(()),
        Some(o) => Err(format!("реестр не принял запись: {}", String::from_utf8_lossy(&o.stderr).trim())),
        None => Err("не удалось запустить reg.exe".into()),
    }
}

pub fn autostart_get() -> (bool, bool) {
    autostart_get_named(RUN_VALUE)
}

pub fn autostart_set(on: bool) -> Result<(), String> {
    autostart_set_named(RUN_VALUE, on)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reg_query_output() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\r\n    Nexora    REG_SZ    \"C:\\Program Files\\Nexora\\nexora.exe\" --background\r\n\r\n";
        assert_eq!(parse_run_value(out, "Nexora").as_deref(), Some("\"C:\\Program Files\\Nexora\\nexora.exe\" --background"));
        assert_eq!(parse_run_value(out, "Другое"), None);
        assert_eq!(parse_run_value("", "Nexora"), None);
        assert_eq!(parse_run_value("    NexoraOld    REG_SZ    x", "Nexora"), None, "имя должно совпадать целиком по границе слова: NexoraOld это другое значение");
    }

    #[test]
    fn command_is_quoted_with_background_flag() {
        let c = autostart_command().unwrap();
        assert!(c.starts_with('"') && c.ends_with("\" --background"), "{c}");
    }

    #[test]
    fn tray_icon_lookup_order() {
        let c = tray_icon_candidates();
        assert!(c.iter().any(|p| p.file_name().map_or(false, |n| n == "tray.png")) && c.iter().any(|p| p.file_name().map_or(false, |n| n == "tray.ico")));
        assert!(c.len() >= 2);
    }

    /// Запись и удаление в реестре под своим именем HKCU\...\Run\NexoraTest (убирается сразу).
    #[cfg(windows)]
    #[test]
    fn autostart_roundtrip_in_registry() {
        let name = "NexoraTestAutostart";
        let _ = autostart_set_named(name, false);
        assert_eq!(autostart_get_named(name), (false, false));
        autostart_set_named(name, true).expect("запись");
        assert_eq!(autostart_get_named(name), (true, true), "указывает на эту программу");
        autostart_set_named(name, false).expect("удаление");
        assert_eq!(autostart_get_named(name), (false, false));
        assert!(autostart_set_named(name, false).is_ok(), "повторное удаление не ошибка");
    }
}
