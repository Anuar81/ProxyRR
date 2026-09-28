//! Manejo de un request: reenvío HTTP, túnel `CONNECT` y respuestas de error propias.

use std::convert::Infallible;
use std::error::Error as StdError;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::header::{CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue};
use hyper::{Method, Request, Response, StatusCode, Uri, Version};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tokio::net::TcpStream;
use tokio::sync::broadcast;

use crate::event::{FlowEvent, HttpFlow, TunnelFlow};
use crate::headers::strip_hop_by_hop;

pub(crate) type ProxyBody = BoxBody<Bytes, hyper::Error>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// Estado compartido por todas las conexiones de una instancia.
#[derive(Debug)]
pub(crate) struct Context {
    local_addr: SocketAddr,
    events: broadcast::Sender<FlowEvent>,
    next_id: AtomicU64,
    client: Client<HttpConnector, Incoming>,
}

impl Context {
    pub(crate) fn new(local_addr: SocketAddr, events: broadcast::Sender<FlowEvent>) -> Self {
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(CONNECT_TIMEOUT));
        connector.set_nodelay(true);
        let client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(POOL_IDLE_TIMEOUT)
            .pool_timer(TokioTimer::new())
            .build(connector);
        Self {
            local_addr,
            events,
            next_id: AtomicU64::new(1),
            client,
        }
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn emit(&self, event: FlowEvent) {
        // Sin suscriptores `send` falla; no es un error del proxy.
        let _ = self.events.send(event);
    }
}

/// Rechazo generado por el proxy (no por el origen).
#[derive(Debug)]
struct Reject {
    status: StatusCode,
    message: String,
}

impl Reject {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn response(&self) -> Response<ProxyBody> {
        text_response(self.status, &self.message)
    }
}

pub(crate) async fn handle(
    req: Request<Incoming>,
    ctx: &Context,
) -> Result<Response<ProxyBody>, Infallible> {
    Ok(if req.method() == Method::CONNECT {
        tunnel(req, ctx).await
    } else {
        forward(req, ctx).await
    })
}

async fn send_upstream(
    mut req: Request<Incoming>,
    ctx: &Context,
) -> Result<Response<Incoming>, Reject> {
    check_http_target(req.uri(), ctx.local_addr)?;
    strip_hop_by_hop(req.headers_mut());
    // El cliente upstream habla HTTP/1.1 aunque el navegador haya venido en 1.0.
    *req.version_mut() = Version::HTTP_11;
    ctx.client.request(req).await.map_err(|e| {
        Reject::new(
            StatusCode::BAD_GATEWAY,
            format!("ProxyRR no pudo llegar al origen: {}", error_chain(&e)),
        )
    })
}

async fn forward(req: Request<Incoming>, ctx: &Context) -> Response<ProxyBody> {
    let id = ctx.next_id();
    let started = Instant::now();
    let method = req.method().to_string();
    let url = req.uri().to_string();

    let (response, error) = match send_upstream(req, ctx).await {
        Ok(mut response) => {
            strip_hop_by_hop(response.headers_mut());
            (response.map(BodyExt::boxed), None)
        }
        Err(reject) => (reject.response(), Some(reject.message)),
    };
    ctx.emit(FlowEvent::Http(HttpFlow {
        id,
        method,
        url,
        status: response.status().as_u16(),
        error,
        elapsed: started.elapsed(),
        content_length: content_length(response.headers()),
    }));
    response
}

async fn tunnel(req: Request<Incoming>, ctx: &Context) -> Response<ProxyBody> {
    let id = ctx.next_id();
    let started = Instant::now();
    let authority = req.uri().to_string();

    let outcome = connect_tunnel(req.uri(), &authority, ctx.local_addr).await;
    let elapsed = started.elapsed();

    let (response, error) = match outcome {
        Ok(upstream) => {
            tokio::spawn(splice(req, upstream));
            let mut response = Response::new(empty());
            *response.status_mut() = StatusCode::OK;
            (response, None)
        }
        Err(reject) => (reject.response(), Some(reject.message)),
    };
    ctx.emit(FlowEvent::Tunnel(TunnelFlow {
        id,
        authority,
        status: response.status().as_u16(),
        error,
        elapsed,
    }));
    response
}

/// Conecta con el destino del CONNECT. Se hace ANTES de responder, para poder devolver 502/504.
async fn connect_tunnel(
    uri: &Uri,
    authority: &str,
    local: SocketAddr,
) -> Result<TcpStream, Reject> {
    let target = check_tunnel_target(uri, local)?;
    match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(target)).await {
        Ok(Ok(upstream)) => Ok(upstream),
        Ok(Err(e)) => Err(Reject::new(
            StatusCode::BAD_GATEWAY,
            format!("ProxyRR no pudo conectar con {authority}: {e}"),
        )),
        Err(_) => Err(Reject::new(
            StatusCode::GATEWAY_TIMEOUT,
            format!("ProxyRR: {authority} no respondió en {CONNECT_TIMEOUT:?}"),
        )),
    }
}

/// Copia bytes entre el cliente (una vez hecho el upgrade) y el destino hasta que alguno cierre.
async fn splice(req: Request<Incoming>, mut upstream: TcpStream) {
    // Si el cliente cortó antes del upgrade no hay nada que copiar.
    if let Ok(upgraded) = hyper::upgrade::on(req).await {
        let mut client = TokioIo::new(upgraded);
        // Un reset de cualquiera de los lados es el final normal de un túnel.
        let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
    }
}

/// Valida un request no-CONNECT: forma absoluta, esquema `http` y que no apunte al propio proxy.
fn check_http_target(uri: &Uri, local: SocketAddr) -> Result<(), Reject> {
    let Some(scheme) = uri.scheme_str() else {
        return Err(Reject::new(
            StatusCode::BAD_REQUEST,
            "Esto es ProxyRR, un proxy: configuralo como proxy HTTP en tu navegador o SO \
             en vez de abrirlo como si fuera un sitio.",
        ));
    };
    if !scheme.eq_ignore_ascii_case("http") {
        return Err(Reject::new(
            StatusCode::BAD_REQUEST,
            format!("ProxyRR: el esquema `{scheme}` solo se acepta por túnel CONNECT."),
        ));
    }
    let Some(authority) = uri.authority() else {
        return Err(Reject::new(
            StatusCode::BAD_REQUEST,
            "ProxyRR: la URL no tiene host.",
        ));
    };
    if is_self(authority.host(), authority.port_u16().unwrap_or(80), local) {
        return Err(loop_detected());
    }
    Ok(())
}

/// Valida el destino de un CONNECT y devuelve el `host:puerto` a conectar.
fn check_tunnel_target(uri: &Uri, local: SocketAddr) -> Result<String, Reject> {
    let Some((authority, port)) = uri.authority().and_then(|a| Some((a, a.port_u16()?))) else {
        return Err(Reject::new(
            StatusCode::BAD_REQUEST,
            "ProxyRR: CONNECT requiere un destino host:puerto.",
        ));
    };
    if is_self(authority.host(), port, local) {
        return Err(loop_detected());
    }
    Ok(authority.as_str().to_owned())
}

fn loop_detected() -> Reject {
    Reject::new(
        StatusCode::LOOP_DETECTED,
        "ProxyRR: el destino es el propio proxy; se corta para no entrar en un bucle.",
    )
}

/// `true` si `host:port` es el propio proxy. Sin resolución DNS: cubre `localhost`, loopback y la
/// IP exacta de escucha.
fn is_self(host: &str, port: u16, local: SocketAddr) -> bool {
    if port != local.port() {
        return false;
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    bare.parse::<IpAddr>()
        .is_ok_and(|ip| ip.is_loopback() || ip == local.ip())
}

fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers.get(CONTENT_LENGTH)?.to_str().ok()?.parse().ok()
}

fn error_chain(error: &dyn StdError) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

fn text_response(status: StatusCode, message: &str) -> Response<ProxyBody> {
    let body = Full::new(Bytes::from(format!("{message}\n")))
        .map_err(|never| match never {})
        .boxed();
    let mut response = Response::new(body);
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(CONNECTION, HeaderValue::from_static("close"));
    response
}

fn empty() -> ProxyBody {
    Empty::new().map_err(|never| match never {}).boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local() -> SocketAddr {
        "127.0.0.1:9090".parse().unwrap()
    }

    #[test]
    fn detects_self_by_loopback_localhost_and_listen_ip() {
        assert!(is_self("127.0.0.1", 9090, local()));
        assert!(is_self("LOCALHOST", 9090, local()));
        assert!(is_self("[::1]", 9090, local()));
        let lan: SocketAddr = "192.168.1.10:9090".parse().unwrap();
        assert!(is_self("192.168.1.10", 9090, lan));
    }

    #[test]
    fn other_port_or_host_is_not_self() {
        assert!(!is_self("127.0.0.1", 8080, local()));
        assert!(!is_self("example.com", 9090, local()));
    }

    #[test]
    fn http_target_rules() {
        let ok: Uri = "http://example.com/a?b=1".parse().unwrap();
        assert!(check_http_target(&ok, local()).is_ok());
        let origin_form: Uri = "/a".parse().unwrap();
        assert_eq!(
            check_http_target(&origin_form, local()).unwrap_err().status,
            StatusCode::BAD_REQUEST
        );
        let https: Uri = "https://example.com/".parse().unwrap();
        assert_eq!(
            check_http_target(&https, local()).unwrap_err().status,
            StatusCode::BAD_REQUEST
        );
        let me: Uri = "http://localhost:9090/".parse().unwrap();
        assert_eq!(
            check_http_target(&me, local()).unwrap_err().status,
            StatusCode::LOOP_DETECTED
        );
    }

    #[test]
    fn tunnel_target_requires_port() {
        let no_port: Uri = "example.com".parse().unwrap();
        assert_eq!(
            check_tunnel_target(&no_port, local()).unwrap_err().status,
            StatusCode::BAD_REQUEST
        );
        let ok: Uri = "example.com:443".parse().unwrap();
        assert_eq!(
            check_tunnel_target(&ok, local()).unwrap(),
            "example.com:443"
        );
    }
}
