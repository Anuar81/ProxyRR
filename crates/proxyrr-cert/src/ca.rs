//! CA raíz: generación, persistencia, export e información.

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::leaf::{self, LeafCert};
use crate::{Error, Result, hash, random_bytes, random_serial};

/// Nombre del archivo del certificado de la CA.
pub const CA_CERT_FILE: &str = "ca.pem";
/// Nombre del archivo de la clave privada de la CA.
pub const CA_KEY_FILE: &str = "ca.key.pem";

const CA_VALIDITY_DAYS: i64 = 3650;

/// CA raíz de ProxyRR. Firma los certificados hoja.
pub struct CertificateAuthority {
    issuer: Issuer<'static, KeyPair>,
    cert_pem: String,
    cert_der: Vec<u8>,
    key_pem: String,
}

impl fmt::Debug for CertificateAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // La clave privada nunca se imprime.
        f.debug_struct("CertificateAuthority")
            .field("cert_der_len", &self.cert_der.len())
            .finish_non_exhaustive()
    }
}

/// Datos visibles de la CA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaInfo {
    /// Subject en formato RFC 4514.
    pub subject: String,
    /// Inicio de validez.
    pub not_before: OffsetDateTime,
    /// Fin de validez.
    pub not_after: OffsetDateTime,
    /// Huella SHA-256 del DER, en hex mayúscula separada por `:`.
    pub sha256_fingerprint: String,
    /// `subject_hash_old` (nombre de archivo en Android).
    pub subject_hash_old: u32,
}

impl CertificateAuthority {
    /// Genera una CA nueva en memoria (no la guarda).
    ///
    /// # Errors
    /// Si falla la generación de la clave o del certificado.
    pub fn generate() -> Result<Self> {
        let key = KeyPair::generate()?;
        let id = hex::encode(random_bytes::<4>()?);

        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, format!("ProxyRR CA ({id})"));
        dn.push(DnType::OrganizationName, "ProxyRR");

        let mut params = CertificateParams::default();
        params.distinguished_name = dn;
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let now = OffsetDateTime::now_utc();
        params.not_before = now - Duration::days(1);
        params.not_after = now + Duration::days(CA_VALIDITY_DAYS);
        params.serial_number = Some(random_serial()?);

        let cert = params.self_signed(&key)?;
        Ok(Self {
            cert_pem: cert.pem(),
            cert_der: cert.der().to_vec(),
            key_pem: key.serialize_pem(),
            issuer: Issuer::new(params, key),
        })
    }

    /// Carga la CA de `dir`, o la genera y guarda si no existe ninguno de sus archivos.
    ///
    /// # Errors
    /// Si la CA está incompleta, corrupta o la clave no coincide (nunca se sobrescribe),
    /// o si falla la E/S.
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let (cert_path, key_path) = paths(dir);
        match (cert_path.exists(), key_path.exists()) {
            (false, false) => {
                let ca = Self::generate()?;
                ca.save(dir)?;
                Ok(ca)
            }
            _ => Self::load(dir),
        }
    }

    /// Carga la CA existente de `dir`.
    ///
    /// # Errors
    /// Si falta algún archivo, están corruptos, o la clave no corresponde al certificado.
    pub fn load(dir: &Path) -> Result<Self> {
        let (cert_path, key_path) = paths(dir);
        for p in [&cert_path, &key_path] {
            if !p.exists() {
                return Err(Error::Incomplete(p.clone()));
            }
        }
        let cert_pem = read(&cert_path)?;
        let key_pem = read(&key_path)?;
        Self::from_pem(&cert_pem, &key_pem, &cert_path)
    }

    fn from_pem(cert_pem: &str, key_pem: &str, origin: &Path) -> Result<Self> {
        let key = KeyPair::from_pem(key_pem).map_err(|e| Error::Invalid(format!("clave: {e}")))?;
        let cert_der = pem_to_der(cert_pem)?;
        {
            let x509 = parse(&cert_der)?;
            if !x509.is_ca() {
                return Err(Error::Invalid("el certificado no es una CA".into()));
            }
            if x509.public_key().subject_public_key.data.as_ref() != key.public_key_raw() {
                return Err(Error::KeyMismatch(origin.to_path_buf()));
            }
        }
        let issuer = Issuer::from_ca_cert_pem(cert_pem, key)?;
        Ok(Self {
            issuer,
            cert_pem: cert_pem.to_owned(),
            cert_der,
            key_pem: key_pem.to_owned(),
        })
    }

    /// Guarda la CA en `dir` (lo crea si hace falta). Nunca sobrescribe.
    ///
    /// # Errors
    /// Si alguno de los archivos ya existe o falla la E/S.
    pub fn save(&self, dir: &Path) -> Result<()> {
        let (cert_path, key_path) = paths(dir);
        for p in [&cert_path, &key_path] {
            if p.exists() {
                return Err(Error::AlreadyExists(p.clone()));
            }
        }
        fs::create_dir_all(dir).map_err(|source| io_err(dir, source))?;
        // Clave primero: si algo falla a mitad, queda "incompleta", nunca un cert huérfano válido.
        write_atomic(&key_path, self.key_pem.as_bytes(), true)?;
        write_atomic(&cert_path, self.cert_pem.as_bytes(), false)
    }

    /// Certificado de la CA en PEM.
    #[must_use]
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// Certificado de la CA en DER.
    #[must_use]
    pub fn cert_der(&self) -> &[u8] {
        &self.cert_der
    }

    /// Datos visibles de la CA.
    ///
    /// # Errors
    /// Si el certificado no se puede interpretar (no debería pasar con una CA válida).
    pub fn info(&self) -> Result<CaInfo> {
        let x509 = parse(&self.cert_der)?;
        let fingerprint = Sha256::digest(&self.cert_der)
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        Ok(CaInfo {
            subject: x509.subject().to_string(),
            not_before: x509.validity().not_before.to_datetime(),
            not_after: x509.validity().not_after.to_datetime(),
            sha256_fingerprint: fingerprint,
            subject_hash_old: hash::subject_hash_old(&self.cert_der)?,
        })
    }

    /// Emite un certificado hoja para `host`, firmado por esta CA. No usa caché
    /// (ver [`crate::LeafCache`]).
    ///
    /// # Errors
    /// Si el host es inválido o falla la firma.
    pub fn issue_leaf(&self, host: &str) -> Result<LeafCert> {
        leaf::issue(&self.issuer, host)
    }
}

fn paths(dir: &Path) -> (PathBuf, PathBuf) {
    (dir.join(CA_CERT_FILE), dir.join(CA_KEY_FILE))
}

fn io_err(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|source| io_err(path, source))
}

pub(crate) fn parse(der: &[u8]) -> Result<X509Certificate<'_>> {
    X509Certificate::from_der(der)
        .map(|(_, c)| c)
        .map_err(|e| Error::Invalid(format!("certificado: {e}")))
}

fn pem_to_der(pem: &str) -> Result<Vec<u8>> {
    let (_, parsed) = x509_parser::pem::parse_x509_pem(pem.as_bytes())
        .map_err(|e| Error::Invalid(format!("PEM: {e}")))?;
    if parsed.label != "CERTIFICATE" {
        return Err(Error::Invalid(format!("PEM inesperado: {}", parsed.label)));
    }
    Ok(parsed.contents)
}

/// Escribe a `<path>.tmp` y renombra, para no dejar archivos a medio escribir.
fn write_atomic(path: &Path, data: &[u8], private: bool) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = opts.open(&tmp).map_err(|source| io_err(&tmp, source))?;
    file.write_all(data)
        .and_then(|()| file.sync_all())
        .map_err(|source| io_err(&tmp, source))?;
    drop(file);
    fs::rename(&tmp, path).map_err(|source| io_err(path, source))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn generated_ca_has_ca_constraints() {
        let ca = CertificateAuthority::generate().unwrap();
        let x509 = parse(ca.cert_der()).unwrap();

        let bc = x509.basic_constraints().unwrap().expect("basicConstraints");
        assert!(bc.value.ca);
        assert_eq!(bc.value.path_len_constraint, Some(0));

        let ku = x509.key_usage().unwrap().expect("keyUsage");
        assert!(ku.value.key_cert_sign());
        assert!(ku.value.crl_sign());

        assert!(x509.subject().to_string().contains("ProxyRR CA ("));
        // Autofirmada.
        x509.verify_signature(None).expect("firma propia válida");
    }

    #[test]
    fn load_or_create_is_stable() {
        let dir = tmp();
        let a = CertificateAuthority::load_or_create(dir.path()).unwrap();
        let b = CertificateAuthority::load_or_create(dir.path()).unwrap();
        assert_eq!(a.cert_der(), b.cert_der());
        assert_eq!(
            a.info().unwrap().sha256_fingerprint,
            b.info().unwrap().sha256_fingerprint
        );
    }

    #[test]
    fn loaded_ca_can_still_issue() {
        let dir = tmp();
        CertificateAuthority::load_or_create(dir.path()).unwrap();
        let ca = CertificateAuthority::load(dir.path()).unwrap();
        let leaf = ca.issue_leaf("example.com").unwrap();
        let leaf_x509 = parse(&leaf.cert_der).unwrap();
        let ca_x509 = parse(ca.cert_der()).unwrap();
        leaf_x509
            .verify_signature(Some(ca_x509.public_key()))
            .expect("hoja firmada por la CA recargada");
    }

    #[test]
    fn names_are_unique() {
        let a = CertificateAuthority::generate().unwrap().info().unwrap();
        let b = CertificateAuthority::generate().unwrap().info().unwrap();
        assert_ne!(a.subject, b.subject);
    }

    #[cfg(unix)]
    #[test]
    fn key_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp();
        CertificateAuthority::load_or_create(dir.path()).unwrap();
        let mode = fs::metadata(dir.path().join(CA_KEY_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn corrupt_files_are_not_overwritten() {
        let dir = tmp();
        let cert = dir.path().join(CA_CERT_FILE);
        let key = dir.path().join(CA_KEY_FILE);
        fs::write(&cert, "basura").unwrap();
        fs::write(&key, "basura").unwrap();

        assert!(matches!(
            CertificateAuthority::load_or_create(dir.path()),
            Err(Error::Invalid(_))
        ));
        assert_eq!(fs::read_to_string(&cert).unwrap(), "basura");
        assert_eq!(fs::read_to_string(&key).unwrap(), "basura");
    }

    #[test]
    fn half_present_ca_is_incomplete() {
        let dir = tmp();
        fs::write(dir.path().join(CA_CERT_FILE), "x").unwrap();
        assert!(matches!(
            CertificateAuthority::load_or_create(dir.path()),
            Err(Error::Incomplete(_))
        ));
        assert!(!dir.path().join(CA_KEY_FILE).exists());
    }

    #[test]
    fn mismatched_key_is_rejected() {
        let a = CertificateAuthority::generate().unwrap();
        let b = CertificateAuthority::generate().unwrap();
        let dir = tmp();
        fs::write(dir.path().join(CA_CERT_FILE), a.cert_pem()).unwrap();
        fs::write(dir.path().join(CA_KEY_FILE), &b.key_pem).unwrap();
        assert!(matches!(
            CertificateAuthority::load(dir.path()),
            Err(Error::KeyMismatch(_))
        ));
    }

    #[test]
    fn save_never_overwrites() {
        let dir = tmp();
        CertificateAuthority::load_or_create(dir.path()).unwrap();
        let other = CertificateAuthority::generate().unwrap();
        assert!(matches!(
            other.save(dir.path()),
            Err(Error::AlreadyExists(_))
        ));
    }

    #[test]
    fn pem_and_der_match() {
        let ca = CertificateAuthority::generate().unwrap();
        assert!(ca.cert_pem().starts_with("-----BEGIN CERTIFICATE-----"));
        assert_eq!(pem_to_der(ca.cert_pem()).unwrap(), ca.cert_der());
    }

    #[test]
    fn debug_does_not_leak_key() {
        let ca = CertificateAuthority::generate().unwrap();
        assert!(!format!("{ca:?}").contains("PRIVATE KEY"));
    }
}
