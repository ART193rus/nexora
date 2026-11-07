//! Реестр Windows для агента: чтение свободно (значения с именами вроде password/token скрываются),
//! запись и удаление только по подтверждению пользователя (см. agent.rs, kind «registry»). Перед изменением
//! старое значение попадает в журнал, чтобы можно было откатить. Через PowerShell, без новых зависимостей.

pub struct RegPath {
    /// Путь для PowerShell: Registry::HKEY_LOCAL_MACHINE\SOFTWARE\...
    pub ps: String,
    /// Короткий вид для показа: HKLM\SOFTWARE\...
    pub display: String,
    /// Глубина ниже раздела верхнего уровня (HKLM\SOFTWARE = 1)
    pub depth: usize,
}

pub fn parse_path(input: &str) -> Result<RegPath, String> {
    let s = input.trim().trim_start_matches("Registry::").replace('/', "\\");
    if s.is_empty() || s.len() > 400 || s.chars().any(|c| c.is_control()) || s.contains("..") {
        return Err("недопустимый путь реестра".into());
    }
    let mut parts = s.split('\\').filter(|p| !p.is_empty());
    let hive_raw = parts.next().ok_or("не указан раздел реестра")?.trim_end_matches(':').to_uppercase();
    let (short, full) = match hive_raw.as_str() {
        "HKLM" | "HKEY_LOCAL_MACHINE" => ("HKLM", "HKEY_LOCAL_MACHINE"),
        "HKCU" | "HKEY_CURRENT_USER" => ("HKCU", "HKEY_CURRENT_USER"),
        "HKCR" | "HKEY_CLASSES_ROOT" => ("HKCR", "HKEY_CLASSES_ROOT"),
        "HKU" | "HKEY_USERS" => ("HKU", "HKEY_USERS"),
        _ => return Err("поддерживаются HKLM, HKCU, HKCR и HKU".into()),
    };
    let rest: Vec<&str> = parts.collect();
    let tail = rest.join("\\");
    Ok(RegPath {
        ps: if tail.is_empty() { format!("Registry::{full}") } else { format!("Registry::{full}\\{tail}") },
        display: if tail.is_empty() { short.to_string() } else { format!("{short}\\{tail}") },
        depth: rest.len(),
    })
}

/// Разделы, которые не меняются даже с согласием пользователя.
pub fn blocked_write(p: &RegPath) -> Option<&'static str> {
    let d = p.display.to_lowercase();
    const B: &[(&str, &str)] = &[
        ("hklm\\sam", "база учётных записей"),
        ("hklm\\security", "политики безопасности системы"),
        ("\\safeboot", "безопасный режим"),
        ("windows defender", "защита Windows"),
        ("\\services\\windefend", "защита Windows"),
        ("\\services\\sense", "защита Windows"),
        ("\\control\\lsa", "защита учётных данных"),
        ("\\winlogon\\", "вход в систему"),
    ];
    B.iter().find(|(k, _)| d.contains(k)).map(|(_, w)| *w)
}

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

const GUARD_START: &str = "$ErrorActionPreference='Stop'; try { ";
const GUARD_END: &str = " } catch { Write-Output ('ОШИБКА: ' + $_.Exception.Message); exit 1 }";

/// Чтение: значения раздела (или одно значение) и подразделы. Секретные имена скрываются.
pub fn read_script(p: &RegPath, name: Option<&str>) -> String {
    let show = r#"function Show($k, $n) { $kind = $k.GetValueKind($n); $v = $k.GetValue($n, $null, 'DoNotExpandEnvironmentNames'); $label = if ($n -eq '') { '(по умолчанию)' } else { $n }; if ($n -match 'pass|pwd|secret|token|credential|apikey|api_key|private') { $t = '*** (скрыто)' } elseif ($kind -eq 'Binary') { $b = [byte[]]$v; $t = ($b | Select-Object -First 48 | ForEach-Object { $_.ToString('x2') }) -join ' '; if ($b.Length -gt 48) { $t += ' …(' + $b.Length + ' байт)' } } elseif ($kind -eq 'MultiString') { $t = ($v -join ' | ') } else { $t = [string]$v }; if ($t.Length -gt 300) { $t = $t.Substring(0, 300) + '…' }; Write-Output ($label + ' [' + $kind + '] = ' + $t) } "#;
    let body = match name {
        Some(n) => format!("if (-not (Test-Path -LiteralPath {p})) {{ Write-Output 'нет такого раздела'; exit 3 }}; $k = Get-Item -LiteralPath {p}; if ($k.GetValueNames() -notcontains {n}) {{ Write-Output 'нет такого значения'; exit 4 }}; Show $k {n}", p = q(&p.ps), n = q(n)),
        None => format!("if (-not (Test-Path -LiteralPath {p})) {{ Write-Output 'нет такого раздела'; exit 3 }}; $k = Get-Item -LiteralPath {p}; Write-Output ('Раздел: ' + $k.Name); $names = @($k.GetValueNames()); Write-Output ('Значений: ' + $names.Count); foreach ($n in ($names | Select-Object -First 80)) {{ Show $k $n }}; $sub = @(Get-ChildItem -LiteralPath {p} -Name -ErrorAction SilentlyContinue); Write-Output ('Подразделов: ' + $sub.Count); foreach ($s in ($sub | Select-Object -First 100)) {{ Write-Output ('  ' + $s) }}", p = q(&p.ps)),
    };
    format!("[Console]::OutputEncoding=[Text.Encoding]::UTF8; {show}{GUARD_START}{body}{GUARD_END}")
}

pub fn value_literal(ty: &str, value: &str) -> Result<(String, &'static str), String> {
    Ok(match ty.to_lowercase().as_str() {
        "string" | "sz" | "reg_sz" => (q(value), "String"),
        "expandstring" | "expand_sz" | "reg_expand_sz" => (q(value), "ExpandString"),
        "dword" | "reg_dword" => {
            let n = parse_num(value)?;
            if n > u32::MAX as u64 {
                return Err("DWORD: число от 0 до 4294967295".into());
            }
            (n.to_string(), "DWord")
        }
        "qword" | "reg_qword" => (parse_num(value)?.to_string(), "QWord"),
        "multistring" | "multi_sz" | "reg_multi_sz" => (format!("@({})", value.lines().map(q).collect::<Vec<_>>().join(",")), "MultiString"),
        _ => return Err("тип: String, ExpandString, DWord, QWord или MultiString".into()),
    })
}

fn parse_num(v: &str) -> Result<u64, String> {
    let t = v.trim();
    let r = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) { u64::from_str_radix(h, 16) } else { t.parse::<u64>() };
    r.map_err(|_| "число: десятичное или 0x…".to_string())
}

pub fn write_script(p: &RegPath, name: &str, ty: &str, value: &str) -> Result<String, String> {
    let (lit, kind) = value_literal(ty, value)?;
    Ok(format!(
        "{GUARD_START}if (-not (Test-Path -LiteralPath {p})) {{ New-Item -Path {p} -Force | Out-Null }}; New-ItemProperty -LiteralPath {p} -Name {n} -Value {lit} -PropertyType {kind} -Force | Out-Null; Write-Output 'записано'{GUARD_END}",
        p = q(&p.ps),
        n = q(name)
    ))
}

pub fn delete_script(p: &RegPath, name: Option<&str>) -> String {
    match name {
        Some(n) => format!("{GUARD_START}Remove-ItemProperty -LiteralPath {} -Name {} -Force; Write-Output 'значение удалено'{GUARD_END}", q(&p.ps), q(n)),
        None => format!("{GUARD_START}Remove-Item -LiteralPath {} -Recurse -Force; Write-Output 'раздел удалён'{GUARD_END}", q(&p.ps)),
    }
}

/// Строка журнала изменения (для отката вручную).
pub fn log_line(action: &str, p: &RegPath, name: &str, old: &str, new: &str) -> String {
    let ts = crate::tasks::local_now();
    format!("{} {:02}:{:02} | {action} | {}\\{name} | было: {} | стало: {}\n", ts.date, ts.minutes / 60, ts.minutes % 60, p.display, old.trim().replace('\n', " / "), new)
}

pub fn run(script: &str, timeout: u64) -> (bool, String) {
    let out = crate::tools::run_command(&crate::engine::data_dir(), script, timeout);
    let ok = out.contains("[код выхода: 0]");
    let text = out.lines().filter(|l| !l.starts_with("[код выхода")).collect::<Vec<_>>().join("\n").trim().to_string();
    (ok, crate::ssh::redact(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        let a = parse_path("HKLM:\\SOFTWARE\\Policies\\Google\\Chrome").unwrap();
        assert_eq!((a.display.as_str(), a.depth), ("HKLM\\SOFTWARE\\Policies\\Google\\Chrome", 4));
        assert_eq!(a.ps, "Registry::HKEY_LOCAL_MACHINE\\SOFTWARE\\Policies\\Google\\Chrome");
        let b = parse_path("hkey_current_user/Software/X").unwrap();
        assert_eq!(b.ps, "Registry::HKEY_CURRENT_USER\\Software\\X");
        assert_eq!(parse_path("Registry::HKCU\\Software").unwrap().display, "HKCU\\Software");
        assert_eq!(parse_path("HKCU").unwrap().depth, 0);
        assert!(parse_path("C:\\Windows").is_err());
        assert!(parse_path("HKLM\\..\\x").is_err());
        assert!(parse_path("").is_err());
    }

    #[test]
    fn protected_areas() {
        for p in ["HKLM\\SAM\\SAM", "HKLM\\SYSTEM\\CurrentControlSet\\Control\\SafeBoot\\Minimal", "HKLM\\SOFTWARE\\Policies\\Microsoft\\Windows Defender", "HKLM\\SYSTEM\\CurrentControlSet\\Services\\WinDefend", "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Lsa"] {
            assert!(blocked_write(&parse_path(p).unwrap()).is_some(), "{p}");
        }
        assert!(blocked_write(&parse_path("HKCU\\Software\\NexoraTest").unwrap()).is_none());
        assert!(blocked_write(&parse_path("HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run").unwrap()).is_none());
    }

    #[test]
    fn literals_and_quoting() {
        assert_eq!(value_literal("DWord", "0x10").unwrap(), ("16".to_string(), "DWord"));
        assert_eq!(value_literal("string", "it's").unwrap().0, "'it''s'");
        assert_eq!(value_literal("MultiString", "a\nb").unwrap().0, "@('a','b')");
        assert!(value_literal("DWord", "-1").is_err());
        assert!(value_literal("DWord", "4294967296").is_err());
        assert!(value_literal("binary", "00").is_err());
        let p = parse_path("HKCU\\Software\\O'Neil").unwrap();
        let s = write_script(&p, "n'am", "String", "v").unwrap();
        assert!(s.contains("'Registry::HKEY_CURRENT_USER\\Software\\O''Neil'") && s.contains("'n''am'"), "{s}");
        assert!(delete_script(&p, None).contains("Remove-Item") && delete_script(&p, Some("x")).contains("Remove-ItemProperty"));
    }

    /// Чтение настоящего реестра и запись/удаление в собственном ключе HKCU\Software\NexoraTest (убирается сразу).
    #[cfg(windows)]
    #[test]
    fn live_read_write_delete() {
        let (ok, t) = run(&read_script(&parse_path("HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer").unwrap(), None), 30);
        assert!(ok && t.contains("Раздел: HKEY_CURRENT_USER"), "{t}");
        let (ok, t) = run(&read_script(&parse_path("HKCU\\Software\\NexoraNoSuchKey").unwrap(), None), 30);
        assert!(!ok && t.contains("нет такого раздела"), "{t}");

        let key = parse_path("HKCU\\Software\\NexoraTest").unwrap();
        let _ = run(&delete_script(&key, None), 30);
        let (ok, t) = run(&write_script(&key, "Counter", "DWord", "42").unwrap(), 30);
        assert!(ok && t.contains("записано"), "{t}");
        let (ok, t) = run(&write_script(&key, "DB_PASSWORD", "String", "hunter2").unwrap(), 30);
        assert!(ok, "{t}");
        let (ok, t) = run(&read_script(&key, Some("Counter")), 30);
        assert!(ok && t.contains("Counter [DWord] = 42"), "{t}");
        let (_, all) = run(&read_script(&key, None), 30);
        assert!(all.contains("Значений: 2") && !all.contains("hunter2") && all.contains("скрыто"), "секретное имя скрыто: {all}");
        let (ok, t) = run(&delete_script(&key, Some("Counter")), 30);
        assert!(ok && t.contains("значение удалено"), "{t}");
        let (ok, t) = run(&read_script(&key, Some("Counter")), 30);
        assert!(!ok && t.contains("нет такого значения"), "{t}");
        let (ok, _) = run(&delete_script(&key, None), 30);
        assert!(ok);
        let (ok, t) = run(&read_script(&key, None), 30);
        assert!(!ok && t.contains("нет такого раздела"), "ключ убран: {t}");
    }
}
