//! Режим «Сервер»: слушатель TLS 1.3 (rustls) с проверкой клиентского сертификата, привязка устройства по коду
//! Authenticator, окно привязки, журнал, блокировка IP, отзыв устройств.
//!
//! Правила: клиент без сертификата получает ответ ТОЛЬКО на POST /enroll и только пока окно привязки открыто;
//! всё остальное закрывается без единого байта ответа. Отозванный или чужой сертификат так же молча закрывается.
//! Ответ при отказе всегда один и тот же («denied»), без подсказок, что именно не так.
use crate::mobile::Hub;
use crate::pki::{self, Ca};
use crate::totp::{self, Verdict, Verifier};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig, ServerConnection, StreamOwned};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const WINDOW_SECS: u64 = 10 * 60;
const IP_MAX_FAILS: u32 = 5;
const IP_LOCK_SECS: u64 = 15 * 60;
const MAX_CONN: usize = 32;
const MAX_PER_IP: usize = 8;
/// Соединения, ещё не предъявившие действительный сертификат устройства: их не больше половины пула.
const MAX_PRE_AUTH: usize = 12;
const HANDSHAKE_SECS: u64 = 8;
const DEVICE_REQUEST_SECS: u64 = 30;
const READ_STALL_SECS: u64 = 5;
const MAX_SMALL_BODY: usize = 128 * 1024;
const MAX_BODY: usize = 64 * 1024;
const LOG_KEEP: usize = 500;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Device {
    pub serial: String,
    pub name: String,
    pub fingerprint: String,
    pub added: u64,
    pub revoked: bool,
    /// Владелец разрешил этому устройству «режим действий» (агент выполняет обычные действия без вопросов).
    #[serde(default)]
    pub trusted: bool,
}

#[derive(Clone, Serialize, Debug)]
pub struct LogEntry {
    pub ts: u64,
    pub ip: String,
    pub event: String,
}

/// Куда пересылать чат: (адрес движка, ключ сессии, псевдоним модели). None = модель не запущена.
pub type Upstream = Arc<dyn Fn() -> Option<(String, String, String)> + Send + Sync>;

const MAX_AUTH_BODY: usize = 16 * 1024 * 1024; // картинки в base64
const MAX_CHATS: usize = 2;
/// Сколько живёт сертификат на выдаче и сколько их может ждать одновременно.
const PICKUP_TTL: u64 = 10 * 60;
const MAX_PICKUPS: usize = 5;
/// Поля запроса, которые клиент вправе передать модели. Всё остальное отбрасывается.
const CHAT_FIELDS: &[&str] = &["messages", "tools", "tool_choice", "temperature", "top_p", "top_k", "repeat_penalty", "max_tokens", "stop", "stream"];

/// Управление приложением на ПК с телефона: список моделей и выбор модели.
pub trait Control: Send + Sync {
    /// {"current": id, "ready": bool, "models": [{"id","label"}]}
    fn models(&self) -> Value;
    fn select(&self, id: &str) -> Result<(), String>;
}

pub struct Core {
    control: Mutex<Option<Arc<dyn Control>>>,
    upstream: Mutex<Option<Upstream>>,
    chats: AtomicUsize,
    ca: Ca,
    verifier: Mutex<Verifier>,
    /// Отдельный счётчик ошибок для подтверждений и открытия окна привязки: блокировка одного не мешает другому.
    approver: Mutex<Verifier>,
    persist_lock: Mutex<()>,
    devices: Mutex<Vec<Device>>,
    window_until: AtomicU64,
    log: Mutex<VecDeque<LogEntry>>,
    ip_fails: Mutex<HashMap<IpAddr, (u32, u64)>>,
    trust_requests: Mutex<HashMap<String, u64>>,
    /// Хеш номера -> (сертификат, серийный номер, время): ждут, пока новое устройство заберёт.
    pickups: Mutex<HashMap<String, (String, String, u64)>>,
    hub: Arc<Hub>,
    on_devices: Mutex<Option<Box<dyn Fn(Vec<Device>) + Send + Sync>>>,
    status: Mutex<Option<Arc<dyn Fn() -> Value + Send + Sync>>>,
}

impl Core {
    pub fn new(ca: Ca, totp_secret_b32: &str, devices: Vec<Device>) -> Result<Core, String> {
        Core::with_hub(ca, totp_secret_b32, devices, Hub::new(Arc::new(crate::agent::Approvals::default())))
    }

    pub fn with_hub(ca: Ca, totp_secret_b32: &str, devices: Vec<Device>, hub: Arc<Hub>) -> Result<Core, String> {
        Ok(Core {
            upstream: Mutex::new(None),
            chats: AtomicUsize::new(0),
            ca,
            verifier: Mutex::new(Verifier::new(totp_secret_b32).ok_or("неверный секрет Authenticator")?),
            approver: Mutex::new(Verifier::new(totp_secret_b32).ok_or("неверный секрет Authenticator")?),
            persist_lock: Mutex::new(()),
            devices: Mutex::new(devices),
            window_until: AtomicU64::new(0),
            log: Mutex::new(VecDeque::new()),
            ip_fails: Mutex::new(HashMap::new()),
            trust_requests: Mutex::new(HashMap::new()),
            pickups: Mutex::new(HashMap::new()),
            hub,
            control: Mutex::new(None),
            on_devices: Mutex::new(None),
            status: Mutex::new(None),
        })
    }

    pub fn hub(&self) -> Arc<Hub> {
        self.hub.clone()
    }

    // ---- выдача сертификата новому устройству, подтверждённому с уже привязанного (по QR) ----
    fn prune_pickups(m: &mut HashMap<String, (String, String, u64)>) {
        let now = totp::now_secs();
        m.retain(|_, (_, _, t)| now < *t + PICKUP_TTL);
    }
    /// Есть ли ожидающие выдачи: пока нет, неопознанным соединениям на /v1/pickup отвечать нечего (тишина).
    fn has_pickups(&self) -> bool {
        let mut m = self.pickups.lock().unwrap();
        Core::prune_pickups(&mut m);
        !m.is_empty()
    }
    fn pickup_room(&self) -> bool {
        let mut m = self.pickups.lock().unwrap();
        Core::prune_pickups(&mut m);
        m.len() < MAX_PICKUPS
    }
    fn put_pickup(&self, nonce_hash: String, cert_pem: String, serial: String) {
        self.pickups.lock().unwrap().insert(nonce_hash, (cert_pem, serial, totp::now_secs()));
    }
    /// Забрать сертификат по хешу номера; одноразово.
    fn take_pickup(&self, nonce_hash: &str) -> Option<(String, String)> {
        let mut m = self.pickups.lock().unwrap();
        Core::prune_pickups(&mut m);
        m.remove(nonce_hash).map(|(c, s, _)| (c, s))
    }
    /// Вызывается при каждом изменении списка устройств (привязка, отзыв): программа сохраняет его в хранилище.
    pub fn on_devices(&self, f: Box<dyn Fn(Vec<Device>) + Send + Sync>) {
        *self.on_devices.lock().unwrap() = Some(f);
    }
    /// Устройство просит «режим действий». Одобрить может только владелец в программе на ПК.
    pub fn request_trust(&self, serial: &str) {
        self.trust_requests.lock().unwrap().insert(serial.to_string(), totp::now_secs());
        self.note("-", &format!("запрос режима действий: {serial}"));
    }
    pub fn trust_pending(&self, serial: &str) -> bool {
        self.trust_requests.lock().unwrap().contains_key(serial)
    }
    /// Решение владельца: разрешить или отклонить запрос; также снимает доверие (ok=false).
    pub fn set_trust(&self, serial: &str, ok: bool) -> bool {
        self.trust_requests.lock().unwrap().remove(serial);
        let hit = {
            let mut d = self.devices.lock().unwrap();
            d.iter_mut().find(|x| x.serial == serial && !x.revoked).map(|x| x.trusted = ok).is_some()
        };
        if hit {
            self.note("-", &format!("режим действий {}: {serial}", if ok { "разрешён" } else { "снят/отклонён" }));
            self.persist_devices();
        }
        hit
    }
    pub fn set_control(&self, c: Arc<dyn Control>) {
        *self.control.lock().unwrap() = Some(c);
    }
    pub fn set_status(&self, f: Arc<dyn Fn() -> Value + Send + Sync>) {
        *self.status.lock().unwrap() = Some(f);
    }
    fn persist_devices(&self) {
        // снимок и запись под одной блокировкой: старый снимок не перезапишет новый (например, откат отзыва)
        let _g = self.persist_lock.lock().unwrap();
        let list = self.devices();
        self.hub.set_trusted(list.iter().filter(|d| d.trusted && !d.revoked).map(|d| d.serial.clone()).collect());
        if let Some(f) = self.on_devices.lock().unwrap().as_ref() {
            f(list);
        }
    }

    pub fn set_upstream(&self, u: Upstream) {
        *self.upstream.lock().unwrap() = Some(u);
    }

    pub fn open_window(&self, secs: u64) {
        self.window_until.store(totp::now_secs() + secs.min(WINDOW_SECS), Ordering::SeqCst);
        self.note("-", "окно привязки открыто");
    }
    pub fn close_window(&self) {
        self.window_until.store(0, Ordering::SeqCst);
    }
    pub fn window_open(&self) -> bool {
        totp::now_secs() < self.window_until.load(Ordering::SeqCst)
    }
    pub fn devices(&self) -> Vec<Device> {
        self.devices.lock().unwrap().clone()
    }
    pub fn log(&self) -> Vec<LogEntry> {
        self.log.lock().unwrap().iter().cloned().collect()
    }
    /// Отозвать устройство: новые соединения с его сертификатом сразу перестают приниматься.
    pub fn revoke(&self, serial: &str) -> bool {
        let mut d = self.devices.lock().unwrap();
        let hit = d.iter_mut().find(|x| x.serial == serial).map(|x| x.revoked = true).is_some();
        drop(d);
        if hit {
            self.note("-", &format!("устройство отозвано: {serial}"));
            self.persist_devices();
            self.hub.cancel_device(serial);
        }
        hit
    }

    fn note(&self, ip: &str, event: &str) {
        let mut l = self.log.lock().unwrap();
        if let Some(last) = l.back_mut() {
            let base = last.event.split(" ×").next().unwrap_or("").to_string();
            if base == event && last.ip == ip {
                let n = last.event.split(" ×").nth(1).and_then(|x| x.parse::<u32>().ok()).unwrap_or(1) + 1;
                last.event = format!("{event} ×{n}");
                last.ts = totp::now_secs();
                return;
            }
        }
        if l.len() >= LOG_KEEP {
            l.pop_front();
        }
        l.push_back(LogEntry { ts: totp::now_secs(), ip: ip.to_string(), event: event.to_string() });
    }

    fn ip_blocked(&self, ip: IpAddr) -> bool {
        self.ip_fails.lock().unwrap().get(&ip).map_or(false, |(_, until)| totp::now_secs() < *until)
    }
    fn ip_fail(&self, ip: IpAddr) {
        let mut m = self.ip_fails.lock().unwrap();
        let now = totp::now_secs();
        if m.len() > 1024 {
            m.retain(|_, (_, until)| *until > now); // не копим записи о прошлых адресах
        }
        let e = m.entry(ip).or_insert((0, 0));
        if now >= e.1 && e.1 != 0 {
            *e = (0, 0);
        }
        e.0 += 1;
        if e.0 >= IP_MAX_FAILS {
            *e = (0, now + IP_LOCK_SECS);
        }
    }
    fn device_by_fp(&self, fp: &str) -> Option<Device> {
        self.devices.lock().unwrap().iter().find(|d| d.fingerprint == fp).cloned()
    }
}

fn tls_config(core: &Core, server: &pki::Issued) -> Result<Arc<ServerConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = RootCertStore::empty();
    let ca_der = CertificateDer::from_pem_slice(core.ca.cert_pem.as_bytes()).map_err(|e| e.to_string())?;
    roots.add(ca_der).map_err(|e| e.to_string())?;
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .allow_unauthenticated()
        .build()
        .map_err(|e| e.to_string())?;
    let cert = CertificateDer::from_pem_slice(server.cert_pem.as_bytes()).map_err(|e| e.to_string())?;
    let key = PrivateKeyDer::from_pem_slice(server.key_pem.as_bytes()).map_err(|e| e.to_string())?;
    let cfg = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![cert], key)
        .map_err(|e| e.to_string())?;
    Ok(Arc::new(cfg))
}

pub struct Server {
    pub addr: SocketAddr,
    pub core: Arc<Core>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    pub fn start(core: Arc<Core>, server_cert: &pki::Issued, bind: SocketAddr) -> Result<Server, String> {
        let cfg = tls_config(&core, server_cert)?;
        let listener = TcpListener::bind(bind).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let addr = listener.local_addr().map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicUsize::new(0));
        let pre_auth = Arc::new(AtomicUsize::new(0));
        let per_ip: Arc<Mutex<HashMap<IpAddr, usize>>> = Arc::new(Mutex::new(HashMap::new()));
        let (c2, s2) = (core.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            while !s2.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((tcp, peer)) => {
                        if active.load(Ordering::SeqCst) >= MAX_CONN || c2.ip_blocked(peer.ip()) {
                            continue; // молча закрываем
                        }
                        {
                            let mut m = per_ip.lock().unwrap();
                            let e = m.entry(peer.ip()).or_insert(0);
                            if *e >= MAX_PER_IP || pre_auth.load(Ordering::SeqCst) >= MAX_PRE_AUTH {
                                continue; // молча закрываем
                            }
                            *e += 1;
                        }
                        c2.note(&peer.ip().to_string(), "входящее соединение");
                        active.fetch_add(1, Ordering::SeqCst);
                        pre_auth.fetch_add(1, Ordering::SeqCst);
                        let (core, cfg, active, per_ip2) = (c2.clone(), cfg.clone(), active.clone(), per_ip.clone());
                        let guard = PreGuard(pre_auth.clone(), false);
                        std::thread::spawn(move || {
                            let _ = tcp.set_nonblocking(false);
                            handle(&core, cfg, tcp, peer.ip(), guard);
                            active.fetch_sub(1, Ordering::SeqCst);
                            let mut m = per_ip2.lock().unwrap();
                            if let Some(e) = m.get_mut(&peer.ip()) {
                                *e = e.saturating_sub(1);
                                if *e == 0 {
                                    m.remove(&peer.ip());
                                }
                            }
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(40)),
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
        });
        Ok(Server { addr, core, stop, handle: Some(handle) })
    }

    /// Остановить слушатель: порт закрывается сразу, новые соединения не принимаются.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

type Tls = StreamOwned<ServerConnection, TcpStream>;

/// Место в малом пуле «ещё не опознанных» соединений; освобождается, когда устройство опознано или соединение закрыто.
struct PreGuard(Arc<AtomicUsize>, bool);
impl PreGuard {
    fn release(&mut self) {
        if !self.1 {
            self.1 = true;
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
}
impl Drop for PreGuard {
    fn drop(&mut self) {
        self.release();
    }
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

fn read_request(tls: &mut Tls, max_body: usize, deadline: Instant) -> Option<(String, String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = find(&buf, b"\r\n\r\n") {
            break p + 4;
        }
        if buf.len() > 16 * 1024 || Instant::now() > deadline {
            return None;
        }
        let n = tls.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut rl = lines.next()?.split_whitespace();
    let (method, path) = (rl.next()?.to_string(), rl.next()?.to_string());
    let cl = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    // большое тело (картинки) нужно только чату; остальным маршрутам хватает 128 КБ
    let cap = if path == "/v1/chat/completions" { max_body } else { max_body.min(MAX_SMALL_BODY) };
    if cl > cap {
        return None;
    }
    while buf.len() < head_end + cl {
        if Instant::now() > deadline {
            return None;
        }
        let n = tls.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    Some((method, path, buf[head_end..head_end + cl].to_vec()))
}

fn respond(tls: &mut Tls, code: u16, body: &Value) {
    let b = body.to_string();
    let reason = match code {
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        429 => "Too Many Requests",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Bad Request",
    };
    let msg = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len());
    let _ = tls.write_all(msg.as_bytes());
    let _ = tls.flush();
    tls.conn.send_close_notify();
    let _ = tls.conn.complete_io(&mut tls.sock);
}

fn handle(core: &Core, cfg: Arc<ServerConfig>, tcp: TcpStream, ip: IpAddr, mut pre: PreGuard) {
    let started = Instant::now();
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(READ_STALL_SECS)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(10)));
    let Ok(conn) = ServerConnection::new(cfg) else { return };
    let mut tls: Tls = StreamOwned::new(conn, tcp);
    let ips = ip.to_string();
    while tls.conn.is_handshaking() {
        if started.elapsed() > Duration::from_secs(HANDSHAKE_SECS) {
            core.note(&ips, "рукопожатие не завершено вовремя");
            return;
        }
        if tls.conn.complete_io(&mut tls.sock).is_err() {
            core.note(&ips, "рукопожатие отклонено (чужой или недействительный сертификат)");
            return;
        }
    }
    let who = tls.conn.peer_certificates().and_then(|c| c.first()).map(|c| {
        let fp: String = Sha256::digest(c.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
        core.device_by_fp(&fp)
    });
    match who {
        // сертификат предъявлен
        Some(Some(dev)) if !dev.revoked => {
            pre.release(); // устройство опознано: дальше оно занимает общий пул, а не малый пул неопознанных
            let Some((m, p, body)) = read_request(&mut tls, MAX_AUTH_BODY, Instant::now() + Duration::from_secs(DEVICE_REQUEST_SECS)) else { return };
            match (m.as_str(), p.as_str()) {
                ("GET", "/v1/models") => match core.current_upstream() {
                    Some((_, _, alias)) => respond(&mut tls, 200, &json!({"object": "list", "data": [{"id": alias, "object": "model"}]})),
                    None => respond(&mut tls, 503, &json!({"error": "model not running"})),
                },
                ("POST", "/v1/chat/completions") => chat_proxy(core, &mut tls, &ips, &dev.name, &body),
                ("POST", "/v1/devices/pair") => pair_device(core, &mut tls, &ips, ip, &dev, &body),
                ("GET", "/ping") => {
                    respond(&mut tls, 200, &json!({"ok": true, "device": dev.name}));
                }
                ("POST", "/bind-window") => {
                    let code = serde_json::from_slice::<Value>(&body).ok().and_then(|v| v["code"].as_str().map(String::from)).unwrap_or_default();
                    let verdict = core.approver.lock().unwrap().check(&code, totp::now_secs());
                    match verdict {
                        Verdict::Ok => {}
                        Verdict::Locked(_) => {
                            core.ip_fail(ip);
                            respond(&mut tls, 429, &json!({"error": "locked"}));
                            return;
                        }
                        Verdict::Wrong | Verdict::Replay => {
                            core.ip_fail(ip);
                            core.note(&ips, &format!("окно привязки: неверный код ({})", dev.name));
                            respond(&mut tls, 403, &json!({"error": "code"}));
                            return;
                        }
                    }
                    core.open_window(WINDOW_SECS);
                    core.note(&ips, &format!("окно привязки открыто с устройства {}", dev.name));
                    respond(&mut tls, 200, &json!({"ok": true, "seconds": WINDOW_SECS}));
                }
                _ => {
                    if !mobile_route(core, &mut tls, &ips, ip, &dev, &m, &p, &body) {
                        respond(&mut tls, 404, &json!({"error": "not found"}))
                    }
                }
            }
        }
        // сертификат предъявлен, но устройство неизвестно или отозвано: тишина
        Some(_) => core.note(&ips, "отказ: отозванное или неизвестное устройство"),
        // сертификата нет: можно только привязка и только при открытом окне
        None => {
            let (window, waiting) = (core.window_open(), core.has_pickups());
            if !window && !waiting {
                core.note(&ips, "отказ: нет сертификата, окно привязки закрыто");
                return;
            }
            let Some((m, p, body)) = read_request(&mut tls, MAX_BODY, started + Duration::from_secs(HANDSHAKE_SECS)) else { return };
            match (m.as_str(), p.as_str()) {
                ("POST", "/enroll") if window => enroll(core, &mut tls, &ips, ip, &body),
                ("POST", "/v1/pickup") if waiting => pickup(core, &mut tls, &ips, &body),
                _ => {}
            }
        }
    }
}

/// Маршруты для мобильного клиента (только с сертификатом устройства).
/// GET /v1/status, GET/POST /v1/tasks, GET /v1/tasks/{id}?after=N&wait=S, POST /v1/tasks/{id}/cancel|retry,
/// POST /v1/approvals, POST /v1/devices/self/revoke. Возвращает false, если маршрут не его.
fn mobile_route(core: &Core, tls: &mut Tls, ips: &str, ip: IpAddr, dev: &Device, m: &str, path: &str, body: &[u8]) -> bool {
    let (p, query) = path.split_once('?').unwrap_or((path, ""));
    let q = |k: &str| query.split('&').filter_map(|kv| kv.split_once('=')).find(|(a, _)| *a == k).map(|(_, v)| v.to_string());
    let seg: Vec<&str> = p.trim_matches('/').split('/').collect();
    let hub = &core.hub;
    let parse = || serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
    match (m, seg.as_slice()) {
        ("GET", ["v1", "status"]) => {
            let mut v = core.status.lock().unwrap().clone().map_or(json!({}), |f| f());
            v["active_tasks"] = json!(hub.active_count());
            v["device"] = json!(dev.name);
            v["trusted"] = json!(dev.trusted);
            v["trust_pending"] = json!(core.trust_pending(&dev.serial));
            respond(tls, 200, &v);
        }
        ("GET", ["v1", "app", "models"]) => match core.control.lock().unwrap().clone() {
            Some(c) => respond(tls, 200, &c.models()),
            None => respond(tls, 404, &json!({"error": "not found"})),
        },
        // выбор модели в приложении на ПК: перезапускает движок (до пары минут), поэтому только когда нет активных задач
        ("POST", ["v1", "app", "model"]) => {
            let Some(c) = core.control.lock().unwrap().clone() else {
                respond(tls, 404, &json!({"error": "not found"}));
                return true;
            };
            if hub.active_count() > 0 {
                respond(tls, 409, &json!({"error": "busy"}));
                return true;
            }
            let id = parse()["id"].as_str().unwrap_or("").to_string();
            match c.select(&id) {
                Ok(()) => {
                    core.note(ips, &format!("модель выбрана с телефона ({}): {id}", dev.name));
                    respond(tls, 200, &json!({"ok": true}))
                }
                Err(e) => respond(tls, 400, &json!({"error": e})),
            }
        }
        // запрос «режима действий» с телефона; одобряется только на ПК
        ("POST", ["v1", "trust", "request"]) => {
            if !dev.trusted {
                core.request_trust(&dev.serial);
            }
            respond(tls, 200, &json!({"ok": true}))
        }
        // телефон может сам отказаться от режима действий
        ("POST", ["v1", "trust", "revoke"]) => {
            core.set_trust(&dev.serial, false);
            respond(tls, 200, &json!({"ok": true}))
        }
        ("GET", ["v1", "tasks"]) => respond(tls, 200, &json!({"tasks": hub.list()})),
        ("POST", ["v1", "tasks"]) => {
            let v = parse();
            let hist: Vec<(String, String)> = v["history"].as_array().map(|a| a.iter().map(|m| (m["role"].as_str().unwrap_or("").to_string(), m["content"].as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
            match hub.create_with(v["prompt"].as_str().unwrap_or(""), v["title"].as_str().unwrap_or(""), &dev.name, &dev.serial, hist, v["mode"].as_str() == Some("fast")) {
                Ok(id) => {
                    core.note(ips, &format!("задача #{id} от {}", dev.name));
                    respond(tls, 200, &json!({"id": id}))
                }
                Err(e) => respond(tls, 400, &json!({"error": e})),
            }
        }
        ("GET", ["v1", "tasks", id]) => {
            let Ok(id) = id.parse::<u64>() else { return false };
            let after = q("after").and_then(|v| v.parse().ok()).unwrap_or(0);
            let wait = q("wait").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).min(25);
            match hub.poll(id, after, Duration::from_secs(wait)) {
                Some(v) => respond(tls, 200, &v),
                None => respond(tls, 404, &json!({"error": "not found"})),
            }
        }
        ("POST", ["v1", "tasks", id, "cancel"]) => {
            let Ok(id) = id.parse::<u64>() else { return false };
            if hub.cancel(id) {
                core.note(ips, &format!("задача #{id} остановлена с {}", dev.name));
                respond(tls, 200, &json!({"ok": true}))
            } else {
                respond(tls, 404, &json!({"error": "not found"}))
            }
        }
        ("POST", ["v1", "tasks", id, "retry"]) => {
            let Ok(id) = id.parse::<u64>() else { return false };
            match hub.prompt_of(id) {
                Some((title, prompt)) => match hub.create_with(&prompt, &title, &dev.name, &dev.serial, vec![], hub.fast_of(id)) {
                    Ok(n) => respond(tls, 200, &json!({"id": n})),
                    Err(e) => respond(tls, 400, &json!({"error": e})),
                },
                None => respond(tls, 404, &json!({"error": "not found"})),
            }
        }
        // подтверждение действия агента. Опасные (удаление, права администратора, боевой сервер) требуют свежий код Authenticator.
        ("POST", ["v1", "approvals"]) => {
            let v = parse();
            let (Some(job), Some(aid), Some(ok)) = (v["task"].as_u64(), v["approval"].as_u64(), v["ok"].as_bool()) else {
                respond(tls, 400, &json!({"error": "bad request"}));
                return true;
            };
            match hub.pending_of(job) {
                Some((pid, dangerous)) if pid == aid => {
                    if ok && dangerous {
                        let code = v["code"].as_str().unwrap_or("");
                        let verdict = core.approver.lock().unwrap().check(code, totp::now_secs());
                        match verdict {
                            Verdict::Ok => {}
                            Verdict::Locked(_) => {
                                core.ip_fail(ip);
                                core.note(ips, &format!("опасное подтверждение: блокировка после неверных кодов ({})", dev.name));
                                respond(tls, 429, &json!({"error": "locked"}));
                                return true;
                            }
                            Verdict::Wrong | Verdict::Replay => {
                                core.ip_fail(ip);
                                core.note(ips, &format!("опасное подтверждение: неверный код ({})", dev.name));
                                respond(tls, 403, &json!({"error": "code"}));
                                return true;
                            }
                        }
                    }
                }
                _ => {
                    respond(tls, 409, &json!({"error": "closed"}));
                    return true;
                }
            }
            match hub.resolve(job, aid, ok) {
                Ok(()) => {
                    core.note(ips, &format!("подтверждение #{aid}: {} ({})", if ok { "разрешено" } else { "отклонено" }, dev.name));
                    respond(tls, 200, &json!({"ok": true}))
                }
                Err(_) => respond(tls, 409, &json!({"error": "closed"})),
            }
        }
        // устройство может отозвать само себя (выход); чужие отзывает только владелец на сервере
        ("POST", ["v1", "devices", "self", "revoke"]) => {
            core.revoke(&dev.serial);
            respond(tls, 200, &json!({"ok": true}))
        }
        _ => return false,
    }
    true
}

impl Core {
    fn current_upstream(&self) -> Option<(String, String, String)> {
        let f = self.upstream.lock().unwrap().clone()?;
        f()
    }
}

/// Переслать запрос чата движку и отдать поток ответа клиенту как есть (SSE).
/// Клиент может передать только разрешённые поля; модель подставляет сервер.
fn chat_proxy(core: &Core, tls: &mut Tls, ips: &str, dev: &str, body: &[u8]) {
    let Ok(Value::Object(src)) = serde_json::from_slice::<Value>(body) else {
        return respond(tls, 400, &json!({"error": "bad request"}));
    };
    let Some((base, token, alias)) = core.current_upstream() else {
        return respond(tls, 503, &json!({"error": "model not running"}));
    };
    if core.chats.fetch_add(1, Ordering::SeqCst) >= MAX_CHATS {
        core.chats.fetch_sub(1, Ordering::SeqCst);
        return respond(tls, 429, &json!({"error": "busy"}));
    }
    let mut out = serde_json::Map::new();
    for k in CHAT_FIELDS {
        if let Some(v) = src.get(*k) {
            out.insert((*k).to_string(), v.clone());
        }
    }
    out.insert("model".into(), Value::String(alias));
    out.entry("stream").or_insert(Value::Bool(true));
    core.note(ips, &format!("чат: {dev}"));
    let sent = ureq::post(&format!("{base}/v1/chat/completions"))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(1800))
        .send_json(Value::Object(out));
    match sent {
        Ok(resp) => {
            let ct = resp.content_type().to_string();
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n");
            let mut ok = tls.write_all(head.as_bytes()).is_ok();
            let mut r = resp.into_reader();
            let mut buf = [0u8; 4096];
            while ok {
                match r.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    // обрыв клиента: выходим, соединение с движком закроется и генерация остановится
                    Ok(n) => ok = tls.write_all(&buf[..n]).and_then(|_| tls.flush()).is_ok(),
                }
            }
            tls.conn.send_close_notify();
            let _ = tls.conn.complete_io(&mut tls.sock);
        }
        Err(_) => respond(tls, 502, &json!({"error": "engine error"})),
    }
    core.chats.fetch_sub(1, Ordering::SeqCst);
}

/// Имя устройства для показа: без управляющих, невидимых и переключающих направление текста символов, не длиннее 64.
fn clean_name(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && !matches!(*c as u32, 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x2069 | 0xFEFF))
        .take(64)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Хеш одноразового номера (hex SHA-256): по нему лежит сертификат на выдаче, сам номер на сервере не хранится.
fn nonce_hash(nonce: &str) -> String {
    Sha256::digest(nonce.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn valid_nonce(n: &str) -> bool {
    n.len() == 32 && n.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Код сверки для экранов нового и подтверждающего устройств: первые 8 hex SHA-256 от номера, «ab12-cd34».
pub fn pair_check_code(nonce: &str) -> String {
    let h = nonce_hash(nonce);
    format!("{}-{}", &h[..4], &h[4..8])
}

/// Запрос на сертификат из тела: PEM в поле csr или DER в base64 в поле csr_b64 (так короче для QR).
fn csr_pem_from(v: &Value) -> String {
    if let Some(p) = v["csr"].as_str() {
        return p.to_string();
    }
    let b = v["csr_b64"].as_str().unwrap_or("");
    let body: Vec<String> = b.as_bytes().chunks(64).map(|c| String::from_utf8_lossy(c).to_string()).collect();
    format!("-----BEGIN CERTIFICATE REQUEST-----\n{}\n-----END CERTIFICATE REQUEST-----\n", body.join("\n"))
}

/// POST /v1/devices/pair: привязанное устройство подтверждает новое (его запрос на сертификат пришёл по QR).
/// Нужен код Authenticator, как для открытия окна привязки: потерянный телефон один устройств не добавит.
fn pair_device(core: &Core, tls: &mut Tls, ips: &str, ip: IpAddr, dev: &Device, body: &[u8]) {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let nonce = v["nonce"].as_str().unwrap_or("");
    let name = clean_name(v["name"].as_str().unwrap_or(""));
    if !valid_nonce(nonce) || name.is_empty() {
        return respond(tls, 400, &json!({"error": "bad request"}));
    }
    match core.approver.lock().unwrap().check(v["code"].as_str().unwrap_or(""), totp::now_secs()) {
        Verdict::Ok => {}
        Verdict::Locked(_) => {
            core.ip_fail(ip);
            return respond(tls, 429, &json!({"error": "locked"}));
        }
        Verdict::Wrong | Verdict::Replay => {
            core.ip_fail(ip);
            core.note(ips, &format!("подтверждение устройства: неверный код ({})", dev.name));
            return respond(tls, 403, &json!({"error": "code", "time": totp::now_secs()}));
        }
    }
    if !core.pickup_room() {
        return respond(tls, 429, &json!({"error": "too many pending"}));
    }
    match core.ca.sign_device_csr(&csr_pem_from(&v), &name) {
        Ok((cert_pem, serial, fingerprint)) => {
            core.devices.lock().unwrap().push(Device { serial: serial.clone(), name: name.clone(), fingerprint, added: totp::now_secs(), revoked: false, trusted: false });
            core.persist_devices();
            core.put_pickup(nonce_hash(nonce), cert_pem, serial.clone());
            core.note(ips, &format!("устройство «{name}» добавлено по подтверждению с «{}»", dev.name));
            respond(tls, 200, &json!({"ok": true, "serial": serial, "check": pair_check_code(nonce)}));
        }
        Err(_) => respond(tls, 400, &json!({"error": "csr"})),
    }
}

/// POST /v1/pickup без сертификата: новое устройство забирает выданный сертификат по своему одноразовому номеру.
fn pickup(core: &Core, tls: &mut Tls, ips: &str, body: &[u8]) {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let nonce = v["nonce"].as_str().unwrap_or("");
    if !valid_nonce(nonce) {
        return respond(tls, 400, &json!({"error": "bad request"}));
    }
    match core.take_pickup(&nonce_hash(nonce)) {
        Some((cert, serial)) => {
            core.note(ips, "новое устройство забрало сертификат");
            respond(tls, 200, &json!({"cert": cert, "ca": core.ca.cert_pem, "serial": serial}));
        }
        None => respond(tls, 404, &json!({"error": "pending"})), // не промахи по IP не считаем: ожидание подтверждения опрашивается часто
    }
}

fn enroll(core: &Core, tls: &mut Tls, ips: &str, ip: IpAddr, body: &[u8]) {
    // время сервера помогает телефону объяснить причину отказа (расхождение часов); ничего секретного в нём нет
    let denied = |tls: &mut Tls| respond(tls, 403, &json!({"error": "denied", "time": totp::now_secs()}));
    let Ok(v) = serde_json::from_slice::<Value>(body) else { return denied(tls) };
    let (code, csr) = (v["code"].as_str().unwrap_or(""), v["csr"].as_str().unwrap_or(""));
    let name_owned = clean_name(v["name"].as_str().unwrap_or(""));
    let name = name_owned.as_str();
    let verdict = core.verifier.lock().unwrap().check(code, totp::now_secs());
    match verdict {
        Verdict::Ok => {}
        Verdict::Locked(_) => {
            core.ip_fail(ip);
            core.note(ips, "привязка: заблокирована после неверных кодов");
            return respond(tls, 429, &json!({"error": "locked"}));
        }
        Verdict::Wrong | Verdict::Replay => {
            core.ip_fail(ip);
            core.note(ips, "привязка: неверный код");
            return denied(tls);
        }
    }
    match core.ca.sign_device_csr(csr, name) {
        Ok((cert_pem, serial, fingerprint)) => {
            core.devices.lock().unwrap().push(Device {
                serial: serial.clone(),
                name: name.to_string(),
                fingerprint,
                added: totp::now_secs(),
                revoked: false,
                trusted: false,
            });
            core.persist_devices();
            core.close_window(); // одна привязка на одно открытие окна
            core.note(ips, &format!("привязано устройство: {name}"));
            respond(tls, 200, &json!({"cert": cert_pem, "ca": core.ca.cert_pem, "serial": serial}));
        }
        Err(_) => {
            core.note(ips, "привязка: неверный запрос устройства");
            denied(tls)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::ServerName;
    use rustls::{ClientConfig, ClientConnection};

    struct Fx {
        server: Server,
        secret: Vec<u8>,
        ca_pem: String,
    }

    fn fx() -> Fx {
        let ca = Ca::create("Test").unwrap();
        let ca_pem = ca.cert_pem.clone();
        let srv = ca.issue_server(&["127.0.0.1".into()]).unwrap();
        let secret_b32 = totp::new_secret().unwrap();
        let secret = totp::base32_decode(&secret_b32).unwrap();
        let core = Arc::new(Core::new(ca, &secret_b32, vec![]).unwrap());
        let server = Server::start(core, &srv, "127.0.0.1:0".parse().unwrap()).unwrap();
        Fx { server, secret, ca_pem }
    }

    /// Запрос клиента. None = соединение закрыто без ответа.
    fn req(f: &Fx, id: Option<(&str, &str)>, method: &str, path: &str, body: &str) -> Option<(u16, Value)> {
        let (code, text) = req_raw(f, id, method, path, body)?;
        Some((code, serde_json::from_str(&text).unwrap_or(Value::Null)))
    }

    fn req_raw(f: &Fx, id: Option<(&str, &str)>, method: &str, path: &str, body: &str) -> Option<(u16, String)> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from_pem_slice(f.ca_pem.as_bytes()).unwrap()).unwrap();
        let b = ClientConfig::builder_with_provider(provider).with_protocol_versions(&[&rustls::version::TLS13]).unwrap().with_root_certificates(roots);
        let cfg = match id {
            Some((c, k)) => b
                .with_client_auth_cert(vec![CertificateDer::from_pem_slice(c.as_bytes()).unwrap()], PrivateKeyDer::from_pem_slice(k.as_bytes()).unwrap())
                .unwrap(),
            None => b.with_no_client_auth(),
        };
        let tcp = TcpStream::connect(f.server.addr).ok()?;
        tcp.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        let name = ServerName::IpAddress("127.0.0.1".parse::<IpAddr>().unwrap().into());
        let mut tls = StreamOwned::new(ClientConnection::new(Arc::new(cfg), name).ok()?, tcp);
        let msg = format!("{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        tls.write_all(msg.as_bytes()).ok()?;
        let mut out = Vec::new();
        let _ = tls.read_to_end(&mut out);
        let s = String::from_utf8_lossy(&out).to_string();
        let code: u16 = s.split_whitespace().nth(1)?.parse().ok()?;
        let body = s.split_once("\r\n\r\n").map(|x| x.1).unwrap_or("");
        Some((code, body.to_string()))
    }

    /// Поддельный движок: принимает один запрос, запоминает его, отвечает потоком SSE.
    fn fake_engine() -> (String, Arc<Mutex<(String, String)>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let seen = Arc::new(Mutex::new((String::new(), String::new())));
        let s2 = seen.clone();
        std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            loop {
                let n = c.read(&mut tmp).unwrap();
                buf.extend_from_slice(&tmp[..n]);
                if let Some(p) = find(&buf, b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..p]).to_string();
                    let cl: usize = head.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                    if buf.len() >= p + 4 + cl {
                        *s2.lock().unwrap() = (head, String::from_utf8_lossy(&buf[p + 4..p + 4 + cl]).to_string());
                        break;
                    }
                }
            }
            let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"при\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"вет\"}}]}\n\ndata: [DONE]\n\n";
            let _ = c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{sse}").as_bytes());
        });
        (base, seen)
    }

    fn bound(f: &Fx) -> (String, String) {
        f.server.core.open_window(60);
        let (body, key) = enroll_body(f, "Ноутбук", 0);
        let (_, v) = req(f, None, "POST", "/enroll", &body).unwrap();
        (v["cert"].as_str().unwrap().to_string(), key)
    }

    #[test]
    fn chat_proxy_streams_and_filters_fields() {
        let f = fx();
        let (cert, key) = bound(&f);
        // модель не запущена
        let msg = r#"{"messages":[{"role":"user","content":"hi"}]}"#;
        assert_eq!(req(&f, Some((&cert, &key)), "POST", "/v1/chat/completions", msg).unwrap().0, 503);
        let (base, seen) = fake_engine();
        f.server.core.set_upstream(Arc::new(move || Some((base.clone(), "tok".to_string(), "qwen".to_string()))));
        assert_eq!(req(&f, Some((&cert, &key)), "GET", "/v1/models", "").unwrap().1["data"][0]["id"], "qwen");
        let evil = r#"{"messages":[{"role":"user","content":"hi"}],"model":"other","temperature":0.2,"n_predict":99999,"grammar":"x","system_prompt":"hack"}"#;
        let (code, text) = req_raw(&f, Some((&cert, &key)), "POST", "/v1/chat/completions", evil).unwrap();
        assert_eq!(code, 200);
        assert!(text.contains("\"при\"") && text.contains("[DONE]"), "{text}");
        let (head, body) = seen.lock().unwrap().clone();
        assert!(head.to_lowercase().contains("authorization: bearer tok"), "{head}");
        let b: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(b["model"], "qwen", "модель подставляет сервер");
        assert_eq!(b["temperature"], 0.2);
        assert_eq!(b["stream"], true);
        assert!(b.get("n_predict").is_none() && b.get("grammar").is_none() && b.get("system_prompt").is_none(), "лишние поля отброшены: {body}");
    }

    #[test]
    fn chat_requires_certificate_and_valid_json() {
        let f = fx();
        f.server.core.set_upstream(Arc::new(|| Some(("http://127.0.0.1:1".into(), "t".into(), "m".into()))));
        assert!(req(&f, None, "POST", "/v1/chat/completions", "{}").is_none(), "без сертификата тишина");
        let (cert, key) = bound(&f);
        assert_eq!(req(&f, Some((&cert, &key)), "POST", "/v1/chat/completions", "не json").unwrap().0, 400);
        // движок недоступен -> 502, сервер жив
        assert_eq!(req(&f, Some((&cert, &key)), "POST", "/v1/chat/completions", r#"{"messages":[]}"#).unwrap().0, 502);
        assert_eq!(req(&f, Some((&cert, &key)), "GET", "/ping", "").unwrap().0, 200);
    }

    fn csr() -> (String, String) {
        let key = rcgen::KeyPair::generate().unwrap();
        let p = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        (p.serialize_request(&key).unwrap().pem().unwrap(), key.serialize_pem())
    }

    fn code(f: &Fx, step_off: i64) -> String {
        totp::code_at(&f.secret, (totp::now_secs() as i64 / 30 + step_off) as u64)
    }

    fn enroll_body(f: &Fx, name: &str, step: i64) -> (String, String) {
        let (csr_pem, key) = csr();
        (json!({"code": code(f, step), "name": name, "csr": csr_pem}).to_string(), key)
    }

    #[test]
    fn full_flow_enroll_ping_revoke() {
        let f = fx();
        f.server.core.open_window(60);
        let (body, key) = enroll_body(&f, "Телефон", 0);
        let (c, v) = req(&f, None, "POST", "/enroll", &body).expect("ответ на привязку");
        assert_eq!(c, 200, "{v}");
        let cert = v["cert"].as_str().unwrap().to_string();
        let serial = v["serial"].as_str().unwrap().to_string();
        assert!(!f.server.core.window_open(), "окно закрылось после привязки");
        // вход по сертификату
        let (c, v) = req(&f, Some((&cert, &key)), "GET", "/ping", "").expect("ping");
        assert_eq!((c, v["device"].as_str()), (200, Some("Телефон")));
        // устройство с сертификатом открывает окно для следующего
        assert_eq!(req(&f, Some((&cert, &key)), "POST", "/bind-window", "").unwrap().0, 403, "без кода окно не открывается");
        assert!(!f.server.core.window_open());
        assert_eq!(req(&f, Some((&cert, &key)), "POST", "/bind-window", &json!({"code": code(&f, 1)}).to_string()).unwrap().0, 200);
        assert!(f.server.core.window_open());
        // отзыв: молчание
        assert!(f.server.core.revoke(&serial));
        assert!(req(&f, Some((&cert, &key)), "GET", "/ping", "").is_none(), "отозванное устройство не получает ответа");
        assert!(f.server.core.log().iter().any(|l| l.event.contains("отозван")));
    }

    #[test]
    fn mobile_tasks_approvals_and_self_revoke() {
        let f = fx();
        let hub = f.server.core.hub();
        hub.start(Arc::new(|h, id, prompt, _| {
            let (aid, rx) = h.approvals.create();
            let kind = if prompt.contains("удали") { "delete" } else { "write" };
            h.emit(id, "approval", json!({"id": aid, "kind": kind, "title": "действие", "detail": "d"}));
            if rx.recv_timeout(Duration::from_secs(10)).unwrap_or(false) { Ok("сделано".into()) } else { Err("отклонено".into()) }
        }));
        f.server.core.open_window(60);
        let (body, key) = enroll_body(&f, "Телефон", 0);
        let (_, v) = req(&f, None, "POST", "/enroll", &body).unwrap();
        let cert = v["cert"].as_str().unwrap().to_string();
        let id_ = Some((cert.as_str(), key.as_str()));
        // без сертификата задач нет
        assert!(req(&f, None, "POST", "/v1/tasks", "{}").is_none());
        let (c, st) = req(&f, id_, "GET", "/v1/status", "").unwrap();
        assert_eq!((c, st["device"].as_str(), st["active_tasks"].as_u64()), (200, Some("Телефон"), Some(0)));
        assert_eq!(req(&f, id_, "POST", "/v1/tasks", r#"{"prompt":"  "}"#).unwrap().0, 400);

        let wait_status = |id: u64, want: &str| {
            for _ in 0..50 {
                let (_, v) = req(&f, id_, "GET", &format!("/v1/tasks/{id}?after=0&wait=1"), "").unwrap();
                if v["status"] == want {
                    return v;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            panic!("не дождались {want}");
        };

        // опасное действие: без кода отказ, с кодом проходит, процентов нет
        let (c, v) = req(&f, id_, "POST", "/v1/tasks", r#"{"prompt":"удали файл"}"#).unwrap();
        assert_eq!(c, 200);
        let t1 = v["id"].as_u64().unwrap();
        let w = wait_status(t1, "waiting");
        assert!(w["progress"].is_null());
        let aid = w["pending"]["id"].as_u64().unwrap();
        let ap = |ok: bool, code: &str| json!({"task": t1, "approval": aid, "ok": ok, "code": code}).to_string();
        assert_eq!(req(&f, id_, "POST", "/v1/approvals", &ap(true, "")).unwrap().0, 403, "опасное без кода");
        assert_eq!(req(&f, id_, "POST", "/v1/approvals", &ap(true, "000000")).unwrap().0, 403, "неверный код");
        assert_eq!(req(&f, id_, "POST", "/v1/approvals", &json!({"task": t1, "approval": aid + 5, "ok": true}).to_string()).unwrap().0, 409);
        assert_eq!(req(&f, id_, "POST", "/v1/approvals", &ap(true, &code(&f, 1))).unwrap().0, 200);
        assert_eq!(wait_status(t1, "done")["result"], "сделано");
        assert_eq!(req(&f, id_, "POST", "/v1/approvals", &ap(true, &code(&f, 1))).unwrap().0, 409, "уже закрыто");

        // обычное действие: подтверждение без кода; отказ возможен без кода даже для опасного
        let (_, v) = req(&f, id_, "POST", "/v1/tasks", r#"{"prompt":"запиши"}"#).unwrap();
        let t2 = v["id"].as_u64().unwrap();
        let aid2 = wait_status(t2, "waiting")["pending"]["id"].as_u64().unwrap();
        assert_eq!(req(&f, id_, "POST", "/v1/approvals", &json!({"task": t2, "approval": aid2, "ok": false}).to_string()).unwrap().0, 200);
        assert_eq!(wait_status(t2, "error")["error"], "отклонено");

        // повтор и список
        let (_, v) = req(&f, id_, "POST", &format!("/v1/tasks/{t2}/retry"), "").unwrap();
        let t3 = v["id"].as_u64().unwrap();
        assert_ne!(t3, t2);
        assert_eq!(req(&f, id_, "POST", &format!("/v1/tasks/{t3}/cancel"), "").unwrap().0, 200);
        wait_status(t3, "cancelled");
        let (_, l) = req(&f, id_, "GET", "/v1/tasks", "").unwrap();
        assert_eq!(l["tasks"].as_array().unwrap().len(), 3);
        assert_eq!(req(&f, id_, "GET", "/v1/tasks/999", "").unwrap().0, 404);

        // выход: устройство отзывает само себя
        assert_eq!(req(&f, id_, "POST", "/v1/devices/self/revoke", "").unwrap().0, 200);
        assert!(req(&f, id_, "GET", "/ping", "").is_none());
        assert!(f.server.core.devices().iter().all(|d| d.revoked));
    }

    #[test]
    fn idle_connections_cannot_starve_the_pool() {
        let f = fx();
        f.server.core.open_window(60);
        let (body, key) = enroll_body(&f, "Телефон", 0);
        let (_, v) = req(&f, None, "POST", "/enroll", &body).unwrap();
        let cert = v["cert"].as_str().unwrap().to_string();
        // один хост держит максимум соединений и ничего не шлёт
        let hold: Vec<TcpStream> = (0..MAX_PER_IP).filter_map(|_| TcpStream::connect(f.server.addr).ok()).collect();
        std::thread::sleep(Duration::from_millis(300));
        assert!(req(&f, Some((&cert, &key)), "GET", "/ping", "").is_none(), "лимит на IP: лишнее соединение закрыто");
        drop(hold);
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(req(&f, Some((&cert, &key)), "GET", "/ping", "").unwrap().0, 200, "после закрытия место освободилось");
    }

    #[test]
    fn big_body_only_for_chat() {
        let f = fx();
        f.server.core.open_window(60);
        let (body, key) = enroll_body(&f, "Телефон", 0);
        let (_, v) = req(&f, None, "POST", "/enroll", &body).unwrap();
        let cert = v["cert"].as_str().unwrap().to_string();
        let big = format!(r#"{{"prompt":"{}"}}"#, "я".repeat(200 * 1024));
        assert!(req(&f, Some((&cert, &key)), "POST", "/v1/tasks", &big).is_none(), "для задач тело больше 128 КБ не читается");
    }

    /// Сервер после перезапуска грузит CA из PEM, а CSR приходит от Android (не от rcgen): вход по сертификату должен работать.
    #[test]
    fn restarted_ca_and_android_csr() {
        let ca0 = Ca::create("Test").unwrap();
        let ca = Ca::from_pem(&ca0.cert_pem, ca0.key_pem()).unwrap();
        let ca_pem = ca0.cert_pem.clone();
        let srv = ca.issue_server(&["127.0.0.1".into()]).unwrap();
        let secret_b32 = totp::new_secret().unwrap();
        let secret = totp::base32_decode(&secret_b32).unwrap();
        let core = Arc::new(Core::new(ca, &secret_b32, vec![]).unwrap());
        let server = Server::start(core, &srv, "127.0.0.1:0".parse().unwrap()).unwrap();
        let f = Fx { server, secret, ca_pem };
        f.server.core.open_window(60);
        let csr_pem = include_str!("../src/android_csr_fixture.pem");
        let body = json!({"code": code(&f, 0), "name": "Pixel", "csr": csr_pem}).to_string();
        let (c, v) = req(&f, None, "POST", "/enroll", &body).expect("ответ на привязку");
        assert_eq!(c, 200, "{v}");
        assert!(pki::fingerprint_pem(v["cert"].as_str().unwrap()).is_ok());
        // полный вход по сертификату после перезапуска (CA из PEM)
        f.server.core.open_window(60);
        let (body2, key2) = enroll_body(&f, "Второй", 1);
        let (c2, v2) = req(&f, None, "POST", "/enroll", &body2).expect("привязка 2");
        assert_eq!(c2, 200, "{v2}");
        let cert2 = v2["cert"].as_str().unwrap().to_string();
        let (c3, v3) = req(&f, Some((&cert2, &key2)), "GET", "/ping", "").expect("вход по сертификату после перезапуска CA");
        assert_eq!((c3, v3["device"].as_str()), (200, Some("Второй")));
    }

    #[test]
    fn trusted_device_skips_ordinary_approvals_but_not_destructive_ones() {
        let f = fx();
        let hub = f.server.core.hub();
        hub.start(Arc::new(|h, id, prompt, _| {
            let (aid, rx) = h.approvals.create();
            let kind = if prompt.contains("удали") { "delete" } else { "run" };
            h.emit(id, "approval", json!({"id": aid, "kind": kind, "title": "действие", "detail": "d"}));
            if rx.recv_timeout(Duration::from_secs(10)).unwrap_or(false) { Ok("сделано".into()) } else { Err("отклонено".into()) }
        }));
        f.server.core.open_window(60);
        let (body, key) = enroll_body(&f, "Телефон", 0);
        let (_, v) = req(&f, None, "POST", "/enroll", &body).unwrap();
        let cert = v["cert"].as_str().unwrap().to_string();
        let serial = v["serial"].as_str().unwrap().to_string();
        let id_ = Some((cert.as_str(), key.as_str()));
        let status = || req(&f, id_, "GET", "/v1/status", "").unwrap().1;
        let wait = |id: u64, want: &str| {
            for _ in 0..60 {
                let (_, v) = req(&f, id_, "GET", &format!("/v1/tasks/{id}?after=0&wait=1"), "").unwrap();
                if v["status"] == want {
                    return v;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            panic!("не дождались {want}");
        };
        assert_eq!((status()["trusted"].as_bool(), status()["trust_pending"].as_bool()), (Some(false), Some(false)));
        // без доверия команда ждёт подтверждения
        let t0 = req(&f, id_, "POST", "/v1/tasks", r#"{"prompt":"команда"}"#).unwrap().1["id"].as_u64().unwrap();
        wait(t0, "waiting");
        req(&f, id_, "POST", &format!("/v1/tasks/{t0}/cancel"), "");
        // запрос режима действий: пока владелец не одобрил, ничего не меняется
        assert_eq!(req(&f, id_, "POST", "/v1/trust/request", "").unwrap().0, 200);
        assert_eq!((status()["trusted"].as_bool(), status()["trust_pending"].as_bool()), (Some(false), Some(true)));
        // решение владельца на ПК
        assert!(f.server.core.set_trust(&serial, true));
        assert_eq!((status()["trusted"].as_bool(), status()["trust_pending"].as_bool()), (Some(true), Some(false)));
        let t1 = req(&f, id_, "POST", "/v1/tasks", r#"{"prompt":"команда"}"#).unwrap().1["id"].as_u64().unwrap();
        assert_eq!(wait(t1, "done")["result"], "сделано", "обычное действие доверенного устройства проходит без вопросов");
        // удаление спрашивается всегда
        let t2 = req(&f, id_, "POST", "/v1/tasks", r#"{"prompt":"удали файл"}"#).unwrap().1["id"].as_u64().unwrap();
        assert_eq!(wait(t2, "waiting")["pending"]["dangerous"], true);
        req(&f, id_, "POST", &format!("/v1/tasks/{t2}/cancel"), "");
        // доверие можно снять с телефона
        assert_eq!(req(&f, id_, "POST", "/v1/trust/revoke", "").unwrap().0, 200);
        assert_eq!(status()["trusted"].as_bool(), Some(false));
        // после отзыва устройства доверие не действует
        assert!(f.server.core.set_trust(&serial, true));
        assert!(f.server.core.devices().iter().any(|d| d.trusted));
        f.server.core.revoke(&serial);
        let t3 = hub.create_with("команда", "", "x", &serial, vec![], false).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(hub.pending_of(t3).map(|p| p.1), Some(true), "после отзыва доверие не действует");
    }

    #[test]
    fn lockouts_are_separate_and_names_and_log_are_clean() {
        let f = fx();
        let now = totp::now_secs();
        for _ in 0..6 {
            f.server.core.verifier.lock().unwrap().check("000000", now);
        }
        assert!(matches!(f.server.core.verifier.lock().unwrap().check(&code(&f, 0), now), Verdict::Locked(_)), "привязка заблокирована");
        assert_eq!(f.server.core.approver.lock().unwrap().check(&code(&f, 0), now), Verdict::Ok, "подтверждения при этом работают");
        assert_eq!(clean_name("  Те\u{202E}ле\u{200B}фон\n  "), "Телефон");
        assert_eq!(clean_name(&"я".repeat(200)).chars().count(), 64);
        // одинаковые подряд записи схлопываются
        for _ in 0..50 {
            f.server.core.note("1.2.3.4", "рукопожатие отклонено");
        }
        let log = f.server.core.log();
        assert_eq!(log.len(), 1);
        assert!(log[0].event.ends_with("×50"), "{}", log[0].event);
    }

    #[test]
    fn closed_window_and_no_cert_get_nothing() {
        let f = fx();
        let (body, _) = enroll_body(&f, "X", 0);
        assert!(req(&f, None, "POST", "/enroll", &body).is_none(), "окно закрыто: тишина");
        f.server.core.open_window(60);
        assert!(req(&f, None, "GET", "/ping", "").is_none(), "без сертификата доступна только /enroll");
        assert!(req(&f, None, "GET", "/", "").is_none());
    }

    #[test]
    fn wrong_codes_lock_and_replay_rejected() {
        let f = fx();
        f.server.core.open_window(60);
        let (csr_pem, _) = csr();
        let bad = json!({"code": "000000", "name": "x", "csr": csr_pem}).to_string();
        for _ in 0..3 {
            let (c, v) = req(&f, None, "POST", "/enroll", &bad).unwrap();
            assert_eq!((c, v["error"].as_str()), (403, Some("denied")));
        }
        // верный код после ошибок ещё проходит, повтор того же кода — нет
        let (ok_body, _) = enroll_body(&f, "A", 0);
        assert_eq!(req(&f, None, "POST", "/enroll", &ok_body).unwrap().0, 200);
        f.server.core.open_window(60);
        assert_eq!(req(&f, None, "POST", "/enroll", &ok_body).unwrap().0, 403, "тот же код дважды не принимается");
    }

    #[test]
    fn ip_blocked_after_five_failures() {
        let f = fx();
        f.server.core.open_window(60);
        let (csr_pem, _) = csr();
        let bad = json!({"code": "123456", "name": "x", "csr": csr_pem}).to_string();
        let mut last = None;
        for _ in 0..5 {
            last = req(&f, None, "POST", "/enroll", &bad).map(|r| r.0);
        }
        assert!(matches!(last, Some(403) | Some(429)));
        // дальше соединение с этого IP закрывается сразу, даже с верным кодом
        let (good, _) = enroll_body(&f, "ok", 0);
        assert!(req(&f, None, "POST", "/enroll", &good).is_none());
    }

    #[test]
    fn foreign_ca_certificate_is_rejected() {
        let f = fx();
        let other = Ca::create("Evil").unwrap();
        let d = other.issue_device("evil").unwrap();
        assert!(req(&f, Some((&d.cert_pem, &d.key_pem)), "GET", "/ping", "").is_none());
    }

    #[test]
    fn phone_approves_new_device_by_qr_and_it_picks_up_cert() {
        let f = fx();
        let (pcert, pkey) = bound(&f); // «телефон»: уже привязанное устройство
        let (csr_pem, lkey) = csr(); // «ноутбук»: свой ключ и запрос на сертификат
        let nonce = "0123456789abcdef0123456789abcdef";
        let pick = json!({"nonce": nonce}).to_string();
        let phone = Some((pcert.as_str(), pkey.as_str()));
        // ничего не ожидает и окно закрыто: неопознанному соединению тишина
        assert!(req(&f, None, "POST", "/v1/pickup", &pick).is_none());
        // без сертификата подтвердить нельзя (тишина)
        let ok_body = |c: String| json!({"code": c, "nonce": nonce, "name": "Ноутбук Артёма", "csr": csr_pem}).to_string();
        assert!(req(&f, None, "POST", "/v1/devices/pair", &ok_body(code(&f, 0))).is_none());
        // неверный код: отказ, ничего не создано
        let (c, v) = req(&f, phone, "POST", "/v1/devices/pair", &ok_body("000000".into())).unwrap();
        assert_eq!((c, v["error"].as_str()), (403, Some("code")));
        assert!(req(&f, None, "POST", "/v1/pickup", &pick).is_none(), "после отказа выдачи нет");
        // плохой номер и плохой запрос на сертификат
        let bad_nonce = json!({"code": code(&f, 0), "nonce": "xyz", "name": "A", "csr": csr_pem}).to_string();
        assert_eq!(req(&f, phone, "POST", "/v1/devices/pair", &bad_nonce).unwrap().0, 400);
        // верный код подтверждает: сервер подписывает запрос и откладывает сертификат на выдачу
        let (c, v) = req(&f, phone, "POST", "/v1/devices/pair", &ok_body(code(&f, 0))).unwrap();
        assert_eq!((c, v["check"].as_str().map(String::from)), (200, Some(pair_check_code(nonce))), "{v}");
        // чужой номер при ожидающей выдаче: «ещё нет», промахи не блокируют IP
        for _ in 0..8 {
            assert_eq!(req(&f, None, "POST", "/v1/pickup", &json!({"nonce": "ffffffffffffffffffffffffffffffff"}).to_string()).unwrap().0, 404);
        }
        // свой номер: сертификат, один раз
        let (c, v) = req(&f, None, "POST", "/v1/pickup", &pick).unwrap();
        assert_eq!(c, 200, "{v}");
        let cert = v["cert"].as_str().unwrap().to_string();
        assert_eq!(v["ca"].as_str().unwrap(), f.ca_pem);
        assert_eq!(req(&f, Some((&cert, &lkey)), "GET", "/ping", "").unwrap().1["device"], "Ноутбук Артёма");
        assert!(req(&f, None, "POST", "/v1/pickup", &pick).is_none(), "повторно забрать нельзя");
        // устройство записано, прав на файлы у него нет
        assert!(f.server.core.devices().iter().any(|d| d.name == "Ноутбук Артёма" && !d.trusted && !d.revoked));
        assert!(f.server.core.log().iter().any(|l| l.event.contains("добавлено по подтверждению")));
    }

    #[test]
    fn pair_rejects_garbage_csr_and_limits_pending() {
        let f = fx();
        let (pcert, pkey) = bound(&f);
        let phone = Some((pcert.as_str(), pkey.as_str()));
        let junk = json!({"code": code(&f, 0), "nonce": "0123456789abcdef0123456789abcdef", "name": "X", "csr_b64": "AAAA"}).to_string();
        assert_eq!(req(&f, phone, "POST", "/v1/devices/pair", &junk).unwrap().0, 400);
        assert!(req(&f, None, "POST", "/v1/pickup", &json!({"nonce": "0123456789abcdef0123456789abcdef"}).to_string()).is_none(), "с плохим запросом выдача не создаётся");
        assert_eq!(pair_check_code("0123456789abcdef0123456789abcdef").len(), 9);
        assert!(!valid_nonce("0123456789abcdef0123456789abcde") && !valid_nonce("g123456789abcdef0123456789abcdef"));
        // DER в base64 превращается в PEM-блок из строк по 64 символа
        let p = csr_pem_from(&json!({"csr_b64": "A".repeat(130)}));
        assert!(p.starts_with("-----BEGIN CERTIFICATE REQUEST-----\n") && p.lines().count() == 5, "{p}");
    }

    #[test]
    fn stop_closes_port() {
        let mut f = fx();
        let addr = f.server.addr;
        f.server.stop();
        assert!(TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_err());
    }
}
