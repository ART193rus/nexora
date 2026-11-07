//! Режим «Сервер»: включение и выключение слушателя, хранение ключей и устройств в зашифрованном хранилище (DPAPI),
//! данные для привязки телефона (секрет Authenticator, QR, строка подключения).
//!
//! Секрет Authenticator, ключ центра сертификации и ключ сервера лежат только в хранилище (AES-GCM, ключ привязан к
//! учётной записи Windows). Открытые настройки (адрес, порт, переключатель команд) лежат в server.json и секретов не содержат.
use crate::mobile::{Exec, Hub};
use crate::pki::{self, Ca, Issued};
use crate::secure::Vault;
use crate::server::{Core, Device, Server};
use crate::totp;
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn vault_dir() -> PathBuf {
    crate::engine::data_dir().join("vault")
}

fn settings_path() -> PathBuf {
    crate::engine::data_dir().join("server.json")
}

#[derive(Clone)]
pub struct Settings {
    pub bind: String,
    pub port: u16,
    pub names: Vec<String>,
    pub tools: bool,
    /// «Виден в локальной сети»: объявлять сервер по mDNS (по умолчанию выключено).
    pub visible: bool,
    /// «Доступ из любой точки»: слушать ещё и на адресе VPN-слоя (Tailscale и т.п.). По умолчанию выключено.
    pub overlay: bool,
    /// Включать сервер для устройств при запуске программы (в том числе после перезагрузки ПК).
    pub autorun: bool,
}

impl Settings {
    pub fn load() -> Settings {
        let v: Value = std::fs::read_to_string(settings_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
        let port = v["port"].as_u64().filter(|p| (1024..=65535).contains(p)).unwrap_or_else(random_port) as u16;
        Settings {
            bind: v["bind"].as_str().unwrap_or("127.0.0.1").to_string(),
            port,
            names: v["names"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
            tools: v["tools"].as_bool().unwrap_or(false),
            visible: v["visible"].as_bool().unwrap_or(false),
            overlay: v["overlay"].as_bool().unwrap_or(false),
            autorun: v["autorun"].as_bool().unwrap_or(false),
        }
    }

    pub fn save(&self) {
        let _ = std::fs::write(settings_path(), json!({"bind": self.bind, "port": self.port, "names": self.names, "tools": self.tools, "visible": self.visible, "overlay": self.overlay, "autorun": self.autorun}).to_string());
    }
}

/// Случайный порт из 20000–60000.
fn random_port() -> u64 {
    let mut b = [0u8; 4];
    let _ = getrandom::getrandom(&mut b);
    20000 + (u32::from_le_bytes(b) as u64 % 40000)
}

/// Основной адрес этого ПК в локальной сети (пакеты не отправляются, система только выбирает маршрут).
pub fn lan_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

pub fn qr_svg(text: &str) -> String {
    qrcode::QrCode::new(text.as_bytes())
        .map(|c| c.render::<qrcode::render::svg::Color>().min_dimensions(220, 220).quiet_zone(true).dark_color(qrcode::render::svg::Color("#000000")).light_color(qrcode::render::svg::Color("#ffffff")).build())
        .unwrap_or_default()
}

#[derive(Default)]
pub struct Host {
    server: Mutex<Option<Server>>,
    advert: Mutex<Option<crate::discover::Advert>>,
    /// Дополнительные слушатели (адрес VPN-слоя) с тем же ядром, сертификатом и устройствами.
    extra: Mutex<Vec<Server>>,
}

fn load_json(v: &Vault, name: &str) -> Option<Value> {
    v.get(name).ok().flatten().and_then(|b| serde_json::from_slice(&b).ok())
}

impl Host {
    pub fn running(&self) -> bool {
        self.server.lock().unwrap().is_some()
    }

    /// Включить сервер. Первый запуск создаёт центр сертификации, секрет Authenticator и сертификат сервера.
    pub fn enable(&self, hub: &Arc<Hub>, exec: Exec, status: Arc<dyn Fn() -> Value + Send + Sync>, control: Arc<dyn crate::server::Control>, set: &Settings) -> Result<(), String> {
        if self.running() {
            return Ok(());
        }
        let bind_ip: IpAddr = set.bind.parse().map_err(|_| "адрес для прослушивания должен быть IP-адресом (например 192.168.1.5 или 100.x.x.x)".to_string())?;
        let vault = Vault::open(&vault_dir(), None)?;
        let ca = match load_json(&vault, "host_ca") {
            Some(v) => Ca::from_pem(v["cert"].as_str().unwrap_or(""), v["key"].as_str().unwrap_or(""))?,
            None => {
                let ca = Ca::create("Nexora")?;
                vault.put("host_ca", json!({"cert": ca.cert_pem, "key": ca.key_pem()}).to_string().as_bytes())?;
                ca
            }
        };
        let secret = match vault.get("host_totp")? {
            Some(b) => String::from_utf8(b).map_err(|e| e.to_string())?,
            None => {
                let s = totp::new_secret()?;
                vault.put("host_totp", s.as_bytes())?;
                s
            }
        };
        let mut names: Vec<String> = vec!["127.0.0.1".into()];
        if !bind_ip.is_unspecified() {
            names.push(bind_ip.to_string());
        }
        if let Some(ip) = lan_ip() {
            names.push(ip.to_string());
        }
        names.extend(set.names.iter().cloned());
        names.extend(crate::overlay::detect().iter().map(|a| a.ip.to_string()));
        names.sort();
        names.dedup();
        // Сертификат создаётся один раз и больше не пересоздаётся при смене адреса: привязанные устройства закрепляют его отпечаток,
        // и при пересоздании (например, после смены IP по DHCP) все они потеряли бы связь. Имена в нём только справочные.
        let cert: Issued = match load_json(&vault, "host_cert") {
            Some(v) => Issued {
                cert_pem: v["cert"].as_str().unwrap_or("").into(),
                key_pem: v["key"].as_str().unwrap_or("").into(),
                serial: String::new(),
                fingerprint: pki::fingerprint_pem(v["cert"].as_str().unwrap_or(""))?,
            },
            None => {
                let c = ca.issue_server(&names)?;
                vault.put("host_cert", json!({"cert": c.cert_pem, "key": c.key_pem, "names": names}).to_string().as_bytes())?;
                c
            }
        };
        let devices: Vec<Device> = load_json(&vault, "host_devices").and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
        hub.set_trusted(devices.iter().filter(|d| d.trusted && !d.revoked).map(|d| d.serial.clone()).collect());
        let core = Arc::new(Core::with_hub(ca, &secret, devices, hub.clone())?);
        core.on_devices(Box::new(|list| {
            if let Ok(v) = Vault::open(&vault_dir(), None) {
                let _ = v.put("host_devices", serde_json::to_vec(&list).unwrap_or_default().as_slice());
            }
        }));
        core.set_status(status);
        core.set_control(control);
        let srv = Server::start(core, &cert, SocketAddr::new(bind_ip, set.port))?;
        if !hub.running() {
            hub.start(exec);
        }
        *self.server.lock().unwrap() = Some(srv);
        let _ = self.apply_visibility(set); // ошибка объявления не мешает работе сервера: причина видна при включении настройки
        let _ = self.apply_overlay(set); // то же для адреса VPN
        Ok(())
    }

    /// Включить или снять объявление сервера в локальной сети (mDNS) по настройке «Виден в локальной сети».
    pub fn apply_visibility(&self, set: &Settings) -> Result<(), String> {
        *self.advert.lock().unwrap() = None; // снять прежнее объявление
        if !set.visible || !self.running() {
            return Ok(());
        }
        let vault = Vault::open(&vault_dir(), None)?;
        let cert = load_json(&vault, "host_cert").ok_or("сервер ещё не настроен")?;
        let fp = pki::short(&pki::fingerprint_pem(cert["cert"].as_str().unwrap_or(""))?);
        let ip: IpAddr = set.bind.parse().ok().filter(|i: &IpAddr| !i.is_unspecified()).or_else(lan_ip).ok_or("нет адреса в локальной сети")?;
        let name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Nexora".into());
        *self.advert.lock().unwrap() = Some(crate::discover::advertise(&name, ip, set.port, &fp)?);
        Ok(())
    }

    /// Слушать дополнительно на адресе VPN-слоя (Tailscale и т.п.), чтобы телефон подключался из любой сети.
    /// Домашний адрес остаётся: сервер доступен и дома, и из любой точки, а не только где-то одном.
    pub fn apply_overlay(&self, set: &Settings) -> Result<(), String> {
        self.extra.lock().unwrap().clear(); // снять прежний дополнительный слушатель
        if !set.overlay {
            return Ok(());
        }
        let g = self.server.lock().unwrap();
        let Some(main) = g.as_ref() else { return Ok(()) };
        let found = crate::overlay::detect();
        if found.is_empty() {
            return Err("адаптер Tailscale (VPN) на этом ПК не найден: установите его и войдите в аккаунт".into());
        }
        let Some(ip) = crate::overlay::listen_addr(&set.bind, &found) else { return Ok(()) };
        let vault = Vault::open(&vault_dir(), None)?;
        let v = load_json(&vault, "host_cert").ok_or("сервер ещё не настроен")?;
        let cert = Issued { cert_pem: v["cert"].as_str().unwrap_or("").into(), key_pem: v["key"].as_str().unwrap_or("").into(), serial: String::new(), fingerprint: pki::fingerprint_pem(v["cert"].as_str().unwrap_or(""))? };
        let srv = Server::start(main.core.clone(), &cert, SocketAddr::new(ip, set.port))?;
        self.extra.lock().unwrap().push(srv);
        Ok(())
    }

    /// Данные для доступа из любой точки: адреса VPN-слоя, строка подключения с этим адресом и QR.
    pub fn remote_info(&self, set: &Settings) -> Result<Value, String> {
        let found = crate::overlay::detect();
        let vault = Vault::open(&vault_dir(), None)?;
        let fp = load_json(&vault, "host_cert").map(|c| pki::fingerprint_pem(c["cert"].as_str().unwrap_or(""))).transpose()?;
        let adapters: Vec<Value> = found.iter().map(|a| json!({"name": a.name, "ip": a.ip.to_string()})).collect();
        let mut v = json!({"enabled": set.overlay, "listening": !self.extra.lock().unwrap().is_empty() || (set.overlay && set.bind.parse::<IpAddr>().map_or(false, |i| i.is_unspecified()) && self.running()), "adapters": adapters, "port": set.port});
        if let (Some(a), Some(fp)) = (found.first(), fp) {
            let link = format!("nexora://{}:{}#{fp}", a.ip, set.port);
            v["link"] = json!(link);
            v["link_qr"] = json!(qr_svg(&link));
        }
        Ok(v)
    }

    pub fn disable(&self) {
        *self.advert.lock().unwrap() = None;
        self.extra.lock().unwrap().clear();
        if let Some(mut s) = self.server.lock().unwrap().take() {
            s.core.hub().cancel_all(); // задачи не должны продолжаться после выключения сервера
            s.stop();
        }
    }

    pub fn open_window(&self) -> Result<(), String> {
        self.server.lock().unwrap().as_ref().map(|s| s.core.open_window(crate::server::WINDOW_SECS)).ok_or_else(|| "сервер выключен".to_string())
    }

    pub fn trust(&self, serial: &str, ok: bool) -> bool {
        self.server.lock().unwrap().as_ref().map_or(false, |s| s.core.set_trust(serial, ok))
    }

    pub fn revoke(&self, serial: &str) -> bool {
        self.server.lock().unwrap().as_ref().map_or(false, |s| s.core.revoke(serial))
    }

    /// Что показать в окне: состояние, устройства, журнал, данные для привязки.
    pub fn state(&self, set: &Settings) -> Value {
        let vault = Vault::open(&vault_dir(), None).ok();
        let configured = vault.as_ref().and_then(|v| v.get("host_totp").ok().flatten()).is_some();
        let g = self.server.lock().unwrap();
        let mut v = json!({"running": g.is_some(), "configured": configured, "bind": set.bind, "port": set.port, "names": set.names, "tools": set.tools, "visible": set.visible, "overlay": set.overlay, "autorun": set.autorun, "lan_ip": lan_ip().map(|i| i.to_string())});
        if let Some(s) = g.as_ref() {
            let devices: Vec<Value> = s.core.devices().iter().map(|d| json!({"serial": d.serial, "name": d.name, "added": d.added, "revoked": d.revoked, "trusted": d.trusted, "trust_pending": s.core.trust_pending(&d.serial)})).collect();
            let log: Vec<Value> = s.core.log().iter().rev().take(50).map(|l| json!({"ts": l.ts, "ip": l.ip, "event": l.event})).collect();
            v["devices"] = json!(devices);
            v["log"] = json!(log);
            v["window_open"] = json!(s.core.window_open());
        }
        v
    }

    /// Данные для привязки телефона: секрет Authenticator (показывается только на этом ПК), QR и строка подключения.
    pub fn setup_info(&self, set: &Settings) -> Result<Value, String> {
        let vault = Vault::open(&vault_dir(), None)?;
        let secret = String::from_utf8(vault.get("host_totp")?.ok_or("сначала включите сервер")?).map_err(|e| e.to_string())?;
        let cert = load_json(&vault, "host_cert").ok_or("сначала включите сервер")?;
        let fp = pki::fingerprint_pem(cert["cert"].as_str().unwrap_or(""))?;
        let host = if set.bind.parse::<IpAddr>().map_or(false, |i| !i.is_unspecified()) { set.bind.clone() } else { lan_ip().map(|i| i.to_string()).unwrap_or_else(|| "127.0.0.1".into()) };
        let link = format!("nexora://{host}:{}#{fp}", set.port);
        let uri = totp::otpauth_uri(&secret, "Nexora Server", "Nexora");
        Ok(json!({"secret": secret, "otpauth": uri, "otpauth_qr": qr_svg(&uri), "link": link, "link_qr": qr_svg(&link), "fingerprint_short": pki::short(&fp)}))
    }
}
