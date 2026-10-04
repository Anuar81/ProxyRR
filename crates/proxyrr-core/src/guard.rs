//! Detección de bucles: requests cuyo destino es el propio proxy (TD-003).
//!
//! Dos controles:
//! 1. Antes de conectar, sobre el host literal: `localhost`, loopback, la IP de escucha y, si se
//!    escucha en todas las interfaces (`0.0.0.0` / `::`), cualquier IP de esta máquina.
//! 2. Después de conectar, sobre la dirección ya resuelta: atrapa hostnames que resuelven al proxy
//!    (un DNS local, `/etc/hosts`, el nombre de la máquina).
//!
//! "IP de esta máquina" se decide sin libc ni listar interfaces: una IP es local si se puede hacer
//! `bind` de un socket UDP a ella (el SO solo lo permite con direcciones asignadas a una interfaz).

use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::pin::Pin;
use std::task::{Context, Poll};

use hyper::Uri;
use hyper::rt::{Read, Write};
use hyper_util::client::legacy::connect::{Connected, Connection, HttpConnector};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tower_service::Service;

/// Sabe si una dirección es el propio proxy.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SelfGuard {
    listen: SocketAddr,
}

impl SelfGuard {
    pub(crate) fn new(listen: SocketAddr) -> Self {
        Self { listen }
    }

    /// `true` si `host:port` (host literal, sin resolver) es el propio proxy.
    pub(crate) fn is_self_host(&self, host: &str, port: u16) -> bool {
        if port != self.listen.port() {
            return false;
        }
        if host.eq_ignore_ascii_case("localhost")
            || host.to_ascii_lowercase().ends_with(".localhost")
        {
            return self.reachable_by_loopback();
        }
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        bare.parse::<IpAddr>()
            .is_ok_and(|ip| self.is_self_addr(SocketAddr::new(ip, port)))
    }

    /// `true` si `addr` (ya resuelta) es el propio proxy.
    pub(crate) fn is_self_addr(&self, addr: SocketAddr) -> bool {
        if addr.port() != self.listen.port() {
            return false;
        }
        let ip = canonical(addr.ip());
        let listen = canonical(self.listen.ip());
        if listen.is_unspecified() {
            // Escucha en todas las interfaces: cualquier IP propia llega al proxy.
            ip.is_loopback() || ip.is_unspecified() || is_local_ip(ip)
        } else if listen.is_loopback() {
            ip.is_loopback() || ip.is_unspecified()
        } else {
            ip == listen
        }
    }

    fn reachable_by_loopback(&self) -> bool {
        let listen = self.listen.ip();
        listen.is_loopback() || listen.is_unspecified()
    }
}

/// `::ffff:a.b.c.d` → `a.b.c.d`, para comparar IPv4 vistas por un socket IPv6.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

/// `true` si `ip` está asignada a alguna interfaz de esta máquina.
pub(crate) fn is_local_ip(ip: IpAddr) -> bool {
    if ip.is_loopback() {
        return true;
    }
    if ip.is_multicast() || ip.is_unspecified() {
        return false;
    }
    UdpSocket::bind(SocketAddr::new(ip, 0)).is_ok()
}

/// Error de conexión: el destino resuelto es el propio proxy.
#[derive(Debug, thiserror::Error)]
#[error("el destino {0} es el propio proxy; se corta para no entrar en un bucle")]
pub(crate) struct LoopError(pub(crate) SocketAddr);

/// `HttpConnector` que rechaza las conexiones que terminan en el propio proxy.
#[derive(Debug, Clone)]
pub(crate) struct GuardedConnector {
    inner: HttpConnector,
    guard: SelfGuard,
}

impl GuardedConnector {
    pub(crate) fn new(inner: HttpConnector, guard: SelfGuard) -> Self {
        Self { inner, guard }
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

impl Service<Uri> for GuardedConnector {
    type Response = GuardedStream;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<GuardedStream, BoxError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, dst: Uri) -> Self::Future {
        let connecting = self.inner.call(dst);
        let guard = self.guard;
        Box::pin(async move {
            let stream = connecting.await?;
            let peer = stream.inner().peer_addr()?;
            if guard.is_self_addr(peer) {
                return Err(LoopError(peer).into());
            }
            Ok(GuardedStream(stream))
        })
    }
}

/// Conexión ya validada; delega todo en el `TcpStream`.
#[derive(Debug)]
pub(crate) struct GuardedStream(TokioIo<TcpStream>);

impl Connection for GuardedStream {
    fn connected(&self) -> Connected {
        self.0.connected()
    }
}

impl Read for GuardedStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
    }
}

impl Write for GuardedStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().0).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().0).poll_write_vectored(cx, bufs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard(listen: &str) -> SelfGuard {
        SelfGuard::new(listen.parse().unwrap())
    }

    #[test]
    fn loopback_listener() {
        let g = guard("127.0.0.1:9090");
        assert!(g.is_self_host("127.0.0.1", 9090));
        assert!(g.is_self_host("LOCALHOST", 9090));
        assert!(g.is_self_host("app.localhost", 9090));
        assert!(g.is_self_host("[::1]", 9090));
        assert!(g.is_self_host("0.0.0.0", 9090));
        assert!(!g.is_self_host("127.0.0.1", 8080));
        assert!(!g.is_self_host("example.com", 9090));
        assert!(!g.is_self_addr("10.0.0.1:9090".parse().unwrap()));
    }

    #[test]
    fn specific_ip_listener() {
        let g = guard("192.168.1.10:9090");
        assert!(g.is_self_host("192.168.1.10", 9090));
        // Escuchando solo en la LAN, loopback no llega al proxy.
        assert!(!g.is_self_host("127.0.0.1", 9090));
        assert!(!g.is_self_host("localhost", 9090));
    }

    #[test]
    fn all_interfaces_listener_covers_local_ips() {
        let g = guard("0.0.0.0:9090");
        assert!(g.is_self_host("127.0.0.1", 9090));
        assert!(g.is_self_host("localhost", 9090));
        assert!(g.is_self_addr("[::ffff:127.0.0.1]:9090".parse().unwrap()));
        // TEST-NET-3 (RFC 5737): nunca está asignada a una interfaz.
        assert!(!g.is_self_host("203.0.113.7", 9090));
    }

    #[test]
    fn local_ip_detection() {
        assert!(is_local_ip("127.0.0.1".parse().unwrap()));
        assert!(!is_local_ip("203.0.113.7".parse().unwrap()));
        assert!(!is_local_ip("224.0.0.1".parse().unwrap()));
    }

    #[tokio::test]
    async fn connector_rejects_hostnames_that_resolve_to_the_proxy() {
        // `localhost` resuelve a loopback: el control post-conexión lo ve aunque el host no sea una IP.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { while listener.accept().await.is_ok() {} });
        let mut http = HttpConnector::new();
        http.enforce_http(false);
        let mut connector = GuardedConnector::new(http, SelfGuard::new(addr));
        let uri: Uri = format!("http://localhost:{}/", addr.port())
            .parse()
            .unwrap();
        let error = connector
            .call(uri)
            .await
            .expect_err("debía detectar el bucle");
        assert!(error.downcast_ref::<LoopError>().is_some(), "{error}");

        let other = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let other_addr = other.local_addr().unwrap();
        tokio::spawn(async move { while other.accept().await.is_ok() {} });
        let uri: Uri = format!("http://127.0.0.1:{}/", other_addr.port())
            .parse()
            .unwrap();
        assert!(connector.call(uri).await.is_ok());
    }
}
