//! Configuración del proxy.

use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use proxyrr_cert::CertificateAuthority;

use crate::capture::DEFAULT_MAX_BODY_CAPTURE;
use crate::hook::FlowHook;
use crate::local::LocalSite;

/// Puerto por defecto (el mismo que usan otras herramientas del rubro, así las guías coinciden).
pub const DEFAULT_PORT: u16 = 9090;
/// Timeout de conexión al origen por defecto: más que lo que espera un navegador antes de rendirse
/// con un sitio lento, para que el proxy no corte antes que el cliente (TD-001).
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

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
    /// No verificar el certificado de los orígenes HTTPS. Solo para desarrollo local: con esto
    /// cualquiera en el camino puede hacerse pasar por el origen.
    pub insecure_upstream: bool,
    /// Tiempo máximo para conectar con un origen (repartido entre sus IPs si tiene varias).
    pub connect_timeout: Duration,
    /// CA con la que se termina el TLS de `https://proxyrr.cert` cuando el MITM está apagado. Con
    /// MITM activo se usa la CA del MITM. `None`: sin MITM, `proxyrr.cert` por HTTPS no se sirve.
    pub local_site_ca: Option<Arc<CertificateAuthority>>,
    /// Bytes máximos que se guardan de cada body (el resto se reenvía igual, sin guardar).
    pub max_body_capture: usize,
    /// Contador de ids de flujo. Compartir el mismo entre varias instancias (reinicios) garantiza ids
    /// únicos aunque una instancia vieja siga cerrando conexiones.
    pub flow_ids: FlowIds,
    /// Sitio que el proxy responde él mismo (`http://proxyrr.cert/` y `/cert` directo). `None`: esos
    /// requests se tratan como cualquier otro.
    pub local_site: Option<Arc<dyn LocalSite>>,
    /// Reglas que modifican el tráfico (spec 0011). `None`: el tráfico pasa sin cambios.
    pub hook: Option<Arc<dyn FlowHook>>,
}

/// Contador de ids de flujo, compartible entre instancias del proxy. Empieza en 1.
#[derive(Debug, Clone)]
pub struct FlowIds(Arc<AtomicU64>);

impl Default for FlowIds {
    fn default() -> Self {
        Self(Arc::new(AtomicU64::new(1)))
    }
}

impl FlowIds {
    /// Reserva el siguiente id.
    pub(crate) fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_PORT)),
            mitm: None,
            upstream_roots: Vec::new(),
            insecure_upstream: false,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            local_site_ca: None,
            max_body_capture: DEFAULT_MAX_BODY_CAPTURE,
            flow_ids: FlowIds::default(),
            local_site: None,
            hook: None,
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
