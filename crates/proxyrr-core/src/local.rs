//! Sitios que responde el propio proxy, sin reenviar a internet (spec 0006, CA 11–12).
//!
//! El motor no sabe qué contienen: solo reconoce los requests que les corresponden y le pasa cada uno
//! a un [`LocalSite`]. La página de la CA (`http://proxyrr.cert/`) vive en `proxyrr-devices`.

use std::fmt;

use bytes::Bytes;

/// Host reservado que el proxy responde él mismo (por HTTP plano, o HTTPS con MITM).
pub const LOCAL_HOST: &str = "proxyrr.cert";
/// Prefijo de ruta para pedir el sitio directo al proxy (`http://<ip>:<puerto>/cert`), antes de
/// configurar el proxy en el dispositivo.
pub const DIRECT_PREFIX: &str = "/cert";

/// Un request dirigido al sitio local.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalRequest<'a> {
    /// `true` si llegó directo al proxy (`/cert…`); `false` si vino como `proxyrr.cert` a través del proxy.
    pub direct: bool,
    /// Ruta dentro del sitio, siempre empieza con `/`. En los directos ya viene sin el prefijo `/cert`.
    pub path: &'a str,
    /// `User-Agent` del cliente, para adaptar las instrucciones.
    pub user_agent: Option<&'a str>,
}

/// Respuesta del sitio local.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalResponse {
    /// Status HTTP.
    pub status: u16,
    /// Headers (se agregan a `Content-Length`).
    pub headers: Vec<(String, String)>,
    /// Body.
    pub body: Bytes,
}

/// Sitio servido por el proxy. `respond` corre en el hilo de la conexión: tiene que ser rápido.
pub trait LocalSite: Send + Sync + fmt::Debug {
    /// Responde el request.
    fn respond(&self, request: &LocalRequest<'_>) -> LocalResponse;
}

/// Si el request va al sitio local, su ruta dentro del sitio y si llegó directo.
pub(crate) fn route<'a>(
    host: Option<&str>,
    has_scheme: bool,
    path: &'a str,
) -> Option<(bool, &'a str)> {
    match host {
        Some(host) if host.eq_ignore_ascii_case(LOCAL_HOST) => Some((false, path)),
        Some(_) => None,
        None if has_scheme => None,
        None => {
            let rest = path.strip_prefix(DIRECT_PREFIX)?;
            if rest.is_empty() {
                Some((true, "/"))
            } else if rest.starts_with('/') {
                Some((true, rest))
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::route;

    #[test]
    fn routes_only_the_local_site() {
        assert_eq!(route(Some("proxyrr.cert"), true, "/"), Some((false, "/")));
        assert_eq!(
            route(Some("PROXYRR.CERT"), true, "/ca.pem"),
            Some((false, "/ca.pem"))
        );
        assert_eq!(route(Some("example.com"), true, "/cert"), None);
        assert_eq!(route(None, false, "/cert"), Some((true, "/")));
        assert_eq!(route(None, false, "/cert/ca.crt"), Some((true, "/ca.crt")));
        assert_eq!(route(None, false, "/certificado"), None);
        assert_eq!(route(None, false, "/"), None);
    }
}
