//! Certificados de ProxyRR.
//!
//! CA raíz propia y persistente, emisión al vuelo de certificados hoja por host
//! (validez ≤ 397 días) con caché LRU, export PEM/DER y `subject_hash_old` para el
//! store de Android. Spec: `specs/0003-ca-certificates`.

mod ca;
mod cache;
mod hash;
mod leaf;

use std::path::PathBuf;

pub use ca::{CA_CERT_FILE, CA_KEY_FILE, CaInfo, CertificateAuthority};
pub use cache::LeafCache;
pub use hash::{android_cert_filename, subject_hash_old};
pub use leaf::LeafCert;

/// Errores de `proxyrr-cert`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Falla al generar o firmar un certificado.
    #[error("error generando certificado: {0}")]
    Rcgen(#[from] rcgen::Error),

    /// Falla de E/S sobre un archivo concreto.
    #[error("error de E/S en {path}: {source}")]
    Io {
        /// Archivo involucrado.
        path: PathBuf,
        /// Error original.
        #[source]
        source: std::io::Error,
    },

    /// El certificado o la clave no se pueden interpretar.
    #[error("certificado o clave inválidos: {0}")]
    Invalid(String),

    /// La CA existe a medias (falta el certificado o la clave).
    #[error("la CA está incompleta: falta {0}")]
    Incomplete(PathBuf),

    /// La clave guardada no corresponde al certificado guardado.
    #[error("la clave de la CA no corresponde a su certificado ({0})")]
    KeyMismatch(PathBuf),

    /// El destino ya existe; nunca se sobrescribe una CA.
    #[error("ya existe {0}; no se sobrescribe")]
    AlreadyExists(PathBuf),

    /// Host vacío o con caracteres no válidos.
    #[error("host inválido: {0:?}")]
    InvalidHost(String),

    /// El generador de números aleatorios del SO falló.
    #[error("no se pudo obtener aleatoriedad del sistema")]
    Random,
}

/// `Result` de `proxyrr-cert`.
pub type Result<T> = std::result::Result<T, Error>;

/// Bytes aleatorios del SO.
pub(crate) fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut buf = [0u8; N];
    SystemRandom::new()
        .fill(&mut buf)
        .map_err(|_| Error::Random)?;
    Ok(buf)
}

/// Serial aleatorio de 16 bytes, positivo y sin cero inicial.
pub(crate) fn random_serial() -> Result<rcgen::SerialNumber> {
    let mut bytes = random_bytes::<16>()?;
    bytes[0] = (bytes[0] & 0x7f) | 0x01;
    Ok(rcgen::SerialNumber::from_slice(&bytes))
}
