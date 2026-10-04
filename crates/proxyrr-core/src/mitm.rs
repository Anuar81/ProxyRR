//! Piezas del descifrado HTTPS: bypass, certificados por host y replay de bytes ya leídos.

use std::io;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use lru::LruCache;
use proxyrr_cert::{CertificateAuthority, LeafCache};
use rustls::ServerConfig;
use rustls::crypto::CryptoProvider;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::config::MitmConfig;

/// Primer byte de un registro TLS de handshake.
pub(crate) const TLS_HANDSHAKE: u8 = 0x16;
/// ALPN ofrecido a clientes y orígenes: HTTP/2 llega en F5.
pub(crate) const ALPN_HTTP1: &[u8] = b"http/1.1";

/// Provider criptográfico explícito (`ring`), sin depender del default global del proceso.
pub(crate) fn crypto_provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Patrón de bypass: `host` exacto o `*.dominio` (solo subdominios).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostPattern {
    Exact(String),
    Subdomains(String),
}

impl HostPattern {
    pub(crate) fn parse(pattern: &str) -> Self {
        let pattern = normalize(pattern);
        match pattern.strip_prefix("*.") {
            Some(domain) => Self::Subdomains(format!(".{domain}")),
            None => Self::Exact(pattern),
        }
    }

    pub(crate) fn matches(&self, host: &str) -> bool {
        let host = normalize(host);
        match self {
            Self::Exact(exact) => host == *exact,
            Self::Subdomains(suffix) => {
                host.len() > suffix.len() && host.ends_with(suffix.as_str())
            }
        }
    }
}

fn normalize(host: &str) -> String {
    host.trim()
        .trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

/// Estado del MITM de una instancia.
#[derive(Debug)]
pub(crate) struct Mitm {
    ca: Arc<CertificateAuthority>,
    leaves: LeafCache,
    /// Config TLS ya armada por host (TD-008): evita reparsear la clave en cada handshake.
    configs: Mutex<LruCache<String, Arc<ServerConfig>>>,
    bypass: Vec<HostPattern>,
}

/// Hosts con config TLS en caché (la misma escala que la caché de hojas).
const CONFIG_CACHE: NonZeroUsize = NonZeroUsize::new(1024).unwrap();

impl Mitm {
    pub(crate) fn new(config: &MitmConfig) -> Self {
        Self {
            ca: Arc::clone(&config.ca),
            leaves: LeafCache::default(),
            configs: Mutex::new(LruCache::new(CONFIG_CACHE)),
            bypass: config
                .bypass
                .iter()
                .map(|p| HostPattern::parse(p))
                .collect(),
        }
    }

    pub(crate) fn is_bypassed(&self, host: &str) -> bool {
        self.bypass.iter().any(|p| p.matches(host))
    }

    /// Config TLS de servidor con la hoja de `host` (de caché, o emitida y cacheada).
    pub(crate) fn server_config(&self, host: &str) -> Result<Arc<ServerConfig>, String> {
        let key = normalize(host);
        if let Some(config) = self.lock_configs().get(&key) {
            return Ok(Arc::clone(config));
        }
        let config = self.build_server_config(host)?;
        self.lock_configs().put(key, Arc::clone(&config));
        Ok(config)
    }

    fn lock_configs(&self) -> std::sync::MutexGuard<'_, LruCache<String, Arc<ServerConfig>>> {
        // Un pánico con el lock tomado no deja la caché inconsistente: se sigue usando.
        self.configs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn build_server_config(&self, host: &str) -> Result<Arc<ServerConfig>, String> {
        let leaf = self
            .leaves
            .get_or_issue(&self.ca, host)
            .map_err(|e| format!("no se pudo emitir el certificado para {host}: {e}"))?;
        let chain = vec![CertificateDer::from(leaf.cert_der.clone())];
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf.key_der.clone()));
        let mut config = ServerConfig::builder_with_provider(crypto_provider())
            .with_safe_default_protocol_versions()
            .and_then(|b| b.with_no_client_auth().with_single_cert(chain, key))
            .map_err(|e| format!("config TLS inválida para {host}: {e}"))?;
        config.alpn_protocols = vec![ALPN_HTTP1.to_vec()];
        Ok(Arc::new(config))
    }
}

/// Explica un fallo del handshake TLS con el cliente. El caso típico es que no confíe en la CA.
pub(crate) fn describe_client_tls_error(error: &io::Error) -> String {
    use rustls::AlertDescription::{BadCertificate, CertificateUnknown, UnknownCA};
    let rustls_error = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>());
    match rustls_error {
        Some(rustls::Error::AlertReceived(UnknownCA | BadCertificate | CertificateUnknown)) => {
            "el cliente no confía en la CA de ProxyRR: instalala como raíz de confianza \
             (`proxyrr ca export --out ca.pem`) o excluí este host con --bypass"
                .to_owned()
        }
        _ => format!("falló el handshake TLS con el cliente: {error}"),
    }
}

/// Stream que primero devuelve `prefix` (bytes ya consumidos al inspeccionar) y después lee de `inner`.
#[derive(Debug)]
pub(crate) struct Prefixed<S> {
    prefix: Vec<u8>,
    pos: usize,
    inner: S,
}

impl<S> Prefixed<S> {
    pub(crate) fn new(prefix: Vec<u8>, inner: S) -> Self {
        Self {
            prefix,
            pos: 0,
            inner,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.pos < this.prefix.len() {
            let rest = &this.prefix[this.pos..];
            let n = rest.len().min(buf.remaining());
            buf.put_slice(&rest[..n]);
            this.pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn exact_pattern_matches_only_that_host() {
        let p = HostPattern::parse("Example.com.");
        assert!(p.matches("example.com"));
        assert!(p.matches("EXAMPLE.COM"));
        assert!(!p.matches("api.example.com"));
        assert!(!p.matches("badexample.com"));
    }

    #[test]
    fn wildcard_pattern_matches_subdomains_only() {
        let p = HostPattern::parse("*.apple.com");
        assert!(p.matches("api.apple.com"));
        assert!(p.matches("a.b.apple.com"));
        assert!(!p.matches("apple.com"));
        assert!(!p.matches("pineapple.com"));
    }

    #[test]
    fn ip_patterns_ignore_brackets() {
        assert!(HostPattern::parse("[::1]").matches("::1"));
        assert!(HostPattern::parse("10.0.2.2").matches("10.0.2.2"));
    }

    #[tokio::test]
    async fn prefixed_replays_prefix_then_inner() {
        let (mut client, server) = tokio::io::duplex(64);
        client.write_all(b" mundo").await.unwrap();
        drop(client);
        let mut stream = Prefixed::new(b"hola".to_vec(), server);
        let mut all = String::new();
        stream.read_to_string(&mut all).await.unwrap();
        assert_eq!(all, "hola mundo");
    }

    #[tokio::test]
    async fn prefixed_writes_go_to_inner() {
        let (mut client, server) = tokio::io::duplex(64);
        let mut stream = Prefixed::new(Vec::new(), server);
        stream.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
    }

    #[test]
    fn issues_server_config_per_host() {
        let ca = Arc::new(CertificateAuthority::generate().unwrap());
        let mitm = Mitm::new(&MitmConfig::new(ca));
        let config = mitm.server_config("example.com").unwrap();
        assert_eq!(config.alpn_protocols, vec![ALPN_HTTP1.to_vec()]);
        // Segunda vez sale de la caché: la misma config, sin reconstruirla.
        let again = mitm.server_config("EXAMPLE.com").unwrap();
        assert!(Arc::ptr_eq(&config, &again));
        assert!(mitm.server_config("bad host").is_err());
    }
}
