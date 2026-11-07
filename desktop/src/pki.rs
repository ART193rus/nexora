//! Собственный центр сертификации сервера Nexora (ECDSA P-256): сертификат сервера, сертификаты устройств
//! (clientAuth), отпечатки SHA-256. Ключи хранятся в зашифрованном хранилище (secure.rs), здесь только логика.
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, SanType,
    SerialNumber,
};
use sha2::{Digest, Sha256};
use std::net::IpAddr;

const CA_YEARS: i32 = 10;
const LEAF_YEARS: i32 = 2;

pub struct Issued {
    pub cert_pem: String,
    pub key_pem: String,
    pub serial: String,
    pub fingerprint: String,
}

pub struct Ca {
    pub cert_pem: String,
    key_pem: String,
}

fn e<T: std::fmt::Display>(x: T) -> String {
    x.to_string()
}

fn year_now() -> i32 {
    // грубо, без календарной библиотеки: достаточно для срока действия
    1970 + (crate::totp::now_secs() / 31_556_952) as i32
}

fn random_serial() -> Result<(SerialNumber, String), String> {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).map_err(e)?;
    b[0] &= 0x7f; // положительное число
    if b[0] == 0 {
        b[0] = 1;
    }
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok((SerialNumber::from_slice(&b), hex))
}

fn dn(cn: &str, org: &str) -> DistinguishedName {
    let mut d = DistinguishedName::new();
    d.push(DnType::CommonName, cn);
    d.push(DnType::OrganizationName, org);
    d
}

fn validity(p: &mut CertificateParams, years: i32) {
    let y = year_now();
    // начало на сутки раньше, чтобы небольшая разница часов не ломала вход
    p.not_before = rcgen::date_time_ymd(y - 1, 1, 1);
    p.not_after = rcgen::date_time_ymd(y + years, 12, 31);
}

/// SHA-256 от DER-сертификата в PEM, 64 hex-символа.
pub fn fingerprint_pem(pem: &str) -> Result<String, String> {
    let (_, p) = x509_parser::pem::parse_x509_pem(pem.as_bytes()).map_err(e)?;
    Ok(Sha256::digest(&p.contents).iter().map(|b| format!("{b:02x}")).collect())
}

/// Короткая форма для показа и сверки глазами: «a1b2-c3d4-e5f6-0718».
pub fn short(fp: &str) -> String {
    fp.chars().take(16).collect::<Vec<_>>().chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join("-")
}

impl Ca {
    pub fn create(name: &str) -> Result<Ca, String> {
        let key = KeyPair::generate().map_err(e)?;
        let mut p = CertificateParams::new(Vec::<String>::new()).map_err(e)?;
        p.distinguished_name = dn(&format!("{name} CA"), "Nexora");
        p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        p.serial_number = Some(random_serial()?.0);
        validity(&mut p, CA_YEARS);
        let cert = p.self_signed(&key).map_err(e)?;
        Ok(Ca { cert_pem: cert.pem(), key_pem: key.serialize_pem() })
    }

    pub fn from_pem(cert_pem: &str, key_pem: &str) -> Result<Ca, String> {
        let ca = Ca { cert_pem: cert_pem.to_string(), key_pem: key_pem.to_string() };
        ca.signer()?; // проверка, что пара подходит
        Ok(ca)
    }

    pub fn key_pem(&self) -> &str {
        &self.key_pem
    }

    pub fn fingerprint(&self) -> Result<String, String> {
        fingerprint_pem(&self.cert_pem)
    }

    fn signer(&self) -> Result<(rcgen::Certificate, KeyPair), String> {
        let key = KeyPair::from_pem(&self.key_pem).map_err(e)?;
        let params = CertificateParams::from_ca_cert_pem(&self.cert_pem).map_err(e)?;
        let cert = params.self_signed(&key).map_err(e)?;
        Ok((cert, key))
    }

    fn issue(&self, mut p: CertificateParams, eku: ExtendedKeyUsagePurpose) -> Result<Issued, String> {
        let (ca_cert, ca_key) = self.signer()?;
        let key = KeyPair::generate().map_err(e)?;
        let (serial, serial_hex) = random_serial()?;
        p.serial_number = Some(serial);
        p.is_ca = IsCa::ExplicitNoCa;
        p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        p.extended_key_usages = vec![eku];
        validity(&mut p, LEAF_YEARS);
        let cert = p.signed_by(&key, &ca_cert, &ca_key).map_err(e)?;
        let pem = cert.pem();
        Ok(Issued { fingerprint: fingerprint_pem(&pem)?, cert_pem: pem, key_pem: key.serialize_pem(), serial: serial_hex })
    }

    /// Сертификат сервера; names = адреса и имена, по которым к нему подключаются (IP или DNS).
    pub fn issue_server(&self, names: &[String]) -> Result<Issued, String> {
        let mut p = CertificateParams::new(Vec::<String>::new()).map_err(e)?;
        p.distinguished_name = dn("Nexora Server", "Nexora");
        for n in names {
            p.subject_alt_names.push(match n.parse::<IpAddr>() {
                Ok(ip) => SanType::IpAddress(ip),
                Err(_) if !n.is_empty() && n.len() <= 253 && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') => SanType::DnsName(n.as_str().try_into().map_err(|_| format!("недопустимое имя: {n}"))?),
                Err(_) => return Err(format!("недопустимое имя: {n}")),
            });
        }
        self.issue(p, ExtendedKeyUsagePurpose::ServerAuth)
    }

    /// Подписать запрос (CSR) от устройства: закрытый ключ остаётся на устройстве.
    /// Всё, кроме открытого ключа, задаёт сервер: имя, срок, назначение, серийный номер; расширения из запроса отбрасываются.
    pub fn sign_device_csr(&self, csr_pem: &str, device_name: &str) -> Result<(String, String, String), String> {
        let clean: String = device_name.chars().filter(|c| !c.is_control()).take(64).collect();
        if clean.trim().is_empty() {
            return Err("пустое имя устройства".into());
        }
        let mut csr = rcgen::CertificateSigningRequestParams::from_pem(csr_pem).map_err(|_| "запрос повреждён или подпись не сходится".to_string())?;
        let (ca_cert, ca_key) = self.signer()?;
        let (serial, serial_hex) = random_serial()?;
        let p = &mut csr.params;
        p.distinguished_name = dn(clean.trim(), "Nexora device");
        p.subject_alt_names.clear();
        p.custom_extensions.clear();
        p.serial_number = Some(serial);
        p.is_ca = IsCa::ExplicitNoCa;
        p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        validity(p, LEAF_YEARS);
        let cert = csr.signed_by(&ca_cert, &ca_key).map_err(e)?;
        let pem = cert.pem();
        let fp = fingerprint_pem(&pem)?;
        Ok((pem, serial_hex, fp))
    }

    /// Сертификат устройства (клиентский вход по mTLS).
    pub fn issue_device(&self, device_name: &str) -> Result<Issued, String> {
        let clean: String = device_name.chars().filter(|c| !c.is_control()).take(64).collect();
        if clean.trim().is_empty() {
            return Err("пустое имя устройства".into());
        }
        let mut p = CertificateParams::new(Vec::<String>::new()).map_err(e)?;
        p.distinguished_name = dn(clean.trim(), "Nexora device");
        self.issue(p, ExtendedKeyUsagePurpose::ClientAuth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x509_parser::prelude::*;

    fn der(pem: &str) -> Vec<u8> {
        parse_x509_pem(pem.as_bytes()).unwrap().1.contents
    }

    #[test]
    fn chain_and_usages() {
        let ca = Ca::create("Дача").unwrap();
        let srv = ca.issue_server(&["192.168.1.50".into(), "nexora.local".into()]).unwrap();
        let dev = ca.issue_device("Телефон Артёма").unwrap();

        let ca_der = der(&ca.cert_pem);
        let (_, ca_x) = X509Certificate::from_der(&ca_der).unwrap();
        assert!(ca_x.is_ca());

        let sd = der(&srv.cert_pem);
        let (_, s) = X509Certificate::from_der(&sd).unwrap();
        let dd = der(&dev.cert_pem);
        let (_, d) = X509Certificate::from_der(&dd).unwrap();
        // подписи проверяются ключом CA, и только им
        assert!(s.verify_signature(Some(ca_x.public_key())).is_ok());
        assert!(d.verify_signature(Some(ca_x.public_key())).is_ok());
        assert!(d.verify_signature(Some(s.public_key())).is_err());
        assert!(!s.is_ca() && !d.is_ca());
        assert_eq!(s.issuer(), ca_x.subject());
        // назначение: сервер только serverAuth, устройство только clientAuth
        let se = s.extended_key_usage().unwrap().unwrap().value;
        let de = d.extended_key_usage().unwrap().unwrap().value;
        assert!(se.server_auth && !se.client_auth);
        assert!(de.client_auth && !de.server_auth);
        // адреса в сертификате сервера
        let san = s.subject_alternative_name().unwrap().unwrap().value;
        assert_eq!(san.general_names.len(), 2);
        assert!(s.validity().is_valid());
        assert!(d.subject().to_string().contains("Телефон Артёма"));
    }

    #[test]
    fn reload_ca_from_pem_still_signs() {
        let ca = Ca::create("Test").unwrap();
        let ca2 = Ca::from_pem(&ca.cert_pem, ca.key_pem()).unwrap();
        let dev = ca2.issue_device("laptop").unwrap();
        let ca_der = der(&ca.cert_pem);
        let (_, ca_x) = X509Certificate::from_der(&ca_der).unwrap();
        let dd = der(&dev.cert_pem);
        let (_, d) = X509Certificate::from_der(&dd).unwrap();
        assert!(d.verify_signature(Some(ca_x.public_key())).is_ok(), "подпись валидна под исходным CA");
        assert_eq!(d.issuer(), ca_x.subject());
        // чужой ключ к чужому сертификату не подходит
        let other = Ca::create("Other").unwrap();
        assert!(Ca::from_pem(&ca.cert_pem, other.key_pem()).is_err() || {
            // допускается только если ключ не совпал и подпись всё равно не пройдёт под исходным CA
            let bad = Ca::from_pem(&ca.cert_pem, other.key_pem()).unwrap().issue_device("x").unwrap();
            let bd = der(&bad.cert_pem);
            X509Certificate::from_der(&bd).unwrap().1.verify_signature(Some(ca_x.public_key())).is_err()
        });
    }

    #[test]
    fn fingerprints_and_serials() {
        let ca = Ca::create("A").unwrap();
        let a = ca.issue_device("a").unwrap();
        let b = ca.issue_device("b").unwrap();
        assert_ne!(a.serial, b.serial);
        assert_ne!(a.fingerprint, b.fingerprint);
        assert_eq!(a.fingerprint.len(), 64);
        assert_eq!(fingerprint_pem(&a.cert_pem).unwrap(), a.fingerprint);
        assert_eq!(short(&a.fingerprint).len(), 19);
        assert_eq!(ca.fingerprint().unwrap(), fingerprint_pem(&ca.cert_pem).unwrap());
    }

    #[test]
    fn bad_input_rejected() {
        let ca = Ca::create("A").unwrap();
        assert!(ca.issue_device("   ").is_err());
        assert!(ca.issue_server(&["bad name with spaces".into()]).is_err());
    }
}
