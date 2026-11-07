//! TOTP (RFC 6238, HMAC-SHA1, 6 цифр, шаг 30 с) для привязки устройств через Authenticator.
//! Чистая логика без ввода-вывода: секрет, проверка кода, защита от повтора и перебора.

use hmac::{Hmac, Mac};
use sha1::Sha1;

const STEP: u64 = 30;
const DIGITS: u32 = 6;
const MAX_FAILS: u32 = 5;
const LOCK_SECS: u64 = 15 * 60;

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for &b in data {
        buf = (buf << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            out.push(B32[((buf >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(B32[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in s.chars().filter(|c| !matches!(c, ' ' | '-' | '=')) {
        let v = B32.iter().position(|&x| x as char == c.to_ascii_uppercase())? as u32;
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            out.push((buf >> (bits - 8)) as u8);
            bits -= 8;
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Новый случайный секрет, 20 байт (160 бит), в base32.
pub fn new_secret() -> Result<String, String> {
    let mut b = [0u8; 20];
    getrandom::getrandom(&mut b).map_err(|e| e.to_string())?;
    Ok(base32_encode(&b))
}

/// Код для заданного шага счётчика.
pub fn code_at(secret: &[u8], counter: u64) -> String {
    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(secret).expect("hmac key");
    mac.update(&counter.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let off = (h[19] & 0xf) as usize;
    let bin = ((h[off] as u32 & 0x7f) << 24)
        | ((h[off + 1] as u32) << 16)
        | ((h[off + 2] as u32) << 8)
        | h[off + 3] as u32;
    format!("{:0width$}", bin % 10u32.pow(DIGITS), width = DIGITS as usize)
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// Ссылка для QR-кода (формат Google Authenticator).
pub fn otpauth_uri(secret_b32: &str, account: &str, issuer: &str) -> String {
    fn enc(s: &str) -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                _ => format!("%{:02X}", b),
            })
            .collect()
    }
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={}&period={}",
        enc(issuer), enc(account), secret_b32, enc(issuer), DIGITS, STEP
    )
}

/// Проверяющий: окно ±1 шаг, один и тот же шаг принимается один раз, 5 ошибок = блокировка на 15 минут.
pub struct Verifier {
    secret: Vec<u8>,
    last_used: Option<u64>,
    fails: u32,
    locked_until: u64,
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Ok,
    Wrong,
    Replay,
    Locked(u64),
}

impl Verifier {
    pub fn new(secret_b32: &str) -> Option<Self> {
        let secret = base32_decode(secret_b32)?;
        if secret.len() < 10 {
            return None;
        }
        Some(Self { secret, last_used: None, fails: 0, locked_until: 0 })
    }

    pub fn check(&mut self, code: &str, now: u64) -> Verdict {
        if now < self.locked_until {
            return Verdict::Locked(self.locked_until - now);
        }
        let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
        let cur = now / STEP;
        let mut hit = None;
        // перебираем все три шага без раннего выхода, чтобы время ответа не зависело от попадания
        for c in [cur.saturating_sub(1), cur, cur + 1] {
            if ct_eq(&code_at(&self.secret, c), &code) && hit.is_none() {
                hit = Some(c);
            }
        }
        match hit {
            Some(c) if self.last_used.map_or(true, |l| c > l) => {
                self.last_used = Some(c);
                self.fails = 0;
                Verdict::Ok
            }
            Some(_) => Verdict::Replay,
            None => {
                self.fails += 1;
                if self.fails >= MAX_FAILS {
                    self.fails = 0;
                    self.locked_until = now + LOCK_SECS;
                    return Verdict::Locked(LOCK_SECS);
                }
                Verdict::Wrong
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 6238, приложение B (SHA1, секрет "12345678901234567890"), берём 6 младших цифр 8-значных кодов
    #[test]
    fn rfc6238_vectors() {
        let k = b"12345678901234567890";
        for (t, want) in [
            (59u64, "287082"),
            (1111111109, "081804"),
            (1111111111, "050471"),
            (1234567890, "005924"),
            (2000000000, "279037"),
        ] {
            assert_eq!(code_at(k, t / 30), want, "t={t}");
        }
    }

    #[test]
    fn base32_roundtrip_and_rfc4648() {
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32_decode("mzxw 6ytb-oi").unwrap(), b"foobar");
        assert!(base32_decode("11!").is_none());
        let s = new_secret().unwrap();
        assert_eq!(s.len(), 32);
        assert_eq!(base32_encode(&base32_decode(&s).unwrap()), s);
    }

    fn v() -> Verifier {
        Verifier::new(&base32_encode(b"12345678901234567890")).unwrap()
    }

    #[test]
    fn window_and_replay() {
        let k = b"12345678901234567890";
        let now = 1_000_000_020u64;
        let mut a = v();
        assert_eq!(a.check(&code_at(k, now / 30 - 1), now), Verdict::Ok); // предыдущий шаг
        assert_eq!(a.check(&code_at(k, now / 30 - 1), now), Verdict::Replay); // тот же код повторно
        assert_eq!(a.check(&code_at(k, now / 30), now), Verdict::Ok); // следующий шаг можно
        assert_eq!(a.check(&code_at(k, now / 30 - 1), now), Verdict::Replay); // назад нельзя
        let mut b = v();
        assert_eq!(b.check(&code_at(k, now / 30 + 2), now), Verdict::Wrong); // вне окна
        assert_eq!(b.check(&format!(" {} ", code_at(k, now / 30)), now), Verdict::Ok); // пробелы допустимы
    }

    #[test]
    fn lockout_after_five_fails() {
        let k = b"12345678901234567890";
        let now = 1_000_000_020u64;
        let mut a = v();
        for _ in 0..4 {
            assert_eq!(a.check("000000", now), Verdict::Wrong);
        }
        assert!(matches!(a.check("000000", now), Verdict::Locked(_)));
        // верный код во время блокировки не принимается
        assert!(matches!(a.check(&code_at(k, now / 30), now + 10), Verdict::Locked(_)));
        // после срока снова работает
        assert_eq!(a.check(&code_at(k, (now + 901) / 30), now + 901), Verdict::Ok);
    }

    #[test]
    fn uri_format() {
        let u = otpauth_uri("ABC", "дача@pc", "Nexora Server");
        assert!(u.starts_with("otpauth://totp/Nexora%20Server:"));
        assert!(u.contains("secret=ABC") && u.contains("period=30"));
    }
}
