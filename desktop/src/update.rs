//! Обновления с подписью. Программа сама ничего не ставит: пользователь нажимает «Проверить», потом «Установить».
//! Манифест лежит по HTTPS-адресу из настроек, установщик скачивается тоже только по HTTPS. Запускается он только после
//! двух проверок: контрольная сумма скачанного файла и подпись Ed25519 над строкой `nexora-update:v1:<версия>:<sha256>`
//! открытым ключом, вшитым в программу. Закрытый ключ есть только у издателя (хранится у владельца, в репозиторий не попадает).
//! Откат на более старую версию запрещён. В режиме «Сервер» обновления тоже только вручную.
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Открытый ключ подписи обновлений (base64, 32 байта). Парный закрытый ключ хранится у издателя.
pub const UPDATE_PUBKEY_B64: &str = "b+rDdF2yJdcG+z4Rd5j5AIcIgae2k7aNOM2yyi7j4Bc=";

const MAX_MANIFEST: u64 = 64 * 1024;
const MAX_INSTALLER: u64 = 500 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Manifest {
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub sig: String,
    #[serde(default)]
    pub notes: String,
}

pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut it = v.trim().trim_start_matches('v').split('.');
    let (a, b, c) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    if it.next().is_some() { None } else { Some((a, b, c)) }
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn signed_text(version: &str, sha256: &str) -> String {
    format!("nexora-update:v1:{version}:{}", sha256.to_lowercase())
}

fn pubkey(b64: &str) -> Result<VerifyingKey, String> {
    let raw = crate::browser::b64_decode(b64);
    let arr: [u8; 32] = raw.try_into().map_err(|_| "открытый ключ обновлений не задан или повреждён".to_string())?;
    VerifyingKey::from_bytes(&arr).map_err(|_| "открытый ключ обновлений недействителен".to_string())
}

/// Проверка подписи манифеста: версия и контрольная сумма подписаны вместе, подмена любого поля ломает подпись.
pub fn verify(m: &Manifest, pub_b64: &str) -> Result<(), String> {
    if parse_version(&m.version).is_none() {
        return Err("в манифесте неверный номер версии".into());
    }
    if m.sha256.len() != 64 || !m.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("в манифесте неверная контрольная сумма".into());
    }
    let sig_raw = crate::browser::b64_decode(&m.sig);
    let sig_arr: [u8; 64] = sig_raw.try_into().map_err(|_| "подпись обновления повреждена".to_string())?;
    pubkey(pub_b64)?.verify(signed_text(&m.version, &m.sha256).as_bytes(), &Signature::from_bytes(&sig_arr)).map_err(|_| "подпись обновления не подошла: файл мог быть подменён".to_string())
}

fn https_only(url: &str, allow_http: bool) -> Result<(), String> {
    if url.starts_with("https://") || (allow_http && url.starts_with("http://127.0.0.1")) {
        Ok(())
    } else {
        Err("адрес обновлений должен начинаться с https://".into())
    }
}

fn get_bytes(url: &str, max: u64, allow_http: bool, timeout: u64) -> Result<Vec<u8>, String> {
    https_only(url, allow_http)?;
    let resp = ureq::get(url).set("User-Agent", "Nexora-updater").timeout(Duration::from_secs(timeout)).call().map_err(|e| match e {
        ureq::Error::Status(c, _) => format!("сервер обновлений ответил {c}"),
        o => format!("нет связи с сервером обновлений: {o}"),
    })?;
    let mut buf = Vec::new();
    resp.into_reader().take(max + 1).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    if buf.len() as u64 > max {
        return Err("файл обновления больше допустимого размера".into());
    }
    Ok(buf)
}

/// Скачать манифест и проверить его подпись. Some, если версия новее текущей.
pub fn check(url: &str, current: &str, pub_b64: &str, allow_http: bool) -> Result<Option<Manifest>, String> {
    let body = get_bytes(url, MAX_MANIFEST, allow_http, 20)?;
    let m: Manifest = serde_json::from_slice(&body).map_err(|_| "манифест обновлений не разобран".to_string())?;
    verify(&m, pub_b64)?;
    https_only(&m.url, allow_http)?;
    Ok(if is_newer(&m.version, current) { Some(m) } else { None })
}

/// Скачать установщик и проверить сумму и подпись. Возвращает путь; при любой проблеме файл удаляется.
pub fn download(m: &Manifest, dir: &Path, current: &str, pub_b64: &str, allow_http: bool) -> Result<PathBuf, String> {
    verify(m, pub_b64)?;
    if !is_newer(&m.version, current) {
        return Err("предложенная версия не новее установленной (откат запрещён)".into());
    }
    let bytes = get_bytes(&m.url, MAX_INSTALLER, allow_http, 600)?;
    let sum: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if sum != m.sha256.to_lowercase() {
        return Err("контрольная сумма скачанного файла не совпала: установка отменена".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let name = format!("Nexora-update-{}.exe", m.version);
    let path = dir.join(name);
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Запустить установщик (с его собственным окном; он сам попросит закрыть Nexora).
pub fn launch(path: &Path) -> Result<(), String> {
    std::process::Command::new(path).spawn().map(|_| ()).map_err(|e| format!("не удалось запустить установщик: {e}"))
}

// ---------- инструменты издателя (запускаются с ключами командной строки, у пользователей не нужны) ----------
/// Новая пара ключей: (закрытый ключ base64, открытый ключ base64).
pub fn gen_keypair() -> Result<(String, String), String> {
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| e.to_string())?;
    let sk = SigningKey::from_bytes(&seed);
    Ok((crate::ssh::b64(&seed), crate::ssh::b64(sk.verifying_key().as_bytes())))
}

/// Подписать файл установщика: строка манифеста JSON.
pub fn sign_release(file: &Path, version: &str, url: &str, notes: &str, priv_b64: &str) -> Result<String, String> {
    if parse_version(version).is_none() {
        return Err("версия вида 1.2.3".into());
    }
    let seed: [u8; 32] = crate::browser::b64_decode(priv_b64.trim()).try_into().map_err(|_| "закрытый ключ повреждён".to_string())?;
    let data = std::fs::read(file).map_err(|e| e.to_string())?;
    let sha: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
    let sig = SigningKey::from_bytes(&seed).sign(signed_text(version, &sha).as_bytes());
    serde_json::to_string_pretty(&Manifest { version: version.into(), url: url.into(), sha256: sha, sig: crate::ssh::b64(&sig.to_bytes()), notes: notes.into() }).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    fn keys() -> (String, String) {
        gen_keypair().unwrap()
    }

    fn tmpfile(tag: &str, data: &[u8]) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nexora-upd-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("setup.exe");
        std::fs::write(&p, data).unwrap();
        p
    }

    fn serve(routes: Vec<(&'static str, Vec<u8>)>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let routes = routes.clone();
                std::thread::spawn(move || {
                    let mut s = s;
                    let mut buf = [0u8; 2048];
                    let n = s.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    match routes.iter().find(|(p, _)| *p == path) {
                        Some((_, body)) => {
                            let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
                            let _ = s.write_all(body);
                        }
                        None => {
                            let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        }
                    }
                });
            }
        });
        base
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert!(parse_version("1.2").is_none() && parse_version("1.2.3.4").is_none() && parse_version("a.b.c").is_none());
        assert!(is_newer("0.2.0", "0.1.9") && is_newer("1.0.0", "0.99.99") && is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("0.1.0", "0.1.0") && !is_newer("0.0.9", "0.1.0") && !is_newer("мусор", "0.1.0"));
    }

    #[test]
    fn sign_and_verify_roundtrip_and_tampering() {
        let (privk, pubk) = keys();
        let f = tmpfile("sig", b"installer-bytes");
        let m: Manifest = serde_json::from_str(&sign_release(&f, "0.2.0", "https://example.com/setup.exe", "Что нового", &privk).unwrap()).unwrap();
        assert!(verify(&m, &pubk).is_ok());
        // подмена версии (откат или подстановка), суммы, подписи, чужой ключ
        assert!(verify(&Manifest { version: "0.2.1".into(), ..m.clone() }, &pubk).is_err(), "версия подписана вместе с суммой");
        assert!(verify(&Manifest { sha256: "0".repeat(64), ..m.clone() }, &pubk).is_err());
        let mut bad = crate::browser::b64_decode(&m.sig);
        bad[0] ^= 1;
        assert!(verify(&Manifest { sig: crate::ssh::b64(&bad), ..m.clone() }, &pubk).is_err());
        assert!(verify(&m, &keys().1).is_err(), "подпись чужим ключом не принимается");
        assert!(verify(&m, "REPLACE_WITH_PUBLIC_KEY").is_err(), "без настоящего ключа ничего не проходит");
        assert!(verify(&Manifest { version: "x".into(), ..m.clone() }, &pubk).is_err());
        assert!(verify(&Manifest { sig: "".into(), ..m }, &pubk).is_err());
    }

    #[test]
    fn https_is_required_in_production() {
        assert!(check("http://example.com/m.json", "0.1.0", "x", false).unwrap_err().contains("https://"));
        assert!(check("ftp://example.com/m.json", "0.1.0", "x", false).is_err());
        assert!(download(&Manifest { version: "0.2.0".into(), url: "http://example.com/s.exe".into(), sha256: "0".repeat(64), sig: "".into(), notes: "".into() }, &std::env::temp_dir(), "0.1.0", "x", false).is_err());
    }

    #[test]
    fn full_flow_check_download_verify_against_local_server() {
        let (privk, pubk) = keys();
        let installer = b"fake-installer-content-0123456789".to_vec();
        let f = tmpfile("flow", &installer);
        let base_holder = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        // сервер: порт нужен для url внутри манифеста, поэтому поднимаем в два шага
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let base = format!("http://127.0.0.1:{port}");
        let manifest = sign_release(&f, "0.3.0", &format!("{base}/setup.exe"), "", &privk).unwrap();
        let tampered = serde_json::to_string(&{
            let mut m: Manifest = serde_json::from_str(&manifest).unwrap();
            m.sha256 = "ab".repeat(32);
            m
        })
        .unwrap();
        let routes: Vec<(&'static str, Vec<u8>)> = vec![("/m.json", manifest.clone().into_bytes()), ("/setup.exe", installer.clone()), ("/bad.json", tampered.into_bytes()), ("/evil.exe", b"other-bytes".to_vec())];
        let l2 = TcpListener::bind(format!("127.0.0.1:{port}")).unwrap();
        std::thread::spawn(move || {
            for s in l2.incoming().flatten() {
                let routes = routes.clone();
                std::thread::spawn(move || {
                    let mut s = s;
                    let mut buf = [0u8; 2048];
                    let n = s.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    match routes.iter().find(|(p, _)| *p == path) {
                        Some((_, body)) => {
                            let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
                            let _ = s.write_all(body);
                        }
                        None => {
                            let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        }
                    }
                });
            }
        });
        base_holder.lock().unwrap().push_str(&base);
        // новее текущей: предлагается; не новее: нет
        let m = check(&format!("{base}/m.json"), "0.1.0", &pubk, true).unwrap().expect("есть обновление");
        assert_eq!(m.version, "0.3.0");
        assert!(check(&format!("{base}/m.json"), "0.3.0", &pubk, true).unwrap().is_none());
        assert!(check(&format!("{base}/bad.json"), "0.1.0", &pubk, true).is_err(), "манифест с подменённой суммой отвергается");
        assert!(check(&format!("{base}/нет.json"), "0.1.0", &pubk, true).is_err());
        // скачать и сверить
        let dir = std::env::temp_dir().join(format!("nexora-upd-dl-{}", std::process::id()));
        let p = download(&m, &dir, "0.1.0", &pubk, true).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), installer);
        // подмена файла на сервере: сумма не совпадёт, файл не сохраняется
        let evil = Manifest { url: format!("{base}/evil.exe"), ..m.clone() };
        let dir2 = std::env::temp_dir().join(format!("nexora-upd-dl2-{}", std::process::id()));
        assert!(download(&evil, &dir2, "0.1.0", &pubk, true).unwrap_err().contains("контрольная сумма"));
        assert!(!dir2.join("Nexora-update-0.3.0.exe").exists());
        // откат запрещён
        assert!(download(&m, &dir, "0.3.0", &pubk, true).unwrap_err().contains("откат"));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = serve(vec![]); // функция используется и в других проверках
    }
}
