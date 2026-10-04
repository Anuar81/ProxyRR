//! Manejo de un request: reenvío HTTP, túnel `CONNECT` (opaco o descifrado) y respuestas de error propias.

use std::convert::Infallible;
use std::error::Error as StdError;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::header::{CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue};
use hyper::http::uri::PathAndQuery;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode, Uri, Version};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::server::Acceptor;
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio::time::timeout;
use tokio_rustls::LazyConfigAcceptor;

use crate::capture::{Recorder, Side, Tap, headers_vec};
use crate::config::{FlowIds, MitmConfig, ProxyConfig};
use crate::event::{FlowEvent, HttpFlow, TunnelFlow};
use crate::guard::{GuardedConnector, LoopError, SelfGuard};
use crate::headers::strip_hop_by_hop;
use crate::lifecycle::{Phase, Tracker};
use crate::local::{self, LOCAL_HOST, LocalRequest, LocalSite};
use crate::mitm::{Mitm, Prefixed, TLS_HANDSHAKE, crypto_provider, describe_client_tls_error};

pub(crate) type ProxyBody = BoxBody<Bytes, hyper::Error>;
type UpstreamClient = Client<HttpsConnector<GuardedConnector>, Recorder<Incoming>>;

const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// Tiempo máximo para recibir los headers de un request (corta conexiones colgadas / slowloris).
pub(crate) const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Espera del primer byte del cliente en un túnel; si no llega, el protocolo es de los que habla
/// primero el servidor y se tuneliza sin descifrar.
const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(5);
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Estado compartido por todas las conexiones de una instancia.
#[derive(Debug)]
pub(crate) struct Context {
    guard: SelfGuard,
    events: broadcast::Sender<FlowEvent>,
    next_id: FlowIds,
    client: UpstreamClient,
    connect_timeout: Duration,
    mitm: Option<Mitm>,
    /// TLS de `https://proxyrr.cert` con el MITM apagado (TD-010).
    local_tls: Option<Mitm>,
    max_body_capture: usize,
    local_site: Option<Arc<dyn LocalSite>>,
    tracker: Tracker,
}

impl Context {
    pub(crate) fn new(
        config: &ProxyConfig,
        local_addr: SocketAddr,
        events: broadcast::Sender<FlowEvent>,
        tracker: Tracker,
    ) -> io::Result<Self> {
        let guard = SelfGuard::new(local_addr);
        let mut http = HttpConnector::new();
        http.set_connect_timeout(Some(config.connect_timeout));
        http.set_nodelay(true);
        http.enforce_http(false);
        let connector = HttpsConnectorBuilder::new()
            .with_tls_config(upstream_tls(
                &config.upstream_roots,
                config.insecure_upstream,
            )?)
            .https_or_http()
            .enable_http1()
            .wrap_connector(GuardedConnector::new(http, guard));
        let client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(POOL_IDLE_TIMEOUT)
            .pool_timer(TokioTimer::new())
            .build(connector);
        if config.insecure_upstream {
            tracing::warn!(
                "verificación de certificados de los orígenes DESACTIVADA (--insecure-upstream)"
            );
        }
        let local_tls = match (&config.local_site, &config.local_site_ca) {
            (Some(_), Some(ca)) => Some(Mitm::new(&MitmConfig::new(Arc::clone(ca)))),
            _ => None,
        };
        Ok(Self {
            guard,
            events,
            next_id: config.flow_ids.clone(),
            client,
            connect_timeout: config.connect_timeout,
            mitm: config.mitm.as_ref().map(Mitm::new),
            local_tls,
            max_body_capture: config.max_body_capture,
            local_site: config.local_site.clone(),
            tracker,
        })
    }

    pub(crate) fn tracker(&self) -> &Tracker {
        &self.tracker
    }

    fn next_id(&self) -> u64 {
        self.next_id.next()
    }

    fn emit(&self, event: FlowEvent) {
        // Sin suscriptores `send` falla; no es un error del proxy.
        let _ = self.events.send(event);
    }

    /// MITM con el que se termina el TLS de un túnel a `host`, si corresponde descifrarlo.
    fn mitm_for(&self, host: &str) -> Option<(&Mitm, bool)> {
        if let Some(mitm) = &self.mitm
            && !mitm.is_bypassed(host)
        {
            return Some((mitm, false));
        }
        // Sin MITM (o con el host en bypass), `proxyrr.cert` igual se termina para servir la página.
        self.local_tls
            .as_ref()
            .filter(|_| host.eq_ignore_ascii_case(LOCAL_HOST))
            .map(|mitm| (mitm, true))
    }
}

/// TLS hacia los orígenes: raíces del SO + las extra de la configuración. El ALPN (solo HTTP/1.1)
/// lo fija `HttpsConnectorBuilder::enable_http1`, que exige recibirlo vacío.
fn upstream_tls(extra_roots: &[Vec<u8>], insecure: bool) -> io::Result<ClientConfig> {
    let builder = ClientConfig::builder_with_provider(crypto_provider())
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?;
    if insecure {
        return Ok(builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCert(crypto_provider())))
            .with_no_client_auth());
    }
    let mut roots = RootCertStore::empty();
    // Un certificado del SO que no se puede interpretar se ignora; el resto sirve igual.
    roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
    let (added, ignored) = roots.add_parsable_certificates(
        extra_roots
            .iter()
            .map(|der| CertificateDer::from(der.clone())),
    );
    if ignored > 0 {
        tracing::warn!(
            added,
            ignored,
            "algunos certificados de --upstream-ca no se pudieron usar"
        );
    }
    Ok(builder.with_root_certificates(roots).with_no_client_auth())
}

/// Verificador que acepta cualquier certificado del origen (solo `--insecure-upstream`). Las firmas
/// del handshake sí se verifican, así que la conexión sigue siendo con quien tiene la clave del
/// certificado presentado; lo que no se comprueba es que ese certificado sea legítimo.
#[derive(Debug)]
struct AcceptAnyCert(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
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

/// Destino validado de un `CONNECT`.
#[derive(Debug, Clone)]
struct Target {
    host: String,
    port: u16,
    authority: String,
}

impl Target {
    /// Base de las URLs de los requests descifrados: sin `:443` cuando es el puerto por defecto.
    fn base_url(&self) -> String {
        if self.port == 443 {
            format!("https://{}", self.host)
        } else {
            format!("https://{}:{}", self.host, self.port)
        }
    }
}

/// Datos para emitir el evento de un túnel cuando se sepa cómo terminó.
#[derive(Debug)]
struct TunnelLog {
    id: u64,
    authority: String,
    started: Instant,
}

impl TunnelLog {
    fn emit(&self, ctx: &Context, status: StatusCode, error: Option<String>, intercepted: bool) {
        ctx.emit(FlowEvent::Tunnel(TunnelFlow {
            id: self.id,
            authority: self.authority.clone(),
            status: status.as_u16(),
            intercepted,
            error,
            elapsed: self.started.elapsed(),
        }));
    }
}

pub(crate) async fn handle(
    req: Request<Incoming>,
    ctx: &Arc<Context>,
) -> Result<Response<ProxyBody>, Infallible> {
    Ok(if req.method() == Method::CONNECT {
        tunnel(req, ctx).await
    } else {
        let check = check_http_target(req.uri(), &ctx.guard);
        exchange(req, ctx, check).await
    })
}

/// Reenvía un request al origen (ya validado por `check`), captura sus bodies y emite sus eventos.
async fn exchange(
    req: Request<Incoming>,
    ctx: &Context,
    check: Result<(), Reject>,
) -> Response<ProxyBody> {
    let id = ctx.next_id();
    let started = Instant::now();
    let tap = Tap::new(id, started, ctx.max_body_capture, ctx.events.clone());
    let method = req.method().to_string();
    let url = req.uri().to_string();
    let request_headers = headers_vec(req.headers());
    let req = req.map(|body| Recorder::new(body, Arc::clone(&tap), Side::Request));

    let outcome = if let Some(response) = local_response(&req, ctx) {
        // El sitio local va antes que la validación: `/cert` directo no tiene forma absoluta.
        let headers = headers_vec(response.headers());
        Ok((response, headers))
    } else if let Err(reject) = check {
        Err(reject)
    } else {
        send_upstream(req, ctx).await.map(|mut response| {
            let headers = headers_vec(response.headers());
            strip_hop_by_hop(response.headers_mut());
            (response.map(BodyExt::boxed), headers)
        })
    };
    let (response, response_headers, error) = match outcome {
        Ok((response, headers)) => (response, headers, None),
        Err(reject) => {
            let response = reject.response();
            let headers = headers_vec(response.headers());
            (response, headers, Some(reject.message))
        }
    };
    // Se emite ANTES de devolver la respuesta: su body no puede terminar antes, así que
    // `HttpBodies` de este id siempre llega después de `Http`.
    ctx.emit(FlowEvent::Http(HttpFlow {
        id,
        method,
        url,
        request_headers,
        status: response.status().as_u16(),
        response_headers,
        error,
        elapsed: started.elapsed(),
        content_length: content_length(response.headers()),
    }));
    response.map(|body| Recorder::new(body, tap, Side::Response).boxed())
}

async fn send_upstream(
    mut req: Request<Recorder<Incoming>>,
    ctx: &Context,
) -> Result<Response<Incoming>, Reject> {
    strip_hop_by_hop(req.headers_mut());
    // El cliente upstream habla HTTP/1.1 aunque el navegador haya venido en 1.0.
    *req.version_mut() = Version::HTTP_11;
    ctx.client.request(req).await.map_err(|e| {
        if is_loop(&e) {
            return loop_detected();
        }
        Reject::new(
            StatusCode::BAD_GATEWAY,
            format!("ProxyRR no pudo llegar al origen: {}", error_chain(&e)),
        )
    })
}

async fn tunnel(req: Request<Incoming>, ctx: &Arc<Context>) -> Response<ProxyBody> {
    let log = TunnelLog {
        id: ctx.next_id(),
        authority: req.uri().to_string(),
        started: Instant::now(),
    };
    let target = match check_tunnel_target(req.uri(), &ctx.guard) {
        Ok(target) => target,
        Err(reject) => return tunnel_rejected(ctx, &log, reject),
    };

    if ctx.mitm_for(&target.host).is_some() {
        // Con MITM se responde enseguida: qué hacer se decide al ver el primer byte del cliente.
        tokio::spawn(intercept(req, Arc::clone(ctx), target, log));
        return ok_empty();
    }

    // Túnel opaco: se conecta ANTES de responder, para poder devolver 502/504.
    match connect_upstream(&target.authority, ctx).await {
        Ok(upstream) => {
            log.emit(ctx, StatusCode::OK, None, false);
            tokio::spawn(splice_upgrade(req, upstream, Arc::clone(ctx)));
            ok_empty()
        }
        Err(reject) => tunnel_rejected(ctx, &log, reject),
    }
}

fn tunnel_rejected(ctx: &Context, log: &TunnelLog, reject: Reject) -> Response<ProxyBody> {
    let response = reject.response();
    log.emit(ctx, reject.status, Some(reject.message), false);
    response
}

/// Túnel con MITM activo: descifra si el cliente habla TLS; si no, tuneliza sin tocar nada.
async fn intercept(req: Request<Incoming>, ctx: Arc<Context>, target: Target, log: TunnelLog) {
    // Si el cliente cortó antes del upgrade no hay nada que hacer.
    let Ok(upgraded) = hyper::upgrade::on(req).await else {
        return;
    };
    let mut client = TokioIo::new(upgraded);
    let mut first = [0u8; 1];
    let prefix = match timeout(FIRST_BYTE_TIMEOUT, client.read(&mut first)).await {
        Ok(Ok(0) | Err(_)) => return,
        Ok(Ok(n)) => first[..n].to_vec(),
        Err(_) => Vec::new(),
    };
    let is_tls = prefix.first() == Some(&TLS_HANDSHAKE);
    let stream = Prefixed::new(prefix, client);
    let Some((mitm, local_only)) = ctx.mitm_for(&target.host).filter(|_| is_tls) else {
        return splice_stream(stream, &ctx, &target, &log).await;
    };

    let handshake_failed = |message: String| log.emit(&ctx, StatusCode::OK, Some(message), true);
    let start = match timeout(
        TLS_HANDSHAKE_TIMEOUT,
        LazyConfigAcceptor::new(Acceptor::default(), stream),
    )
    .await
    {
        Ok(Ok(start)) => start,
        Ok(Err(e)) => return handshake_failed(format!("ClientHello TLS inválido: {e}")),
        Err(_) => return handshake_failed("el cliente no completó el handshake TLS".to_owned()),
    };
    // Sin SNI (p. ej. destino por IP) la hoja se emite para el host del CONNECT.
    let leaf_host = start
        .client_hello()
        .server_name()
        .map_or_else(|| target.host.clone(), str::to_owned);
    // Solo el sitio local: un SNI distinto de `proxyrr.cert` no se descifra con la CA local.
    if local_only && !leaf_host.eq_ignore_ascii_case(LOCAL_HOST) {
        return handshake_failed(format!(
            "SNI {leaf_host} en un túnel a {LOCAL_HOST}: no se descifra sin --mitm"
        ));
    }
    let config = match mitm.server_config(&leaf_host) {
        Ok(config) => config,
        Err(message) => return handshake_failed(message),
    };
    let tls = match timeout(TLS_HANDSHAKE_TIMEOUT, start.into_stream(config)).await {
        Ok(Ok(tls)) => tls,
        Ok(Err(e)) => return handshake_failed(describe_client_tls_error(&e)),
        Err(_) => return handshake_failed("el cliente no completó el handshake TLS".to_owned()),
    };
    log.emit(&ctx, StatusCode::OK, None, true);

    let base = target.base_url();
    let tracker = ctx.tracker().clone();
    let service = service_fn(move |req| {
        let ctx = Arc::clone(&ctx);
        let base = base.clone();
        async move { Ok::<_, Infallible>(decrypted(req, &ctx, &base).await) }
    });
    let conn = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .serve_connection(TokioIo::new(tls), service);
    tokio::pin!(conn);
    // Los errores de la conexión descifrada (cliente que corta) no afectan al resto.
    tokio::select! {
        _ = conn.as_mut() => return,
        () = tracker.reached(Phase::Draining) => conn.as_mut().graceful_shutdown(),
    }
    tokio::select! {
        _ = conn => {}
        () = tracker.reached(Phase::Closing) => {}
    }
}

/// Un request dentro del túnel descifrado: llega en forma de origen y va a `https://host[:puerto]`.
async fn decrypted(mut req: Request<Incoming>, ctx: &Context, base: &str) -> Response<ProxyBody> {
    let path = req.uri().path_and_query().map_or("/", PathAndQuery::as_str);
    let check = format!("{base}{path}")
        .parse::<Uri>()
        .map(|uri| *req.uri_mut() = uri)
        .map_err(|_| {
            Reject::new(
                StatusCode::BAD_REQUEST,
                "ProxyRR: URL inválida en el túnel.",
            )
        });
    exchange(req, ctx, check).await
}

/// Túnel opaco sobre un cliente ya upgradeado (con los bytes que se hayan leído al inspeccionar).
async fn splice_stream(
    mut client: Prefixed<TokioIo<hyper::upgrade::Upgraded>>,
    ctx: &Context,
    target: &Target,
    log: &TunnelLog,
) {
    match connect_upstream(&target.authority, ctx).await {
        Ok(mut upstream) => {
            log.emit(ctx, StatusCode::OK, None, false);
            // Un túnel no tiene requests que terminar: al apagar se cierra enseguida.
            tokio::select! {
                // Un reset de cualquiera de los lados es el final normal de un túnel.
                _ = tokio::io::copy_bidirectional(&mut client, &mut upstream) => {}
                () = ctx.tracker().reached(Phase::Draining) => {}
            }
        }
        // El cliente ya recibió 200: solo queda cerrar y registrar el motivo.
        Err(reject) => log.emit(ctx, reject.status, Some(reject.message), false),
    }
}

/// Conecta con el destino de un túnel, con timeout, y rechaza si resolvió al propio proxy.
async fn connect_upstream(authority: &str, ctx: &Context) -> Result<TcpStream, Reject> {
    let limit = ctx.connect_timeout;
    match timeout(limit, TcpStream::connect(authority)).await {
        Ok(Ok(upstream)) => match upstream.peer_addr() {
            Ok(peer) if ctx.guard.is_self_addr(peer) => Err(loop_detected()),
            _ => Ok(upstream),
        },
        Ok(Err(e)) => Err(Reject::new(
            StatusCode::BAD_GATEWAY,
            format!("ProxyRR no pudo conectar con {authority}: {e}"),
        )),
        Err(_) => Err(Reject::new(
            StatusCode::GATEWAY_TIMEOUT,
            format!("ProxyRR: {authority} no respondió en {limit:?}"),
        )),
    }
}

/// Copia bytes entre el cliente (una vez hecho el upgrade) y el destino hasta que alguno cierre.
async fn splice_upgrade(req: Request<Incoming>, mut upstream: TcpStream, ctx: Arc<Context>) {
    // Si el cliente cortó antes del upgrade no hay nada que copiar.
    if let Ok(upgraded) = hyper::upgrade::on(req).await {
        let mut client = TokioIo::new(upgraded);
        tokio::select! {
            // Un reset de cualquiera de los lados es el final normal de un túnel.
            _ = tokio::io::copy_bidirectional(&mut client, &mut upstream) => {}
            () = ctx.tracker().reached(Phase::Draining) => {}
        }
    }
}

/// Valida un request no-CONNECT: forma absoluta, esquema `http` y que no apunte al propio proxy.
fn check_http_target(uri: &Uri, guard: &SelfGuard) -> Result<(), Reject> {
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
    // El esquema ya se validó como `http`, así que sin puerto explícito el puerto es 80 (RFC 9110 §4.2.1).
    if guard.is_self_host(authority.host(), authority.port_u16().unwrap_or(80)) {
        return Err(loop_detected());
    }
    Ok(())
}

/// Valida el destino de un CONNECT.
fn check_tunnel_target(uri: &Uri, guard: &SelfGuard) -> Result<Target, Reject> {
    let Some((authority, port)) = uri.authority().and_then(|a| Some((a, a.port_u16()?))) else {
        return Err(Reject::new(
            StatusCode::BAD_REQUEST,
            "ProxyRR: CONNECT requiere un destino host:puerto.",
        ));
    };
    if guard.is_self_host(authority.host(), port) {
        return Err(loop_detected());
    }
    Ok(Target {
        host: authority.host().to_owned(),
        port,
        authority: authority.as_str().to_owned(),
    })
}

fn loop_detected() -> Reject {
    Reject::new(
        StatusCode::LOOP_DETECTED,
        "ProxyRR: el destino es el propio proxy; se corta para no entrar en un bucle.",
    )
}

/// `true` si en la cadena de causas hay un [`LoopError`] (el destino resolvió al propio proxy).
fn is_loop(error: &(dyn StdError + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(e) = current {
        if e.downcast_ref::<LoopError>().is_some() {
            return true;
        }
        current = e.source();
    }
    false
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

fn ok_empty() -> Response<ProxyBody> {
    Response::new(Empty::new().map_err(|never| match never {}).boxed())
}

/// Respuesta del sitio local si el request le corresponde (spec 0006, CA 11–12).
fn local_response<B>(req: &Request<B>, ctx: &Context) -> Option<Response<ProxyBody>> {
    let site = ctx.local_site.as_ref()?;
    let uri = req.uri();
    let (direct, path) = local::route(
        uri.authority().map(hyper::http::uri::Authority::host),
        uri.scheme().is_some(),
        uri.path(),
    )?;
    let user_agent = req
        .headers()
        .get(hyper::header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    let local = site.respond(&LocalRequest {
        direct,
        path,
        user_agent,
    });
    let mut response = Response::new(
        Full::new(local.body)
            .map_err(|never| match never {})
            .boxed(),
    );
    *response.status_mut() = StatusCode::from_u16(local.status).unwrap_or(StatusCode::OK);
    let headers = response.headers_mut();
    for (name, value) in &local.headers {
        if let (Ok(name), Ok(value)) = (
            hyper::header::HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            headers.append(name, value);
        }
    }
    Some(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local() -> SelfGuard {
        SelfGuard::new("127.0.0.1:9090".parse().unwrap())
    }

    #[test]
    fn loop_errors_are_found_in_the_cause_chain() {
        #[derive(Debug, thiserror::Error)]
        #[error("envoltorio")]
        struct Wrapper(#[source] LoopError);
        let wrapped = Wrapper(LoopError("127.0.0.1:9090".parse().unwrap()));
        assert!(is_loop(&wrapped));
        assert!(!is_loop(&io::Error::other("otro")));
    }

    #[test]
    fn http_target_rules() {
        let ok: Uri = "http://example.com/a?b=1".parse().unwrap();
        assert!(check_http_target(&ok, &local()).is_ok());
        let origin_form: Uri = "/a".parse().unwrap();
        assert_eq!(
            check_http_target(&origin_form, &local())
                .unwrap_err()
                .status,
            StatusCode::BAD_REQUEST
        );
        let https: Uri = "https://example.com/".parse().unwrap();
        assert_eq!(
            check_http_target(&https, &local()).unwrap_err().status,
            StatusCode::BAD_REQUEST
        );
        let me: Uri = "http://localhost:9090/".parse().unwrap();
        assert_eq!(
            check_http_target(&me, &local()).unwrap_err().status,
            StatusCode::LOOP_DETECTED
        );
    }

    #[test]
    fn tunnel_target_requires_port() {
        let no_port: Uri = "example.com".parse().unwrap();
        assert_eq!(
            check_tunnel_target(&no_port, &local()).unwrap_err().status,
            StatusCode::BAD_REQUEST
        );
        let ok: Uri = "example.com:443".parse().unwrap();
        let target = check_tunnel_target(&ok, &local()).unwrap();
        assert_eq!(target.authority, "example.com:443");
        assert_eq!(target.base_url(), "https://example.com");
    }

    #[test]
    fn base_url_keeps_non_default_port_and_ipv6_brackets() {
        let uri: Uri = "[::1]:8443".parse().unwrap();
        let target = check_tunnel_target(&uri, &local()).unwrap();
        assert_eq!(target.base_url(), "https://[::1]:8443");
    }
}
