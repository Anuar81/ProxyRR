//! Configuración del proxy.

use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use proxyrr_cert::CertificateAuthority;

use crate::capture::DEFAULT_MAX_BODY_CAPTURE;

/// Puerto por defecto (el mismo que usan otras herramientas del rubro, así las guías coinciden).
pub const DEFAULT_PORT: u16 = 9090;

/// Configuración de una instancia del proxy.
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Dirección de escucha. Por defecto solo loopback: exponer el proxy a la red es una decisión
    /// explícita del usuario (hace falta para dispositivos físicos).
    pub listen: SocketAddr,
    /// Descifrado HTTPS. `None`: los `CONNECT` son túneles opacos (spec 0004).
    pub mitm: Option<MitmConfig>,
    /// Certificados raíz (DER) extra en los que confiar al hablar con orígenes HTTPS, además de los del
    /// SO. Útil para una CA corporativa o un servidor de desarrollo con CA propia.
    pub upstream_roots: Vec<Vec<u8>>,
    /// Bytes máximos que se guardan de cada body (el resto se reenvía igual, sin guardar).
    pub max_body_capture: usize,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_PORT)),
            mitm: None,
            upstream_roots: Vec::new(),
            max_body_capture: DEFAULT_MAX_BODY_CAPTURE,
        }
    }
}

/// Configuración del descifrado HTTPS.
#[derive(Clone)]
pub struct MitmConfig {
    /// CA que firma los certificados hoja presentados a los clientes.
    pub ca: Arc<CertificateAuthority>,
    /// Hosts que NO se descifran: `example.com` exacto o `*.example.com` para sus subdominios.
    pub bypass: Vec<String>,
}

impl MitmConfig {
    /// Descifra todo con `ca`, sin bypass.
    #[must_use]
    pub fn new(ca: Arc<CertificateAuthority>) -> Self {
        Self {
            ca,
            bypass: Vec::new(),
        }
    }
}

impl fmt::Debug for MitmConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MitmConfig")
            .field("bypass", &self.bypass)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_listens_on_loopback_only_without_mitm() {
        let config = ProxyConfig::default();
        assert!(config.listen.ip().is_loopback());
        assert_eq!(config.listen.port(), DEFAULT_PORT);
        assert!(config.mitm.is_none());
    }
}
