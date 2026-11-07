//! Поиск по всему компьютеру (только чтение имён и путей) и команды с правами администратора.
//! Содержимое файлов вне папки проекта не читается. Администратор: только через окно UAC и только после
//! подтверждения пользователя в Nexora; тихого повышения прав нет.
use crate::tools;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SKIP_DIRS: &[&str] = &["$recycle.bin", "system volume information", "$windows.~bt", "$windows.~ws"];

pub fn drives() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        (b'A'..=b'Z').map(|c| PathBuf::from(format!("{}:\\", c as char))).filter(|p| p.exists()).collect()
    }
    #[cfg(not(windows))]
    {
        vec![PathBuf::from("/")]
    }
}

/// Обход в глубину без переходов по ссылкам; visit возвращает false, чтобы остановить обход.
/// Результат: true, если обход дошёл до конца (не остановлен и не вышло время).
fn walk(roots: &[PathBuf], max_depth: usize, deadline: Instant, visit: &mut dyn FnMut(&Path, bool) -> bool) -> bool {
    let mut stack: Vec<(PathBuf, usize)> = roots.iter().rev().map(|p| (p.clone(), 0)).collect();
    while let Some((dir, d)) = stack.pop() {
        if Instant::now() > deadline {
            return false;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let p = e.path();
            if ft.is_dir() {
                let n = e.file_name().to_string_lossy().to_lowercase();
                if SKIP_DIRS.contains(&n.as_str()) {
                    continue;
                }
                if !visit(&p, true) {
                    return false;
                }
                if d < max_depth {
                    stack.push((p, d + 1));
                }
            } else if !visit(&p, false) {
                return false;
            }
        }
    }
    true
}

fn human(n: u64) -> String {
    if n >= 1 << 30 { format!("{:.1} ГБ", n as f64 / (1u64 << 30) as f64) }
    else if n >= 1 << 20 { format!("{:.1} МБ", n as f64 / (1u64 << 20) as f64) }
    else { format!("{} КБ", n.div_ceil(1024)) }
}

pub struct Query {
    pub text: String,
    pub ext: Option<String>,
    pub min_size: u64,
    pub max_results: usize,
}

/// Поиск файлов по имени / расширению / размеру. Секретные файлы в результат не попадают.
pub fn search_files(roots: &[PathBuf], q: &Query, limit: Duration) -> String {
    let needle = q.text.to_lowercase();
    let ext = q.ext.as_ref().map(|e| e.trim_start_matches('.').to_lowercase());
    let mut out: Vec<String> = Vec::new();
    let mut visited = 0u64;
    let done = walk(roots, 64, Instant::now() + limit, &mut |p, is_dir| {
        visited += 1;
        if is_dir || tools::is_secret(p) {
            return true;
        }
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
        if !needle.is_empty() && !name.contains(&needle) {
            return true;
        }
        if let Some(e) = &ext {
            if p.extension().and_then(|s| s.to_str()).map(|s| s.to_lowercase()).as_deref() != Some(e.as_str()) {
                return true;
            }
        }
        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        if size < q.min_size {
            return true;
        }
        out.push(format!("{} ({})", p.display(), human(size)));
        out.len() < q.max_results
    });
    let full = out.len() >= q.max_results;
    let mut s = if out.is_empty() { "Ничего не найдено".to_string() } else { out.join("\n") };
    if full {
        s.push_str(&format!("\n… показаны первые {} результатов, уточни запрос", q.max_results));
    } else if !done {
        s.push_str(&format!("\n… поиск остановлен по времени ({} с), просмотрено {visited} элементов; сузь папку или запрос", limit.as_secs()));
    }
    s
}

/// Установленные программы: реестр (список «Программы и компоненты»), ярлыки меню «Пуск», exe в Program Files.
pub fn find_apps(query: &str) -> String {
    let q = query.trim().to_lowercase();
    let mut lines: Vec<String> = Vec::new();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let script = "$p='HKLM:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\*','HKLM:\\Software\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\*','HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\*'; \
            [Console]::OutputEncoding=[Text.Encoding]::UTF8; \
            Get-ItemProperty $p -ErrorAction SilentlyContinue | Where-Object DisplayName | ForEach-Object { \"$($_.DisplayName)|$($_.DisplayVersion)|$($_.InstallLocation)\" }";
        if let Ok(o) = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .creation_flags(0x0800_0000)
            .output()
        {
            let mut seen = std::collections::HashSet::new();
            for l in String::from_utf8_lossy(&o.stdout).lines() {
                let mut it = l.splitn(3, '|');
                let (name, ver, loc) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""));
                if (q.is_empty() || name.to_lowercase().contains(&q)) && seen.insert(name.to_string()) {
                    lines.push(format!("[установлена] {name} {ver} {}", if loc.is_empty() { String::new() } else { format!("— {loc}") }).trim_end().to_string());
                }
            }
        }
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for (var, sub) in [("ProgramData", "Microsoft\\Windows\\Start Menu\\Programs"), ("APPDATA", "Microsoft\\Windows\\Start Menu\\Programs")] {
        if let Ok(b) = std::env::var(var) {
            roots.push(PathBuf::from(b).join(sub));
        }
    }
    let mut found = 0;
    walk(&roots, 4, Instant::now() + Duration::from_secs(10), &mut |p, is_dir| {
        if !is_dir && p.extension().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case("lnk")).unwrap_or(false) {
            let n = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if q.is_empty() || n.to_lowercase().contains(&q) {
                lines.push(format!("[ярлык «Пуск»] {n} — {}", p.display()));
                found += 1;
            }
        }
        found < 40
    });
    let mut exe_roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)"].iter().filter_map(|v| std::env::var(v).ok()).map(PathBuf::from).collect();
    if let Ok(l) = std::env::var("LOCALAPPDATA") {
        exe_roots.push(PathBuf::from(l).join("Programs"));
    }
    let mut exes = 0;
    if !q.is_empty() {
        walk(&exe_roots, 3, Instant::now() + Duration::from_secs(15), &mut |p, is_dir| {
            if !is_dir && p.extension().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case("exe")).unwrap_or(false) {
                let n = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
                if n.contains(&q) && !n.contains("unins") {
                    lines.push(format!("[exe] {}", p.display()));
                    exes += 1;
                }
            }
            exes < 40
        });
    }
    if lines.is_empty() { "Не найдено. Попробуй другое название или search_files по всем дискам.".into() } else { lines.join("\n") }
}

/// Причина, по которой команду нельзя выполнять даже с правами администратора.
pub fn elevated_block(cmd: &str) -> Option<&'static str> {
    if let tools::CmdClass::Blocked(why) = tools::classify(cmd) {
        return Some(why);
    }
    let c = cmd.to_lowercase();
    const EXTRA: &[(&str, &str)] = &[
        ("set-mppreference", "отключение или настройка защиты Windows"),
        ("add-mppreference", "исключения антивируса"),
        ("netsh advfirewall", "изменение брандмауэра"),
        ("set-executionpolicy", "изменение политики запуска скриптов"),
        ("takeown", "смена владельца файлов"),
        ("icacls", "изменение прав на файлы"),
        ("new-localuser", "создание учётных записей"),
        ("add-localgroupmember", "изменение групп пользователей"),
        ("-verb runas", "вложенное повышение прав"),
    ];
    EXTRA.iter().find(|(k, _)| c.contains(k)).map(|(_, why)| *why)
}

fn ps_sq(s: &str) -> String {
    s.replace('\'', "''")
}

/// Выполнить команду с правами администратора: системное окно UAC, вывод через временный файл в work_dir.
pub fn run_elevated(cmd: &str, work_dir: &Path, timeout: u64) -> String {
    let _ = std::fs::create_dir_all(work_dir);
    let mut idb = [0u8; 6];
    let _ = getrandom::getrandom(&mut idb);
    let id: String = idb.iter().map(|b| format!("{b:02x}")).collect();
    let script = work_dir.join(format!("elev-{id}.ps1"));
    let outf = work_dir.join(format!("elev-{id}.txt"));
    let body = format!(
        "$OutputEncoding=[Console]::OutputEncoding=[Text.Encoding]::UTF8\r\n& {{ {cmd}\r\n}} *>&1 | Out-String -Width 200 | Set-Content -Encoding UTF8 -Path '{}'\r\n",
        ps_sq(&outf.display().to_string())
    );
    if let Err(e) = std::fs::write(&script, body) {
        return format!("не удалось подготовить команду: {e}");
    }
    let outer = format!(
        "Start-Process -FilePath powershell -Verb RunAs -Wait -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','{}'",
        ps_sq(&script.display().to_string())
    );
    let res = tools::run_command(work_dir, &outer, timeout.clamp(30, 600));
    let text = std::fs::read_to_string(&outf).ok().map(|t| t.trim_start_matches('\u{feff}').trim().to_string());
    let _ = std::fs::remove_file(&script);
    let _ = std::fs::remove_file(&outf);
    match text {
        Some(t) => if t.is_empty() { "(команда выполнена, вывода нет)".to_string() } else { t },
        None => format!("Вывода нет: окно UAC отклонено или время вышло.\n{res}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nexora-scan-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        std::fs::create_dir_all(d.join("$Recycle.Bin")).unwrap();
        std::fs::write(d.join("a/b/Telegram.exe"), vec![0u8; 3000]).unwrap();
        std::fs::write(d.join("a/notes.txt"), b"x").unwrap();
        std::fs::write(d.join("a/.env"), b"KEY=1").unwrap();
        std::fs::write(d.join("$Recycle.Bin/hidden.exe"), b"x").unwrap();
        d
    }

    fn q(text: &str, ext: Option<&str>, min: u64) -> Query {
        Query { text: text.into(), ext: ext.map(String::from), min_size: min, max_results: 50 }
    }

    #[test]
    fn search_by_name_ext_size() {
        let d = tree("s");
        let r = search_files(&[d.clone()], &q("telegram", None, 0), Duration::from_secs(5));
        assert!(r.contains("Telegram.exe") && !r.contains("notes"), "{r}");
        let r = search_files(&[d.clone()], &q("", Some(".exe"), 0), Duration::from_secs(5));
        assert!(r.contains("Telegram.exe") && !r.contains("hidden.exe"), "корзина пропускается: {r}");
        let r = search_files(&[d.clone()], &q("", None, 2000), Duration::from_secs(5));
        assert!(r.contains("Telegram.exe") && !r.contains("notes"), "{r}");
        let r = search_files(&[d.clone()], &q("env", None, 0), Duration::from_secs(5));
        assert!(r.contains("Ничего"), "секретные файлы скрыты: {r}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn search_limits() {
        let d = tree("l");
        for i in 0..10 {
            std::fs::write(d.join(format!("a/f{i}.log")), b"x").unwrap();
        }
        let r = search_files(&[d.clone()], &Query { text: "f".into(), ext: Some("log".into()), min_size: 0, max_results: 3 }, Duration::from_secs(5));
        assert_eq!(r.lines().filter(|l| l.contains(".log")).count(), 3, "{r}");
        assert!(r.contains("уточни запрос"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn elevated_blocklist() {
        assert!(elevated_block("Set-MpPreference -DisableRealtimeMonitoring $true").is_some());
        assert!(elevated_block("netsh advfirewall set allprofiles state off").is_some());
        assert!(elevated_block("shutdown /r").is_some());
        assert!(elevated_block("Start-Process x -Verb RunAs").is_some());
        assert!(elevated_block("Get-Service | Where Status -eq Running").is_none());
        assert!(elevated_block("Restart-Service Spooler").is_none());
    }

    #[test]
    fn ps_quote_doubles() {
        assert_eq!(ps_sq("C:\\o'neil\\x"), "C:\\o''neil\\x");
    }

    #[cfg(windows)]
    #[test]
    fn find_apps_runs() {
        // PowerShell есть всегда; запрос заведомо несуществующий -> понятный ответ, без паники
        let r = find_apps("zzzz-no-such-app-12345");
        assert!(r.starts_with("Не найдено"), "{r}");
    }
}
