//! Хранилище секретов: AES-256-GCM, ключ данных защищён Windows DPAPI (привязка к учётке и ПК)
//! и, по желанию, мастер-паролем (Argon2id). Скопированная на другой ПК папка бесполезна.
//!
//! Файл ключа `vault.key`: байт 0 = режим (0 без пароля, 1 с паролем), дальше блок DPAPI.
//! Внутри DPAPI: режим 0 -> 32 байта ключа; режим 1 -> salt(16) | nonce(12) | AES-GCM(ключ) под Argon2id(пароль).
//! Файл секрета: "NXS1" | nonce(12) | шифртекст+тег. Имя секрета входит в AAD, поэтому файлы нельзя подменять местами.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"NXS1";
const KEYFILE: &str = "vault.key";

fn rand<const N: usize>() -> Result<[u8; N], String> {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).map_err(|e| e.to_string())?;
    Ok(b)
}

fn aes_seal(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Result<Vec<u8>, String> {
    let nonce = rand::<12>()?;
    let ct = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad })
        .map_err(|_| "шифрование не удалось".to_string())?;
    let mut out = nonce.to_vec();
    out.extend(ct);
    Ok(out)
}

fn aes_open(key: &[u8; 32], aad: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 12 + 16 {
        return Err("повреждённые данные".into());
    }
    Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
        .decrypt(Nonce::from_slice(&data[..12]), Payload { msg: &data[12..], aad })
        .map_err(|_| "не удалось расшифровать (чужой ПК, неверный пароль или файл изменён)".to_string())
}

fn kdf(password: &str, salt: &[u8]) -> Result<[u8; 32], String> {
    let mut out = [0u8; 32];
    argon2::Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut out)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

// ---------- DPAPI ----------
#[cfg(windows)]
mod dpapi {
    use std::ffi::c_void;

    #[repr(C)]
    struct Blob {
        cb: u32,
        pb: *mut u8,
    }
    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(i: *const Blob, d: *const u16, e: *const Blob, r: *mut c_void, p: *mut c_void, f: u32, o: *mut Blob) -> i32;
        fn CryptUnprotectData(i: *const Blob, d: *mut *mut u16, e: *const Blob, r: *mut c_void, p: *mut c_void, f: u32, o: *mut Blob) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(h: *mut c_void) -> *mut c_void;
    }
    const UI_FORBIDDEN: u32 = 0x1;

    unsafe fn take(out: Blob) -> Vec<u8> {
        let v = std::slice::from_raw_parts(out.pb, out.cb as usize).to_vec();
        LocalFree(out.pb as *mut c_void);
        v
    }

    pub fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
        unsafe {
            let inp = Blob { cb: data.len() as u32, pb: data.as_ptr() as *mut u8 };
            let mut out = Blob { cb: 0, pb: std::ptr::null_mut() };
            let ok = CryptProtectData(&inp, std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), UI_FORBIDDEN, &mut out);
            if ok == 0 {
                return Err("DPAPI: защита ключа не удалась".into());
            }
            Ok(take(out))
        }
    }

    pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
        unsafe {
            let inp = Blob { cb: data.len() as u32, pb: data.as_ptr() as *mut u8 };
            let mut out = Blob { cb: 0, pb: std::ptr::null_mut() };
            let ok = CryptUnprotectData(&inp, std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), UI_FORBIDDEN, &mut out);
            if ok == 0 {
                return Err("DPAPI: ключ не открывается (другой ПК или другая учётная запись)".into());
            }
            Ok(take(out))
        }
    }
}

#[cfg(not(windows))]
mod dpapi {
    pub fn protect(_: &[u8]) -> Result<Vec<u8>, String> {
        Err("DPAPI доступен только в Windows".into())
    }
    pub fn unprotect(_: &[u8]) -> Result<Vec<u8>, String> {
        Err("DPAPI доступен только в Windows".into())
    }
}

// ---------- Хранилище ----------
pub struct Vault {
    key: [u8; 32],
    dir: PathBuf,
}

impl Drop for Vault {
    fn drop(&mut self) {
        self.key = [0; 32];
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl Vault {
    /// Есть ли уже ключ и нужен ли для него пароль.
    pub fn status(dir: &Path) -> Option<bool> {
        std::fs::read(dir.join(KEYFILE)).ok().and_then(|b| b.first().map(|m| *m == 1))
    }

    /// Открыть хранилище; если ключа нет, создать (с паролем или без).
    pub fn open(dir: &Path, password: Option<&str>) -> Result<Vault, String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let kf = dir.join(KEYFILE);
        if !kf.exists() {
            let key = rand::<32>()?;
            let (mode, inner) = match password {
                Some(p) if !p.is_empty() => {
                    let salt = rand::<16>()?;
                    let mut v = salt.to_vec();
                    v.extend(aes_seal(&kdf(p, &salt)?, b"nexora-key", &key)?);
                    (1u8, v)
                }
                _ => (0u8, key.to_vec()),
            };
            let mut file = vec![mode];
            file.extend(dpapi::protect(&inner)?);
            write_atomic(&kf, &file)?;
            return Ok(Vault { key, dir: dir.to_path_buf() });
        }
        let raw = std::fs::read(&kf).map_err(|e| e.to_string())?;
        let (mode, blob) = raw.split_first().ok_or("пустой файл ключа")?;
        let inner = dpapi::unprotect(blob)?;
        let key: [u8; 32] = match mode {
            0 => inner.try_into().map_err(|_| "повреждённый ключ")?,
            1 => {
                let p = password.filter(|p| !p.is_empty()).ok_or("нужен мастер-пароль")?;
                if inner.len() < 16 {
                    return Err("повреждённый ключ".into());
                }
                let k = aes_open(&kdf(p, &inner[..16])?, b"nexora-key", &inner[16..])?;
                k.try_into().map_err(|_| "повреждённый ключ")?
            }
            _ => return Err("неизвестный формат ключа".into()),
        };
        Ok(Vault { key, dir: dir.to_path_buf() })
    }

    fn path(&self, name: &str) -> Result<PathBuf, String> {
        if !valid_name(name) {
            return Err("недопустимое имя секрета".into());
        }
        Ok(self.dir.join(format!("{name}.nxs")))
    }

    pub fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let p = self.path(name)?;
        let mut out = MAGIC.to_vec();
        out.extend(aes_seal(&self.key, name.as_bytes(), data)?);
        write_atomic(&p, &out)
    }

    pub fn get(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let p = self.path(name)?;
        let raw = match std::fs::read(&p) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        if raw.len() < 4 || &raw[..4] != MAGIC {
            return Err("неизвестный формат файла".into());
        }
        aes_open(&self.key, name.as_bytes(), &raw[4..]).map(Some)
    }

    pub fn delete(&self, name: &str) -> Result<(), String> {
        let p = self.path(name)?;
        match std::fs::remove_file(p) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nexora-secure-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn aes_roundtrip_tamper_aad() {
        let k = [7u8; 32];
        let c = aes_seal(&k, b"a", b"secret").unwrap();
        assert_eq!(aes_open(&k, b"a", &c).unwrap(), b"secret");
        assert!(aes_open(&k, b"b", &c).is_err()); // другое имя
        assert!(aes_open(&[8u8; 32], b"a", &c).is_err()); // другой ключ
        let mut t = c.clone();
        *t.last_mut().unwrap() ^= 1;
        assert!(aes_open(&k, b"a", &t).is_err()); // изменённый файл
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip() {
        let p = dpapi::protect(b"hello").unwrap();
        assert_ne!(p, b"hello");
        assert_eq!(dpapi::unprotect(&p).unwrap(), b"hello");
        let mut bad = p.clone();
        let n = bad.len() / 2;
        bad[n] ^= 1;
        assert!(dpapi::unprotect(&bad).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn vault_no_password() {
        let d = tmp("np");
        let v = Vault::open(&d, None).unwrap();
        assert_eq!(Vault::status(&d), Some(false));
        v.put("totp_secret", b"JBSWY3DP").unwrap();
        drop(v);
        let v2 = Vault::open(&d, None).unwrap();
        assert_eq!(v2.get("totp_secret").unwrap().unwrap(), b"JBSWY3DP");
        assert_eq!(v2.get("missing").unwrap(), None);
        // на диске нет открытого текста
        let raw = std::fs::read(d.join("totp_secret.nxs")).unwrap();
        assert!(!raw.windows(8).any(|w| w == b"JBSWY3DP"));
        // файл, подложенный под другим именем, не открывается
        std::fs::copy(d.join("totp_secret.nxs"), d.join("other.nxs")).unwrap();
        assert!(v2.get("other").is_err());
        v2.delete("totp_secret").unwrap();
        assert_eq!(v2.get("totp_secret").unwrap(), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(windows)]
    #[test]
    fn vault_with_password() {
        let d = tmp("pw");
        let v = Vault::open(&d, Some("правильный пароль")).unwrap();
        assert_eq!(Vault::status(&d), Some(true));
        v.put("k", b"data").unwrap();
        drop(v);
        assert!(Vault::open(&d, None).is_err());
        assert!(Vault::open(&d, Some("неверный")).is_err());
        let v = Vault::open(&d, Some("правильный пароль")).unwrap();
        assert_eq!(v.get("k").unwrap().unwrap(), b"data");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn names_are_validated() {
        assert!(!valid_name("../x"));
        assert!(!valid_name(""));
        assert!(!valid_name("a b"));
        assert!(valid_name("device-1_cert"));
    }
}
