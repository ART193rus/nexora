//! Адреса VPN-слоя (Tailscale, WireGuard, ZeroTier) на этом ПК: через них телефон подключается к серверу из любой сети,
//! в том числе по мобильному интернету и за провайдерским NAT, без открытых портов. Сам VPN в Nexora не встроен:
//! программа только находит адаптер и слушает на его адресе.
use serde_json::Value;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub struct Adapter {
    pub name: String,
    pub ip: IpAddr,
}

/// Диапазон 100.64.0.0/10 (его выдаёт Tailscale).
fn in_cgnat(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (64..=127).contains(&o[1])
}

fn is_overlay(name: &str, ip: Ipv4Addr) -> bool {
    let n = name.to_lowercase();
    ["tailscale", "wireguard", "zerotier", "wintun"].iter().any(|k| n.contains(k)) || in_cgnat(ip)
}

/// Разбор вывода `Get-NetIPAddress | Select IPAddress,InterfaceAlias | ConvertTo-Json` (объект или массив).
pub fn parse(json: &str) -> Vec<Adapter> {
    let v: Value = serde_json::from_str(json.trim()).unwrap_or(Value::Null);
    let items: Vec<Value> = match v {
        Value::Array(a) => a,
        Value::Object(_) => vec![v],
        _ => vec![],
    };
    let mut out: Vec<Adapter> = vec![];
    for it in items {
        let (Some(ip), Some(name)) = (it["IPAddress"].as_str().and_then(|s| s.parse::<Ipv4Addr>().ok()), it["InterfaceAlias"].as_str()) else { continue };
        if is_overlay(name, ip) && !out.iter().any(|a| a.ip == IpAddr::V4(ip)) {
            out.push(Adapter { name: name.to_string(), ip: IpAddr::V4(ip) });
        }
    }
    out
}

static CACHE: LazyLock<Mutex<Option<(Instant, Vec<Adapter>)>>> = LazyLock::new(|| Mutex::new(None));

/// Найденные адаптеры VPN-слоя. Результат запоминается на 15 секунд (окно «Сервер» опрашивает состояние каждые 3 секунды).
pub fn detect() -> Vec<Adapter> {
    if let Some((t, v)) = CACHE.lock().unwrap().as_ref() {
        if t.elapsed() < Duration::from_secs(15) {
            return v.clone();
        }
    }
    let v = query();
    *CACHE.lock().unwrap() = Some((Instant::now(), v.clone()));
    v
}

#[cfg(windows)]
fn query() -> Vec<Adapter> {
    use std::os::windows::process::CommandExt;
    let o = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Get-NetIPAddress -AddressFamily IPv4 | Select-Object IPAddress,InterfaceAlias | ConvertTo-Json -Compress"])
        .creation_flags(0x0800_0000)
        .output();
    o.map(|o| parse(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

#[cfg(not(windows))]
fn query() -> Vec<Adapter> {
    vec![]
}

/// На каком дополнительном адресе слушать. None, если слушать нечего: сервер уже слушает на всех адресах (0.0.0.0),
/// сервер привязан именно к этому адресу или VPN-адаптера нет.
pub fn listen_addr(bind: &str, found: &[Adapter]) -> Option<IpAddr> {
    let b: Option<IpAddr> = bind.parse().ok();
    if b.map_or(false, |i| i.is_unspecified()) {
        return None;
    }
    found.iter().map(|a| a.ip).find(|ip| Some(*ip) != b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_array_and_single_object() {
        let arr = r#"[{"IPAddress":"192.168.1.120","InterfaceAlias":"Ethernet"},{"IPAddress":"100.101.102.103","InterfaceAlias":"Tailscale"},{"IPAddress":"10.8.0.2","InterfaceAlias":"WireGuard Tunnel"},{"IPAddress":"127.0.0.1","InterfaceAlias":"Loopback Pseudo-Interface 1"}]"#;
        let a = parse(arr);
        assert_eq!(a.iter().map(|x| x.ip.to_string()).collect::<Vec<_>>(), vec!["100.101.102.103", "10.8.0.2"]);
        assert_eq!(parse(r#"{"IPAddress":"100.64.0.5","InterfaceAlias":"Беспроводная сеть"}"#).len(), 1, "диапазон Tailscale без имени тоже признаётся");
        assert!(parse(r#"{"IPAddress":"192.168.1.5","InterfaceAlias":"Ethernet"}"#).is_empty());
        assert!(parse("").is_empty() && parse("не json").is_empty() && parse("null").is_empty());
        assert_eq!(parse(r#"[{"IPAddress":"100.101.102.103","InterfaceAlias":"Tailscale"},{"IPAddress":"100.101.102.103","InterfaceAlias":"Tailscale"}]"#).len(), 1, "дубликаты убираются");
    }

    #[test]
    fn cgnat_range() {
        assert!(in_cgnat("100.64.0.1".parse().unwrap()) && in_cgnat("100.127.255.254".parse().unwrap()));
        assert!(!in_cgnat("100.63.255.255".parse().unwrap()) && !in_cgnat("100.128.0.1".parse().unwrap()));
    }

    #[test]
    fn extra_listen_address_rules() {
        let ts = vec![Adapter { name: "Tailscale".into(), ip: "100.1.2.3".parse().unwrap() }];
        assert_eq!(listen_addr("192.168.1.120", &ts), Some("100.1.2.3".parse().unwrap()), "домашний адрес остаётся, добавляется адрес VPN");
        assert_eq!(listen_addr("0.0.0.0", &ts), None, "уже слушает на всех адресах");
        assert_eq!(listen_addr("100.1.2.3", &ts), None, "уже привязан к этому адресу");
        assert_eq!(listen_addr("192.168.1.120", &[]), None);
        assert_eq!(listen_addr("не адрес", &ts), Some("100.1.2.3".parse().unwrap()));
    }

    /// Живая проверка на этом ПК (только чтение): cargo test live_overlay -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_overlay() {
        println!("найдено: {:?}", detect());
    }
}
