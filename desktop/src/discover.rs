//! Поиск сервера в локальной сети (mDNS / DNS-SD, служба `_nexora._tcp`). Правила из PLAN.md:
//! объявляется только имя, порт и короткий отпечаток; только на частных адресах (192.168.x, 10.x, 172.16-31.x);
//! по умолчанию выключено («скрытый режим»). Найти сервер не значит войти: вход по-прежнему требует сертификат
//! устройства и код Authenticator, а отпечаток из объявления можно подменить, поэтому при первой привязке его надо
//! сверить с экраном сервера.
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

pub const SERVICE: &str = "_nexora._tcp.local.";

#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub name: String,
    pub host: String,
    pub port: u16,
    /// Короткий отпечаток для сверки глазами (a1b2-c3d4-e5f6-0718).
    pub fp: String,
}

pub fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            o[0] == 10 || (o[0] == 172 && (16..=31).contains(&o[1])) || (o[0] == 192 && o[1] == 168)
        }
        IpAddr::V6(_) => false,
    }
}

/// Имя экземпляра для объявления: без точек и управляющих символов, до 40 символов.
pub fn clean_name(s: &str) -> String {
    let n: String = s.chars().filter(|c| !c.is_control() && *c != '.').take(40).collect();
    let n = n.trim().to_string();
    if n.is_empty() { "Nexora".to_string() } else { n }
}

/// Объявление сервера в сети; пока объект жив, сервер виден. Drop снимает объявление.
pub struct Advert {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Drop for Advert {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        std::thread::sleep(Duration::from_millis(150)); // дать отправить «прощальный» пакет
        let _ = self.daemon.shutdown();
    }
}

pub fn advertise(instance: &str, ip: IpAddr, port: u16, fp_short: &str) -> Result<Advert, String> {
    if !is_private(ip) {
        return Err("поиск в сети включается только на частном адресе (192.168.x.x, 10.x.x.x, 172.16-31.x.x): на этом адресе объявление запрещено".into());
    }
    let name = clean_name(instance);
    let host_name = format!("nexora-{}.local.", name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>());
    let props: HashMap<String, String> = [("fp".to_string(), fp_short.to_string()), ("v".to_string(), "1".to_string())].into();
    let info = ServiceInfo::new(SERVICE, &name, &host_name, ip.to_string().as_str(), port, props).map_err(|e| e.to_string())?;
    let fullname = info.get_fullname().to_string();
    let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
    daemon.register(info).map_err(|e| e.to_string())?;
    Ok(Advert { daemon, fullname })
}

/// Найти серверы в сети: слушает объявления указанное время. Адреса вне частных диапазонов отбрасываются.
pub fn browse(timeout: Duration) -> Result<Vec<Found>, String> {
    let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
    let rx = daemon.browse(SERVICE).map_err(|e| e.to_string())?;
    let end = Instant::now() + timeout;
    let mut out: Vec<Found> = vec![];
    while Instant::now() < end {
        let left = end.saturating_duration_since(Instant::now()).min(Duration::from_millis(300));
        if let Ok(ServiceEvent::ServiceResolved(info)) = rx.recv_timeout(left) {
            let host = info.get_addresses().iter().copied().find(|a| is_private(*a) && a.is_ipv4());
            if let Some(h) = host {
                let f = Found {
                    name: info.get_fullname().trim_end_matches(SERVICE).trim_end_matches('.').to_string(),
                    host: h.to_string(),
                    port: info.get_port(),
                    fp: info.get_property_val_str("fp").unwrap_or("").to_string(),
                };
                if !out.iter().any(|x| x.host == f.host && x.port == f.port) {
                    out.push(f);
                }
            }
        }
    }
    let _ = daemon.shutdown();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_ranges() {
        for ip in ["192.168.1.5", "10.0.0.1", "172.16.0.1", "172.31.255.1"] {
            assert!(is_private(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "172.32.0.1", "172.15.0.1", "100.64.0.1", "127.0.0.1", "169.254.1.1", "::1"] {
            assert!(!is_private(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn names_are_clean() {
        assert_eq!(clean_name("  Дом.ПК \n"), "ДомПК");
        assert_eq!(clean_name("..."), "Nexora");
        assert_eq!(clean_name(&"а".repeat(80)).chars().count(), 40);
    }

    #[test]
    fn public_address_is_refused() {
        let e = advertise("x", "8.8.8.8".parse().unwrap(), 1234, "abcd").err().unwrap();
        assert!(e.contains("частном адресе"));
        assert!(advertise("x", "127.0.0.1".parse().unwrap(), 1234, "abcd").is_err());
    }

    /// Живая проверка в сети этого ПК: cargo test live_discover -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_discover() {
        let ip = crate::hostmode::lan_ip().expect("нет LAN-адреса");
        let ad = advertise("Nexora Test", ip, 47999, "a1b2-c3d4-e5f6-0718").expect("advertise");
        let found = browse(Duration::from_secs(8)).expect("browse");
        println!("найдено: {found:?}");
        assert!(found.iter().any(|f| f.port == 47999 && f.fp == "a1b2-c3d4-e5f6-0718" && f.host == ip.to_string()), "{found:?}");
        drop(ad);
    }
}
