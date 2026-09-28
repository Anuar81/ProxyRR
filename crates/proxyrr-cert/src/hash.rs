//! `subject_hash_old`: el hash que usa Android para nombrar los certificados del sistema.

use md5::{Digest, Md5};

use crate::Result;
use crate::ca::parse;

/// Igual que `openssl x509 -subject_hash_old`: MD5 del DER del subject,
/// primeros 4 bytes leídos little-endian.
///
/// # Errors
/// Si `cert_der` no es un certificado X.509 válido.
pub fn subject_hash_old(cert_der: &[u8]) -> Result<u32> {
    let x509 = parse(cert_der)?;
    let digest = Md5::digest(x509.subject().as_raw());
    Ok(u32::from_le_bytes([
        digest[0], digest[1], digest[2], digest[3],
    ]))
}

/// Nombre de archivo del certificado en `/system/etc/security/cacerts` (p. ej. `1a2b3c4d.0`).
///
/// # Errors
/// Si `cert_der` no es un certificado X.509 válido.
pub fn android_cert_filename(cert_der: &[u8]) -> Result<String> {
    Ok(format!("{:08x}.0", subject_hash_old(cert_der)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CA fija generada una vez; el valor esperado sale de
    /// `openssl x509 -in tests/fixtures/ca.pem -noout -subject_hash_old`.
    const FIXTURE_PEM: &str = include_str!("../tests/fixtures/ca.pem");
    const FIXTURE_HASH_OLD: &str = include_str!("../tests/fixtures/ca.subject_hash_old");

    #[test]
    fn matches_openssl_vector() {
        let (_, pem) = x509_parser::pem::parse_x509_pem(FIXTURE_PEM.as_bytes()).unwrap();
        let name = android_cert_filename(&pem.contents).unwrap();
        assert_eq!(name, format!("{}.0", FIXTURE_HASH_OLD.trim()));
    }
}
