//! Certificados hoja firmados por la CA.

use std::fmt;

use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use time::{Duration, OffsetDateTime};

use crate::{Error, Result, random_serial};

/// Validez total máxima que aceptan Chrome/Safari/WebView de Android.
pub(crate) const MAX_LEAF_VALIDITY_DAYS: i64 = 397;
/// Margen hacia atrás para clientes con el reloj adelantado/atrasado.
const BACKDATE_DAYS: i64 = 1;

/// Certificado hoja para un host, listo para servir por TLS.
#[derive(Clone, PartialEq, Eq)]
pub struct LeafCert {
    /// Host normalizado para el que se emitió.
    pub host: String,
    /// Certificado en DER.
    pub cert_der: Vec<u8>,
    /// Clave privada en DER PKCS#8.
    pub key_der: Vec<u8>,
}

impl fmt::Debug for LeafCert {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeafCert")
            .field("host", &self.host)
            .field("cert_der_len", &self.cert_der.len())
            .finish_non_exhaustive()
    }
}

/// Normaliza un host: sin espacios, sin punto final, sin corchetes IPv6, en minúsculas.
pub(crate) fn normalize_host(host: &str) -> Result<String> {
    let trimmed = host.trim().trim_end_matches('.');
    let h = trimmed
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .unwrap_or(trimmed);
    let invalid = h.is_empty()
        || h.len() > 253
        || h.chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '\\' | '@'));
    if invalid {
        return Err(Error::InvalidHost(host.to_owned()));
    }
    Ok(h.to_ascii_lowercase())
}

pub(crate) fn issue(issuer: &Issuer<'_, KeyPair>, host: &str) -> Result<LeafCert> {
    let host = normalize_host(host)?;

    // `new` decide solo si el SAN es IP o DNS.
    let mut params =
        CertificateParams::new(vec![host.clone()]).map_err(|_| Error::InvalidHost(host.clone()))?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, host.clone());
    params.distinguished_name = dn;
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.use_authority_key_identifier_extension = true;
    let now = OffsetDateTime::now_utc();
    params.not_before = now - Duration::days(BACKDATE_DAYS);
    params.not_after = now + Duration::days(MAX_LEAF_VALIDITY_DAYS - BACKDATE_DAYS);
    params.serial_number = Some(random_serial()?);

    let key = KeyPair::generate()?;
    let cert = params.signed_by(&key, issuer)?;
    Ok(LeafCert {
        host,
        cert_der: cert.der().to_vec(),
        key_der: key.serialize_der(),
    })
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use x509_parser::extensions::GeneralName;

    use super::*;
    use crate::CertificateAuthority;
    use crate::ca::parse;

    fn sans(der: &[u8]) -> Vec<String> {
        let x509 = parse(der).unwrap();
        let ext = x509.subject_alternative_name().unwrap().expect("SAN");
        ext.value
            .general_names
            .iter()
            .map(|n| match n {
                GeneralName::DNSName(d) => format!("dns:{d}"),
                GeneralName::IPAddress(ip) => {
                    let addr = match ip.len() {
                        4 => IpAddr::from(<[u8; 4]>::try_from(*ip).unwrap()),
                        16 => IpAddr::from(<[u8; 16]>::try_from(*ip).unwrap()),
                        _ => panic!("IP de largo inválido"),
                    };
                    format!("ip:{addr}")
                }
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn leaf_is_signed_by_ca() {
        let ca = CertificateAuthority::generate().unwrap();
        let leaf = ca.issue_leaf("api.example.com").unwrap();
        let ca_x509 = parse(ca.cert_der()).unwrap();
        let leaf_x509 = parse(&leaf.cert_der).unwrap();
        leaf_x509
            .verify_signature(Some(ca_x509.public_key()))
            .expect("firma de la CA");
        assert_eq!(leaf_x509.issuer(), ca_x509.subject());
    }

    #[test]
    fn leaf_has_expected_extensions() {
        let ca = CertificateAuthority::generate().unwrap();
        let leaf = ca.issue_leaf("API.Example.com.").unwrap();
        assert_eq!(leaf.host, "api.example.com");
        assert_eq!(sans(&leaf.cert_der), vec!["dns:api.example.com"]);

        let x509 = parse(&leaf.cert_der).unwrap();
        let bc = x509.basic_constraints().unwrap().expect("basicConstraints");
        assert!(!bc.value.ca);
        let eku = x509.extended_key_usage().unwrap().expect("EKU");
        assert!(eku.value.server_auth);
        // La clave es PKCS#8 válida.
        KeyPair::try_from(leaf.key_der.as_slice()).expect("PKCS#8");
    }

    #[test]
    fn ip_host_uses_ip_san() {
        let ca = CertificateAuthority::generate().unwrap();
        let v4 = ca.issue_leaf("10.0.2.2").unwrap();
        assert_eq!(
            sans(&v4.cert_der),
            vec![format!("ip:{}", Ipv4Addr::new(10, 0, 2, 2))]
        );
        let v6 = ca.issue_leaf("[::1]").unwrap();
        assert_eq!(
            sans(&v6.cert_der),
            vec![format!("ip:{}", Ipv6Addr::LOCALHOST)]
        );
    }

    #[test]
    fn wildcard_host_is_accepted() {
        let ca = CertificateAuthority::generate().unwrap();
        let leaf = ca.issue_leaf("*.example.com").unwrap();
        assert_eq!(sans(&leaf.cert_der), vec!["dns:*.example.com"]);
    }

    #[test]
    fn validity_within_397_days() {
        let ca = CertificateAuthority::generate().unwrap();
        let leaf = ca.issue_leaf("example.com").unwrap();
        let x509 = parse(&leaf.cert_der).unwrap();
        let from = x509.validity().not_before.to_datetime();
        let to = x509.validity().not_after.to_datetime();
        assert!(to - from <= Duration::days(MAX_LEAF_VALIDITY_DAYS));
        assert!(from < OffsetDateTime::now_utc());
    }

    #[test]
    fn serials_are_unique() {
        let ca = CertificateAuthority::generate().unwrap();
        let a = parse(&ca.issue_leaf("a.com").unwrap().cert_der)
            .unwrap()
            .raw_serial()
            .to_vec();
        let b = parse(&ca.issue_leaf("a.com").unwrap().cert_der)
            .unwrap()
            .raw_serial()
            .to_vec();
        assert_ne!(a, b);
    }

    #[test]
    fn invalid_hosts_are_rejected() {
        let ca = CertificateAuthority::generate().unwrap();
        for bad in ["", "   ", "a b.com", "evil.com/path", "user@host", "."] {
            assert!(
                matches!(ca.issue_leaf(bad), Err(Error::InvalidHost(_))),
                "{bad:?} debería rechazarse"
            );
        }
    }
}
