//! Configuración del proxy.

use std::net::{Ipv4Addr, SocketAddr};

/// Puerto por defecto (el mismo que usan otras herramientas del rubro, así las guías coinciden).
pub const DEFAULT_PORT: u16 = 9090;

/// Configuración de una instancia del proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyConfig {
    /// Dirección de escucha. Por defecto solo loopback: exponer el proxy a la red es una decisión
    /// explícita del usuario (hace falta para dispositivos físicos).
    pub listen: SocketAddr,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_PORT)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_listens_on_loopback_only() {
        let config = ProxyConfig::default();
        assert!(config.listen.ip().is_loopback());
        assert_eq!(config.listen.port(), DEFAULT_PORT);
    }
}
