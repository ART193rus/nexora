//! Nexora Agent на ПК: подключение ноутбука к серверу Nexora.
//! Безопасность: закрытый ключ создаётся на ноутбуке и никуда не передаётся (серверу уходит только запрос на сертификат);
//! сервер проверяется по закреплённому отпечатку (не по имени и не по системным корням); соединение только TLS 1.3 с
//! клиентским сертификатом. Привязка двумя способами: кодом Authenticator (окно привязки на сервере) или по QR, который
//! сканирует уже привязанный телефон (он подтверждает новое устройство, см. PLAN.md, «Привязка по QR»).
//! Модель сервера используется через локальный мост: обычный агент Nexora ходит на 127.0.0.1, мост пересылает запросы серверу.
use crate::secure::Vault;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PAIR_TTL_SECS: u64 = 600;

/// Адрес сервера и закреплённый отпечаток (полный SHA-256 в hex; пусто, если известен только короткий код).
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct ServerRef {
    pub host: String,
    pub port: u16,
    pub fp: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Credentials {
    pub cert_pem: String,
    pub key_pem: String,
    pub ca_pem: String,
    pub serial: String,
}

/// Всё, что нужно для работы после привязки; лежит в зашифрованном хранилище.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Connection {
    pub server: ServerRef,
    pub name: String,
    pub creds: Credentials,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Разбор строки подключения `nexora://host:port#отпечаток` (отпечаток: 64 hex; допустим короткий вид a1b2-c3d4-e5f6-0718).
pub fn parse_link(s: &str) -> Result<(ServerRef, Option<String>), String> {
    let t = s.trim();
    let rest = t.strip_prefix("nexora://").ok_or("строка подключения начинается с nexora://")?;
    let (hostport, frag) = rest.split_once('#').unwrap_or((rest, ""));
    let (host, port) = hostport.rsplit_once(':').ok_or("в строке нужен порт: nexora://адрес:порт#отпечаток")?;
    let port: u16 = port.parse().map_err(|_| "порт должен быть числом")?;
    let host = host.trim_matches(['[', ']']).to_string();
    if host.is_empty() || host.len() > 253 || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b':') || port == 0 {
        return Err("недопустимый адрес сервера".into());
    }
    let f: String = frag.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>().to_lowercase();
    match f.len() {
        64 => Ok((ServerRef { host, port, fp: f }, None)),
        16 => Ok((ServerRef { host, port, fp: String::new() }, Some(f))),
        0 => Err("в строке нет отпечатка сервера (#…): без него подлинность сервера не проверить".into()),
        _ => Err("отпечаток в строке неполный или испорчен".into()),
    }
}

// ---------- TLS: закрепление отпечатка ----------
#[derive(Debug)]
struct Pin {
    want: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
    algs: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for Pin {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, _inter: &[CertificateDer<'_>], _name: &ServerName<'_>, _ocsp: &[u8], _now: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        let fp = hex(&Sha256::digest(end_entity.as_ref()));
        *self.seen.lock().unwrap() = Some(fp.clone());
        match &self.want {
            Some(w) if *w != fp => Err(rustls::Error::General("отпечаток сервера не совпал с сохранённым".into())),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }
    fn verify_tls12_signature(&self, _m: &[u8], _c: &CertificateDer<'_>, _d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("только TLS 1.3".into()))
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.algs)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

type Tls = StreamOwned<ClientConnection, TcpStream>;

fn config(want: Option<String>, id: Option<&Credentials>, seen: Arc<Mutex<Option<String>>>) -> Result<Arc<ClientConfig>, String> {
    let provider: Arc<CryptoProvider> = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(Pin { want, seen, algs: provider.signature_verification_algorithms });
    let b = ClientConfig::builder_with_provider(provider).with_protocol_versions(&[&rustls::version::TLS13]).map_err(|e| e.to_string())?.dangerous().with_custom_certificate_verifier(verifier);
    let cfg = match id {
        Some(c) => b
            .with_client_auth_cert(
                vec![CertificateDer::from_pem_slice(c.cert_pem.as_bytes()).map_err(|e| e.to_string())?],
                PrivateKeyDer::from_pem_slice(c.key_pem.as_bytes()).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?,
        None => b.with_no_client_auth(),
    };
    Ok(Arc::new(cfg))
}

fn open(srv: &ServerRef, id: Option<&Credentials>, timeout: Duration) -> Result<(Tls, Arc<Mutex<Option<String>>>), String> {
    let seen = Arc::new(Mutex::new(None));
    let cfg = config(if srv.fp.is_empty() { None } else { Some(srv.fp.clone()) }, id, seen.clone())?;
    let addr = (srv.host.as_str(), srv.port).to_socket_addrs().map_err(|_| "не удалось определить адрес сервера".to_string())?.next().ok_or("адрес сервера не найден")?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(8)).map_err(|e| format!("нет связи с сервером ({}:{}): {e}", srv.host, srv.port))?;
    let _ = tcp.set_read_timeout(Some(timeout));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(10)));
    let name = ServerName::try_from(srv.host.clone()).map_err(|_| "недопустимое имя сервера".to_string())?;
    let conn = ClientConnection::new(cfg, name).map_err(|e| e.to_string())?;
    Ok((StreamOwned::new(conn, tcp), seen))
}

fn handshake(tls: &mut Tls) -> Result<(), String> {
    while tls.conn.is_handshaking() {
        tls.conn.complete_io(&mut tls.sock).map_err(|e| {
            let m = e.to_string();
            if m.contains("отпечаток") { "отпечаток сервера не совпал: это не тот сервер или его сертификат пересоздан. Проверьте строку подключения на экране сервера".to_string() } else { format!("не удалось установить защищённое соединение: {m}") }
        })?;
    }
    Ok(())
}

/// Запрос к серверу. Ok(None): сервер молча закрыл соединение (так он отвечает неопознанным и когда привязка закрыта).
pub fn call(srv: &ServerRef, id: Option<&Credentials>, method: &str, path: &str, body: &str, timeout: Duration) -> Result<Option<(u16, String)>, String> {
    let (mut tls, _) = open(srv, id, timeout)?;
    handshake(&mut tls)?;
    let msg = format!("{method} {path} HTTP/1.1\r\nHost: nexora\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    tls.write_all(msg.as_bytes()).map_err(|_| "сервер закрыл соединение".to_string())?;
    let mut out = Vec::new();
    let _ = tls.read_to_end(&mut out);
    let s = String::from_utf8_lossy(&out).to_string();
    let Some(code) = s.split_whitespace().nth(1).and_then(|c| c.parse::<u16>().ok()) else { return Ok(None) };
    Ok(Some((code, s.split_once("\r\n\r\n").map(|x| x.1).unwrap_or("").to_string())))
}

/// Посмотреть отпечаток сервера, ничего не отправляя (для сверки глазами с экраном сервера).
pub fn probe(host: &str, port: u16) -> Result<String, String> {
    let srv = ServerRef { host: host.to_string(), port, fp: String::new() };
    let (mut tls, seen) = open(&srv, None, Duration::from_secs(8))?;
    handshake(&mut tls)?;
    let fp = seen.lock().unwrap().clone().ok_or("сервер не предъявил сертификат")?;
    Ok(fp)
}

// ---------- ключ и запрос на сертификат ----------
struct NewKey {
    key_pem: String,
    csr_pem: String,
    csr_der_b64: String,
}

fn new_key() -> Result<NewKey, String> {
    let key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let params = rcgen::CertificateParams::new(Vec::<String>::new()).map_err(|e| e.to_string())?;
    let csr = params.serialize_request(&key).map_err(|e| e.to_string())?;
    Ok(NewKey { key_pem: key.serialize_pem(), csr_pem: csr.pem().map_err(|e| e.to_string())?, csr_der_b64: crate::ssh::b64(csr.der().as_ref()) })
}

fn percent(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn clean_device_name(s: &str) -> String {
    let n: String = s.chars().filter(|c| !c.is_control()).take(64).collect();
    n.trim().to_string()
}

fn err_text(code: u16, body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    match (code, v["error"].as_str()) {
        (429, _) => "слишком много попыток: подождите 15 минут".into(),
        (403, _) => {
            let skew = v["time"].as_u64().map(|t| t.abs_diff(crate::totp::now_secs())).filter(|d| *d > 25);
            match skew {
                Some(d) => format!("код не подошёл. Часы этого компьютера расходятся с часами сервера на {d} с: синхронизируйте время"),
                None => "код не подошёл или окно привязки на сервере закрыто".into(),
            }
        }
        (c, e) => format!("сервер ответил {c}{}", e.map(|e| format!(": {e}")).unwrap_or_default()),
    }
}

/// Привязка кодом Authenticator: на сервере должно быть открыто окно привязки.
pub fn enroll_with_code(srv: &ServerRef, name: &str, code: &str) -> Result<Credentials, String> {
    let name = clean_device_name(name);
    if name.is_empty() {
        return Err("введите имя этого компьютера".into());
    }
    let k = new_key()?;
    let body = json!({"code": code.trim(), "name": name, "csr": k.csr_pem}).to_string();
    match call(srv, None, "POST", "/enroll", &body, Duration::from_secs(15))? {
        None => Err("сервер не отвечает на привязку: откройте окно привязки на сервере (кнопка «Открыть окно привязки»)".into()),
        Some((200, b)) => {
            let v: Value = serde_json::from_str(&b).map_err(|_| "непонятный ответ сервера")?;
            Ok(Credentials { cert_pem: v["cert"].as_str().ok_or("в ответе нет сертификата")?.into(), key_pem: k.key_pem, ca_pem: v["ca"].as_str().unwrap_or("").into(), serial: v["serial"].as_str().unwrap_or("").into() })
        }
        Some((c, b)) => Err(err_text(c, &b)),
    }
}

/// Привязка по QR, шаг 1: ключ, запрос на сертификат, одноразовый номер, текст для QR-кода.
pub struct PairSession {
    pub nonce: String,
    pub name: String,
    key_pem: String,
    pub qr_text: String,
    pub check: String,
    started: Instant,
}

pub fn pair_start(srv: &ServerRef, name: &str) -> Result<PairSession, String> {
    let name = clean_device_name(name);
    if name.is_empty() {
        return Err("введите имя этого компьютера".into());
    }
    let k = new_key()?;
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).map_err(|e| e.to_string())?;
    let nonce = hex(&b);
    let short: String = srv.fp.chars().take(16).collect::<Vec<_>>().chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join("-");
    let qr_text = format!("nexora-pair://1?n={nonce}&name={}&fp={short}&csr={}", percent(&name), percent(&k.csr_der_b64));
    Ok(PairSession { check: crate::server::pair_check_code(&nonce), nonce, name, key_pem: k.key_pem, qr_text, started: Instant::now() })
}

/// Шаг 2 (опрос раз в пару секунд): Ok(Some) когда телефон подтвердил и сертификат выдан; Ok(None) пока ждём.
pub fn pair_poll(srv: &ServerRef, s: &PairSession) -> Result<Option<Credentials>, String> {
    if s.started.elapsed() > Duration::from_secs(PAIR_TTL_SECS) {
        return Err("время ожидания истекло (10 минут): начните заново".into());
    }
    match call(srv, None, "POST", "/v1/pickup", &json!({"nonce": s.nonce}).to_string(), Duration::from_secs(10))? {
        None | Some((404, _)) => Ok(None),
        Some((200, b)) => {
            let v: Value = serde_json::from_str(&b).map_err(|_| "непонятный ответ сервера")?;
            Ok(Some(Credentials { cert_pem: v["cert"].as_str().ok_or("в ответе нет сертификата")?.into(), key_pem: s.key_pem.clone(), ca_pem: v["ca"].as_str().unwrap_or("").into(), serial: v["serial"].as_str().unwrap_or("").into() }))
        }
        Some((c, b)) => Err(err_text(c, &b)),
    }
}

// ---------- хранение ----------
fn vault() -> Result<Vault, String> {
    Vault::open(&crate::engine::data_dir().join("vault"), None)
}

pub fn save(c: &Connection) -> Result<(), String> {
    vault()?.put("agent_conn", serde_json::to_string(c).map_err(|e| e.to_string())?.as_bytes())
}

pub fn load() -> Option<Connection> {
    vault().ok()?.get("agent_conn").ok().flatten().and_then(|b| serde_json::from_slice(&b).ok())
}

pub fn forget() -> Result<(), String> {
    vault()?.delete("agent_conn")
}

// ---------- локальный мост ----------
/// Мост на 127.0.0.1: обычный агент Nexora (как с любым внешним движком) ходит сюда по HTTP с ключом моста,
/// мост пересылает запрос серверу по защищённому каналу. Разрешены только /v1/models и /v1/chat/completions.
pub struct Proxy {
    pub port: u16,
    pub token: String,
    stop: Arc<AtomicBool>,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

fn local_reply(s: &mut TcpStream, code: u16, text: &str) {
    let body = json!({"error": text}).to_string();
    let _ = s.write_all(format!("HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
}

fn serve_local(mut s: TcpStream, srv: &ServerRef, creds: &Credentials, token: &str) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let head_end = loop {
        if let Some(p) = find(&buf, b"\r\n\r\n") {
            break p + 4;
        }
        if buf.len() > 32 * 1024 {
            return;
        }
        match s.read(&mut tmp) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut rl = lines.next().unwrap_or("").split_whitespace();
    let (method, path) = (rl.next().unwrap_or("").to_string(), rl.next().unwrap_or("").to_string());
    let hdr = |k: &str| head.split("\r\n").skip(1).filter_map(|l| l.split_once(':')).find(|(a, _)| a.trim().eq_ignore_ascii_case(k)).map(|(_, v)| v.trim().to_string());
    if hdr("authorization").as_deref() != Some(&format!("Bearer {token}")) {
        return local_reply(&mut s, 401, "unauthorized");
    }
    let allowed = matches!((method.as_str(), path.as_str()), ("GET", "/v1/models") | ("POST", "/v1/chat/completions"));
    if !allowed {
        return local_reply(&mut s, 404, "not found");
    }
    let cl: usize = hdr("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if cl > 16 * 1024 * 1024 {
        return local_reply(&mut s, 413, "too large");
    }
    while buf.len() < head_end + cl {
        match s.read(&mut tmp) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
    let body = &buf[head_end..head_end + cl];
    let upstream = (|| -> Result<Tls, String> {
        let (mut tls, _) = open(srv, Some(creds), Duration::from_secs(1800))?;
        handshake(&mut tls)?;
        let req = format!("{method} {path} HTTP/1.1\r\nHost: nexora\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        tls.write_all(req.as_bytes()).and_then(|_| tls.write_all(body)).and_then(|_| tls.flush()).map_err(|_| "сервер закрыл соединение".to_string())?;
        Ok(tls)
    })();
    let mut tls = match upstream {
        Ok(t) => t,
        Err(e) => return local_reply(&mut s, 502, &e),
    };
    let mut first = true;
    let mut chunk = [0u8; 4096];
    loop {
        match tls.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                first = false;
                if s.write_all(&chunk[..n]).and_then(|_| s.flush()).is_err() {
                    return; // локальный клиент отключился: закрываем канал к серверу, генерация остановится
                }
            }
        }
    }
    if first {
        local_reply(&mut s, 502, "сервер не ответил (возможно, права отозваны или модель не запущена)");
    }
}

pub fn start_proxy(srv: ServerRef, creds: Credentials) -> Result<Proxy, String> {
    let l = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    l.set_nonblocking(true).map_err(|e| e.to_string())?;
    let port = l.local_addr().map_err(|e| e.to_string())?.port();
    let mut tb = [0u8; 24];
    getrandom::getrandom(&mut tb).map_err(|e| e.to_string())?;
    let token = hex(&tb);
    let stop = Arc::new(AtomicBool::new(false));
    let (st2, tok2) = (stop.clone(), token.clone());
    std::thread::spawn(move || {
        while !st2.load(Ordering::SeqCst) {
            match l.accept() {
                Ok((s, _)) => {
                    let _ = s.set_nonblocking(false);
                    let (srv, creds, tok) = (srv.clone(), creds.clone(), tok2.clone());
                    std::thread::spawn(move || serve_local(s, &srv, &creds, &tok));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(40)),
                Err(_) => std::thread::sleep(Duration::from_millis(200)),
            }
        }
    });
    Ok(Proxy { port, token, stop })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pki::Ca;
    use crate::server::{Core, Server};
    use crate::totp;

    struct Fx {
        server: Server,
        srv_ref: ServerRef,
        secret: Vec<u8>,
    }

    fn fx() -> Fx {
        let ca = Ca::create("Test").unwrap();
        let srv_cert = ca.issue_server(&["127.0.0.1".into()]).unwrap();
        let fp = srv_cert.fingerprint.clone();
        let secret_b32 = totp::new_secret().unwrap();
        let secret = totp::base32_decode(&secret_b32).unwrap();
        let core = Arc::new(Core::new(ca, &secret_b32, vec![]).unwrap());
        let server = Server::start(core, &srv_cert, "127.0.0.1:0".parse().unwrap()).unwrap();
        let srv_ref = ServerRef { host: "127.0.0.1".into(), port: server.addr.port(), fp };
        Fx { server, srv_ref, secret }
    }

    fn code(f: &Fx) -> String {
        totp::code_at(&f.secret, totp::now_secs() / 30)
    }

    #[test]
    fn link_parsing() {
        let fp = "ab".repeat(32);
        let (s, short) = parse_link(&format!("  nexora://192.168.1.120:47758#{fp} ")).unwrap();
        assert_eq!((s.host.as_str(), s.port, s.fp.as_str(), short), ("192.168.1.120", 47758, fp.as_str(), None));
        let (s, short) = parse_link("nexora://pc.local:5000#a1b2-c3d4-e5f6-0718").unwrap();
        assert_eq!((s.fp.as_str(), short.as_deref()), ("", Some("a1b2c3d4e5f60718")));
        for bad in ["http://x:1#ab", "nexora://x#ab", "nexora://x:99999#ab", "nexora://x:1", "nexora://x:1#abc", "nexora://:1#".into(), "nexora://a b:1#ab"] {
            assert!(parse_link(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn probe_shows_pinned_fingerprint_and_pin_mismatch_is_refused() {
        let f = fx();
        assert_eq!(probe("127.0.0.1", f.srv_ref.port).unwrap(), f.srv_ref.fp, "отпечаток виден до привязки");
        let wrong = ServerRef { fp: "00".repeat(32), ..f.srv_ref.clone() };
        let e = call(&wrong, None, "POST", "/enroll", "{}", Duration::from_secs(5)).unwrap_err();
        assert!(e.contains("отпечаток сервера не совпал"), "{e}");
    }

    #[test]
    fn enroll_with_code_then_work_with_certificate() {
        let f = fx();
        // окно закрыто: понятная причина
        let e = enroll_with_code(&f.srv_ref, "Ноутбук", &code(&f)).unwrap_err();
        assert!(e.contains("окно привязки"), "{e}");
        f.server.core.open_window(60);
        assert!(enroll_with_code(&f.srv_ref, "Ноутбук", "000000").unwrap_err().contains("не подошёл"));
        let creds = enroll_with_code(&f.srv_ref, "Ноутбук", &code(&f)).expect("привязка");
        let (c, b) = call(&f.srv_ref, Some(&creds), "GET", "/ping", "", Duration::from_secs(5)).unwrap().unwrap();
        assert_eq!(c, 200);
        assert!(b.contains("Ноутбук"));
        // сертификат другой стороны не подходит: тишина
        assert!(call(&f.srv_ref, None, "GET", "/ping", "", Duration::from_secs(5)).unwrap().is_none());
        assert!(enroll_with_code(&f.srv_ref, "  ", "123456").unwrap_err().contains("имя"));
    }

    #[test]
    fn pair_by_phone_qr_end_to_end() {
        let f = fx();
        f.server.core.open_window(60);
        let phone = enroll_with_code(&f.srv_ref, "Телефон", &code(&f)).unwrap();
        // ноутбук: QR
        let ps = pair_start(&f.srv_ref, "Ноутбук Артёма").unwrap();
        assert!(ps.qr_text.starts_with("nexora-pair://1?n=") && ps.qr_text.contains("&csr=") && ps.qr_text.len() < 1500, "{}", ps.qr_text);
        assert_eq!(ps.nonce.len(), 32);
        assert!(pair_poll(&f.srv_ref, &ps).unwrap().is_none(), "пока никто не подтвердил: ждём (сервер молчит)");
        // «телефон» разбирает QR и подтверждает (код Authenticator того же шага)
        let q = &ps.qr_text["nexora-pair://1?".len()..];
        let get = |k: &str| q.split('&').filter_map(|kv| kv.split_once('=')).find(|(a, _)| *a == k).map(|(_, v)| v.to_string()).unwrap();
        let unp = |s: String| {
            let b = s.as_bytes();
            let mut out = Vec::new();
            let mut i = 0;
            while i < b.len() {
                if b[i] == b'%' {
                    out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                    i += 3;
                } else {
                    out.push(b[i]);
                    i += 1;
                }
            }
            String::from_utf8(out).unwrap()
        };
        assert_eq!(unp(get("name")), "Ноутбук Артёма");
        let body = json!({"code": code(&f), "nonce": get("n"), "name": unp(get("name")), "csr_b64": unp(get("csr"))}).to_string();
        let (c, v) = call(&f.srv_ref, Some(&phone), "POST", "/v1/devices/pair", &body, Duration::from_secs(10)).unwrap().unwrap();
        assert_eq!(c, 200, "{v}");
        assert!(v.contains(&ps.check), "код сверки сервера совпадает с показанным на ноутбуке");
        // ноутбук забирает сертификат
        let creds = pair_poll(&f.srv_ref, &ps).unwrap().expect("сертификат выдан");
        assert_eq!(call(&f.srv_ref, Some(&creds), "GET", "/ping", "", Duration::from_secs(5)).unwrap().unwrap().0, 200);
        assert!(pair_poll(&f.srv_ref, &ps).unwrap().is_none(), "второй раз тот же номер уже ничего не даёт");
    }

    #[test]
    fn saved_connection_roundtrip_with_encryption() {
        let dir = std::env::temp_dir().join(format!("nexora-agentconn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let v = Vault::open(&dir, None).unwrap();
        let c = Connection { server: ServerRef { host: "h".into(), port: 1, fp: "ab".repeat(32) }, name: "N".into(), creds: Credentials { cert_pem: "C".into(), key_pem: "SECRET-KEY".into(), ca_pem: "A".into(), serial: "1".into() } };
        v.put("agent_conn", serde_json::to_string(&c).unwrap().as_bytes()).unwrap();
        let raw = std::fs::read(dir.join("agent_conn.nxs")).unwrap();
        assert!(!raw.windows(10).any(|w| w == b"SECRET-KEY"), "ключ на диске в открытом виде не лежит");
        let back: Connection = serde_json::from_slice(&v.get("agent_conn").unwrap().unwrap()).unwrap();
        assert_eq!(back.creds.key_pem, "SECRET-KEY");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn fake_engine() -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for c in l.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut c = c;
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    loop {
                        let n = c.read(&mut tmp).unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(p) = find(&buf, b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..p]).to_lowercase();
                            let cl: usize = head.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                            if buf.len() >= p + 4 + cl {
                                break;
                            }
                        }
                    }
                    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"при\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"вет\"}}]}\n\ndata: [DONE]\n\n";
                    let _ = c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{sse}").as_bytes());
                });
            }
        });
        base
    }

    #[test]
    fn local_proxy_streams_model_answer_and_guards_access() {
        let f = fx();
        f.server.core.open_window(60);
        let creds = enroll_with_code(&f.srv_ref, "Ноутбук", &code(&f)).unwrap();
        let eng = fake_engine();
        f.server.core.set_upstream(Arc::new(move || Some((eng.clone(), "tok".into(), "qwen".into()))));
        let proxy = start_proxy(f.srv_ref.clone(), creds).unwrap();
        let base = format!("http://127.0.0.1:{}", proxy.port);
        // модель сервера видна как внешний движок
        let m: Value = ureq::get(&format!("{base}/v1/models")).set("Authorization", &format!("Bearer {}", proxy.token)).call().unwrap().into_json().unwrap();
        assert_eq!(m["data"][0]["id"], "qwen");
        // поток ответа доходит
        let r = ureq::post(&format!("{base}/v1/chat/completions")).set("Authorization", &format!("Bearer {}", proxy.token)).send_json(json!({"messages": [{"role": "user", "content": "hi"}], "model": "x"})).unwrap();
        let mut text = String::new();
        r.into_reader().read_to_string(&mut text).unwrap();
        assert!(text.contains("\"при\"") && text.contains("[DONE]"), "{text}");
        // чужой процесс без ключа моста и лишние маршруты не проходят
        assert!(matches!(ureq::get(&format!("{base}/v1/models")).call(), Err(ureq::Error::Status(401, _))));
        assert!(matches!(ureq::get(&format!("{base}/ping")).set("Authorization", &format!("Bearer {}", proxy.token)).call(), Err(ureq::Error::Status(404, _))));
        assert!(matches!(ureq::post(&format!("{base}/bind-window")).set("Authorization", &format!("Bearer {}", proxy.token)).send_string("{}"), Err(ureq::Error::Status(404, _))));
    }

    #[test]
    fn second_listener_shares_core_cert_and_devices() {
        // имитация «адрес VPN»: тот же порт на другом адресе (127.0.0.2) с тем же ядром и сертификатом
        let ca = Ca::create("Test").unwrap();
        let srv_cert = ca.issue_server(&["127.0.0.1".into()]).unwrap();
        let secret_b32 = totp::new_secret().unwrap();
        let secret = totp::base32_decode(&secret_b32).unwrap();
        let core = Arc::new(Core::new(ca, &secret_b32, vec![]).unwrap());
        let a = Server::start(core.clone(), &srv_cert, "127.0.0.1:0".parse().unwrap()).unwrap();
        let port = a.addr.port();
        let b = Server::start(core.clone(), &srv_cert, format!("127.0.0.2:{port}").parse().unwrap()).expect("второй адрес");
        let home = ServerRef { host: "127.0.0.1".into(), port, fp: srv_cert.fingerprint.clone() };
        let away = ServerRef { host: "127.0.0.2".into(), port, fp: srv_cert.fingerprint.clone() };
        assert_eq!(probe("127.0.0.1", port).unwrap(), probe("127.0.0.2", port).unwrap(), "один и тот же отпечаток на обоих адресах");
        // устройство, привязанное «дома», работает и «из любой точки» без новой привязки
        core.open_window(60);
        let creds = enroll_with_code(&home, "Телефон", &totp::code_at(&secret, totp::now_secs() / 30)).unwrap();
        assert_eq!(call(&away, Some(&creds), "GET", "/ping", "", Duration::from_secs(5)).unwrap().unwrap().0, 200);
        // выключение дополнительного слушателя закрывает только его адрес
        drop(b);
        assert!(call(&away, Some(&creds), "GET", "/ping", "", Duration::from_secs(2)).is_err());
        assert_eq!(call(&home, Some(&creds), "GET", "/ping", "", Duration::from_secs(5)).unwrap().unwrap().0, 200);
        drop(a);
    }
}
