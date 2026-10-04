//! Servidor HTTP + WebSocket de la API (spec 0008).

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, RawQuery, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use bytes::Bytes;
use proxyrr_core::Headers;
use proxyrr_rules::Rule;
use proxyrr_store::StoredFlow;
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::dto::{FlowDto, FlowSummaryDto, StatusDto, WsMessage};
use crate::engine::{Engine, EngineError, ProxySettings};

/// Puerto por defecto de la API (el del proxy + 1).
pub const DEFAULT_API_PORT: u16 = 9091;
/// Ruta del WebSocket de eventos: la única que acepta el token por query.
const EVENTS_PATH: &str = "/api/v1/events";
const MIN_TOKEN_LEN: usize = 16;

/// Configuración del servidor de la API.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// Dirección de escucha. Tiene que ser loopback.
    pub listen: SocketAddr,
    /// Token fijo (≥ 16 caracteres `[A-Za-z0-9._~-]`). `None`: uno aleatorio de 256 bits.
    pub token: Option<String>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_API_PORT)),
            token: None,
        }
    }
}

/// Errores al arrancar la API.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// Dirección fuera de loopback.
    #[error("la API solo escucha en loopback (127.0.0.1 o ::1), no en {0}")]
    NotLoopback(SocketAddr),
    /// Token explícito inválido.
    #[error("el token de la API debe tener al menos {MIN_TOKEN_LEN} caracteres [A-Za-z0-9._~-]")]
    InvalidToken,
    /// No se pudo generar el token aleatorio.
    #[error("no se pudo generar el token de la API")]
    Random,
    /// No se pudo abrir el puerto.
    #[error("no se pudo escuchar en {addr}: {source}")]
    Bind {
        /// Dirección pedida.
        addr: SocketAddr,
        /// Causa.
        source: io::Error,
    },
}

/// Servidor de la API corriendo.
#[derive(Debug)]
pub struct ApiServer {
    local_addr: SocketAddr,
    token: Arc<str>,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl ApiServer {
    /// Abre el puerto y sirve la API sobre `engine`.
    ///
    /// # Errors
    ///
    /// Ver [`ApiError`].
    pub async fn start(config: ApiConfig, engine: Arc<Engine>) -> Result<Self, ApiError> {
        if !config.listen.ip().is_loopback() {
            return Err(ApiError::NotLoopback(config.listen));
        }
        let token: Arc<str> = match config.token {
            Some(token) if valid_token(&token) => token.into(),
            Some(_) => return Err(ApiError::InvalidToken),
            None => random_token()?.into(),
        };
        let bind = |source| ApiError::Bind {
            addr: config.listen,
            source,
        };
        let listener = TcpListener::bind(config.listen).await.map_err(bind)?;
        let local_addr = listener.local_addr().map_err(bind)?;
        let (shutdown, stop) = watch::channel(false);
        let state = AppState {
            engine,
            token: Arc::clone(&token),
            port: local_addr.port(),
            stop: stop.clone(),
        };
        let app = router(state);
        let mut stop = stop;
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = stop.wait_for(|stopped| *stopped).await;
                })
                .await;
        });
        Ok(Self {
            local_addr,
            token,
            shutdown,
            task,
        })
    }

    /// Dirección efectiva.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// `http://127.0.0.1:<puerto>`.
    #[must_use]
    pub fn base_url(&self) -> String {
        format!("http://{}", self.local_addr)
    }

    /// Token de esta sesión.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Cierra los WebSocket, deja de aceptar y libera el puerto.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
    }
}

fn valid_token(token: &str) -> bool {
    token.len() >= MIN_TOKEN_LEN
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._~-".contains(&b))
}

fn random_token() -> Result<String, ApiError> {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| ApiError::Random)?;
    Ok(hex::encode(bytes))
}

/// Comparación sin cortocircuito: el tiempo no depende de dónde difieren.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

#[derive(Debug, Clone)]
struct AppState {
    engine: Arc<Engine>,
    token: Arc<str>,
    port: u16,
    stop: watch::Receiver<bool>,
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/status", get(status))
        .route("/api/v1/proxy/start", post(start_proxy))
        .route("/api/v1/proxy/stop", post(stop_proxy))
        .route("/api/v1/flows", get(list_flows).delete(clear_flows))
        .route("/api/v1/flows/{id}", get(get_flow))
        .route("/api/v1/flows/{id}/replay", post(replay_flow))
        .route("/api/v1/flows/{id}/{side}/body", get(get_body))
        .route("/api/v1/rules", get(get_rules).put(put_rules))
        .route("/api/v1/har", get(har))
        .route(EVENTS_PATH, get(events))
        .fallback(|| async { Failure::new(StatusCode::NOT_FOUND, "not_found", "ruta desconocida") })
        .method_not_allowed_fallback(|| async {
            Failure::new(
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "método no permitido en esta ruta",
            )
        })
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// Error de la API: `{"error": {"code", "message"}}` (CA 9).
#[derive(Debug)]
struct Failure {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl Failure {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn not_found(id: u64) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("no hay flujo {id}"),
        )
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": { "code": self.code, "message": self.message } });
        (self.status, Json(body)).into_response()
    }
}

impl From<EngineError> for Failure {
    fn from(e: EngineError) -> Self {
        let (status, code) = match e {
            EngineError::AlreadyRunning(_) => (StatusCode::CONFLICT, "already_running"),
            EngineError::Bind { .. } => (StatusCode::CONFLICT, "bind_failed"),
            EngineError::NoCa => (StatusCode::UNPROCESSABLE_ENTITY, "no_ca"),
        };
        Self::new(status, code, e.to_string())
    }
}

/// `Host` permitido → token (CA 2, 3). Todas las respuestas llevan `Cache-Control: no-store`.
async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let mut response = if !host_allowed(request.headers(), state.port) {
        Failure::new(
            StatusCode::FORBIDDEN,
            "forbidden_host",
            "Host no permitido: usá 127.0.0.1, localhost o [::1]",
        )
        .into_response()
    } else if authorized(&request, &state.token) {
        next.run(request).await
    } else {
        Failure::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "falta el token o es inválido (Authorization: Bearer <token>)",
        )
        .into_response()
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn host_allowed(headers: &HeaderMap, port: u16) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    ["127.0.0.1", "localhost", "[::1]"]
        .iter()
        .any(|name| host == format!("{name}:{port}"))
}

fn authorized(request: &Request, token: &str) -> bool {
    let bearer = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let query = (request.uri().path() == EVENTS_PATH)
        .then(|| query_param(request.uri().query(), "token"))
        .flatten();
    bearer
        .or(query)
        .is_some_and(|given| constant_time_eq(given.as_bytes(), token.as_bytes()))
}

fn query_param<'a>(query: Option<&'a str>, name: &str) -> Option<&'a str> {
    query?
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find_map(|(key, value)| (key == name).then_some(value))
}

async fn status(State(state): State<AppState>) -> Json<StatusDto> {
    Json(state.engine.status().await.into())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StartRequest {
    listen: Option<String>,
    mitm: bool,
    bypass: Vec<String>,
}

/// Body JSON opcional: vacío equivale a `{}`.
async fn start_proxy(State(state): State<AppState>, body: Bytes) -> Result<Response, Failure> {
    let request: StartRequest = if body.iter().all(u8::is_ascii_whitespace) {
        StartRequest::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| Failure::new(StatusCode::BAD_REQUEST, "bad_request", e.to_string()))?
    };
    let mut settings = ProxySettings {
        mitm: request.mitm,
        bypass: request.bypass,
        ..ProxySettings::default()
    };
    if let Some(listen) = request.listen {
        settings.listen = listen.parse().map_err(|_| {
            Failure::new(
                StatusCode::BAD_REQUEST,
                "bad_request",
                format!("listen inválido: {listen:?} (esperado IP:PUERTO)"),
            )
        })?;
    }
    let status = state.engine.start_proxy(settings).await?;
    Ok(Json(crate::dto::ProxyStatusDto::from(status)).into_response())
}

async fn stop_proxy(State(state): State<AppState>) -> Json<crate::dto::ProxyStatusDto> {
    Json(state.engine.stop_proxy().await.into())
}

async fn list_flows(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
) -> Result<Json<Vec<FlowSummaryDto>>, Failure> {
    let store = state.engine.store();
    let flows = match query_param(query.as_deref(), "after") {
        None => store.list(),
        Some(after) => {
            let after = after.parse().map_err(|_| {
                Failure::new(
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "after debe ser un id",
                )
            })?;
            store.list_after(after)
        }
    };
    Ok(Json(flows.into_iter().map(Into::into).collect()))
}

async fn clear_flows(State(state): State<AppState>) -> StatusCode {
    state.engine.clear();
    StatusCode::NO_CONTENT
}

/// `GET /api/v1/har`: todos los flujos HTTP como HAR 1.2, para descargar.
async fn har(State(state): State<AppState>) -> Response {
    let mut response = Json(state.engine.har()).into_response();
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"proxyrr.har\""),
    );
    response
}

fn parse_id(raw: &str) -> Result<u64, Failure> {
    raw.parse()
        .map_err(|_| Failure::new(StatusCode::NOT_FOUND, "not_found", "id de flujo inválido"))
}

/// `GET /api/v1/rules`: reglas en orden.
async fn get_rules(State(state): State<AppState>) -> Json<Vec<Rule>> {
    Json(state.engine.rules().list())
}

/// `PUT /api/v1/rules`: reemplaza todas las reglas (aplican desde el próximo request).
async fn put_rules(State(state): State<AppState>, body: Bytes) -> Result<Json<Vec<Rule>>, Failure> {
    let rules: Vec<Rule> = serde_json::from_slice(&body)
        .map_err(|e| Failure::new(StatusCode::BAD_REQUEST, "bad_request", e.to_string()))?;
    state
        .engine
        .rules()
        .replace(rules)
        .map(Json)
        .map_err(|e| Failure::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_rules", e))
}

/// `POST /api/v1/flows/{id}/replay`: repite el request; devuelve el id del flujo nuevo.
async fn replay_flow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, Failure> {
    let id = parse_id(&id)?;
    if state.engine.store().get(id).is_none() {
        return Err(Failure::not_found(id));
    }
    let new_id = state
        .engine
        .replay_flow(id)
        .await
        .map_err(|e| Failure::new(StatusCode::CONFLICT, "replay_failed", e))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "id": new_id })),
    )
        .into_response())
}

async fn get_flow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<FlowDto>, Failure> {
    let id = parse_id(&id)?;
    let flow = state.engine.store().get(id).ok_or(Failure::not_found(id))?;
    Ok(Json(flow.into()))
}

/// Bytes tal como viajaron (CA 7). `nosniff` + `sandbox`: un HTML capturado no corre en este origen.
async fn get_body(
    State(state): State<AppState>,
    Path((id, side)): Path<(String, String)>,
) -> Result<Response, Failure> {
    let id = parse_id(&id)?;
    let request_side = match side.as_str() {
        "request" => true,
        "response" => false,
        _ => {
            return Err(Failure::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "ruta desconocida",
            ));
        }
    };
    let Some(StoredFlow::Http(record)) = state.engine.store().get(id) else {
        return Err(Failure::not_found(id));
    };
    let Some(bodies) = &record.bodies else {
        return Err(Failure::new(
            StatusCode::CONFLICT,
            "in_progress",
            "el body todavía se está recibiendo",
        ));
    };
    let (body, headers) = if request_side {
        (&bodies.request, &record.head.request_headers)
    } else {
        (&bodies.response, &record.head.response_headers)
    };
    let mut response = Response::new(Body::from(body.data.clone()));
    let out = response.headers_mut();
    let content_type = find_header(headers, "content-type")
        .and_then(|v| HeaderValue::from_str(v).ok())
        .unwrap_or_else(|| HeaderValue::from_static("application/octet-stream"));
    out.insert(header::CONTENT_TYPE, content_type);
    out.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    out.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox"),
    );
    if let Some(encoding) =
        find_header(headers, "content-encoding").and_then(|v| HeaderValue::from_str(v).ok())
    {
        out.insert("x-proxyrr-content-encoding", encoding);
    }
    out.insert(
        "x-proxyrr-truncated",
        HeaderValue::from_static(if body.truncated { "true" } else { "false" }),
    );
    Ok(response)
}

fn find_header<'a>(headers: &'a Headers, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

async fn events(
    State(state): State<AppState>,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    match upgrade {
        Ok(upgrade) => upgrade.on_upgrade(move |socket| stream_events(socket, state)),
        Err(_) => Failure::new(
            StatusCode::BAD_REQUEST,
            "websocket_required",
            "esta ruta es un WebSocket",
        )
        .into_response(),
    }
}

/// `hello` y después los avisos del engine hasta que el cliente cierre o la API se apague (CA 8).
async fn stream_events(mut socket: WebSocket, state: AppState) {
    // Suscribir antes de armar el `hello`: nada queda entre los dos.
    let mut notices = state.engine.subscribe();
    let mut stop = state.stop;
    let hello = WsMessage::Hello {
        status: state.engine.status().await.into(),
    };
    if send(&mut socket, &hello).await.is_err() {
        return;
    }
    loop {
        let message = tokio::select! {
            _ = stop.wait_for(|stopped| *stopped) => break,
            incoming = socket.recv() => match incoming {
                // El cliente no manda nada útil; solo se atiende el cierre (los ping los responde axum).
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => continue,
            },
            notice = notices.recv() => match notice {
                Ok(notice) => WsMessage::from(notice),
                Err(RecvError::Lagged(missed)) => WsMessage::Lagged { missed },
                Err(RecvError::Closed) => break,
            },
        };
        if send(&mut socket, &message).await.is_err() {
            return;
        }
    }
    let _ = socket.send(Message::Close(None)).await;
}

async fn send(socket: &mut WebSocket, message: &WsMessage) -> Result<(), axum::Error> {
    let text = serde_json::to_string(message).expect("los DTO siempre serializan");
    socket.send(Message::Text(text.into())).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        assert!(valid_token("abcdefghijklmnop"));
        assert!(valid_token("A-b_c.d~e0123456789"));
        assert!(!valid_token("corto"));
        assert!(!valid_token("con espacio 1234567"));
        assert!(!valid_token("con&amp=raro12345678"));
        let random = random_token().unwrap();
        assert_eq!(random.len(), 64);
        assert!(valid_token(&random));
        assert_ne!(random, random_token().unwrap());
    }

    #[test]
    fn constant_time_compare() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }

    #[test]
    fn hosts() {
        let with_host = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, value.parse().unwrap());
            headers
        };
        assert!(host_allowed(&with_host("127.0.0.1:9091"), 9091));
        assert!(host_allowed(&with_host("LOCALHOST:9091"), 9091));
        assert!(host_allowed(&with_host("[::1]:9091"), 9091));
        assert!(!host_allowed(&with_host("127.0.0.1:9092"), 9091));
        assert!(!host_allowed(&with_host("evil.example:9091"), 9091));
        assert!(!host_allowed(&with_host("localhost"), 9091));
        assert!(!host_allowed(&HeaderMap::new(), 9091));
    }

    #[test]
    fn query_params() {
        assert_eq!(query_param(Some("a=1&token=xyz"), "token"), Some("xyz"));
        assert_eq!(query_param(Some("token"), "token"), None);
        assert_eq!(query_param(None, "token"), None);
    }
}
