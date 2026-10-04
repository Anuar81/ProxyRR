//! Integración de la API de control: servidor real, proxy real, origen falso y clientes crudos (spec 0008).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use proxyrr_api::{ApiConfig, ApiError, ApiServer, Engine, EngineOptions, ProxySettings};
use proxyrr_cert::CertificateAuthority;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const LIMIT: Duration = Duration::from_secs(10);
const PAGE: &str = "<html><script>alert(1)</script></html>";

fn loopback() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

async fn start(engine: Engine) -> (Arc<Engine>, ApiServer) {
    let engine = Arc::new(engine);
    let api = ApiServer::start(
        ApiConfig {
            listen: loopback(),
            token: None,
        },
        Arc::clone(&engine),
    )
    .await
    .unwrap();
    (engine, api)
}

/// Origen HTTP que responde siempre la misma página HTML.
async fn start_origin() -> SocketAddr {
    let listener = TcpListener::bind(loopback()).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                read_head(&mut stream).await;
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{PAGE}",
                    PAGE.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });
    addr
}

async fn read_head(stream: &mut TcpStream) {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).await.unwrap_or(0) == 0 {
            return;
        }
        buf.push(byte[0]);
    }
}

struct Reply {
    status: u16,
    head: String,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("no es JSON ({e}): {}", String::from_utf8_lossy(&self.body)))
    }

    fn header(&self, name: &str) -> Option<String> {
        let prefix = format!("{}:", name.to_ascii_lowercase());
        self.head.lines().find_map(|line| {
            line.to_ascii_lowercase()
                .starts_with(&prefix)
                .then(|| line[prefix.len()..].trim().to_owned())
        })
    }

    fn error_code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap().to_owned()
    }
}

/// Request HTTP/1.1 crudo con `Connection: close`.
async fn raw(
    addr: SocketAddr,
    host: &str,
    method: &str,
    path: &str,
    extra: &str,
    body: &str,
) -> Reply {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut data = Vec::new();
    timeout(LIMIT, stream.read_to_end(&mut data))
        .await
        .expect("la API no cerró la conexión")
        .unwrap();
    let split = data
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("respuesta sin headers");
    let head = String::from_utf8(data[..split].to_vec()).unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    Reply {
        status,
        head,
        body: data[split + 4..].to_vec(),
    }
}

async fn call(api: &ApiServer, method: &str, path: &str, body: &str) -> Reply {
    let auth = format!("Authorization: Bearer {}\r\n", api.token());
    raw(
        api.local_addr(),
        &api.local_addr().to_string(),
        method,
        path,
        &auth,
        body,
    )
    .await
}

/// Manda un GET por el proxy y espera a que el flujo quede completo en el store.
async fn through_proxy(api: &ApiServer, proxy: SocketAddr, origin: SocketAddr, path: &str) -> u64 {
    let url = format!("http://{origin}{path}");
    let reply = raw(proxy, &origin.to_string(), "GET", &url, "", "").await;
    assert_eq!(reply.status, 200);
    timeout(LIMIT, async {
        loop {
            let flows = call(api, "GET", "/api/v1/flows", "").await.json();
            if let Some(flow) = flows
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["url"] == url && f["in_progress"] == false)
            {
                return flow["id"].as_u64().unwrap();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("el flujo no llegó al store")
}

fn proxy_addr(status: &Value) -> SocketAddr {
    status["listen"].as_str().unwrap().parse().unwrap()
}

#[tokio::test]
async fn refuses_non_loopback_and_bad_tokens() {
    let engine = Arc::new(Engine::new(EngineOptions::default()));
    let outside = ApiServer::start(
        ApiConfig {
            listen: "0.0.0.0:0".parse().unwrap(),
            token: None,
        },
        Arc::clone(&engine),
    )
    .await;
    assert!(matches!(outside, Err(ApiError::NotLoopback(_))));
    let short = ApiServer::start(
        ApiConfig {
            listen: loopback(),
            token: Some("corto".into()),
        },
        Arc::clone(&engine),
    )
    .await;
    assert!(matches!(short, Err(ApiError::InvalidToken)));
    let fixed = ApiServer::start(
        ApiConfig {
            listen: loopback(),
            token: Some("token-fijo-para-scripts".into()),
        },
        engine,
    )
    .await
    .unwrap();
    assert_eq!(fixed.token(), "token-fijo-para-scripts");
    assert_eq!(call(&fixed, "GET", "/api/v1/status", "").await.status, 200);
    fixed.shutdown().await;
}

#[tokio::test]
async fn requires_token_and_local_host() {
    let (_engine, api) = start(Engine::new(EngineOptions::default())).await;
    let addr = api.local_addr();
    let host = addr.to_string();

    let missing = raw(addr, &host, "GET", "/api/v1/status", "", "").await;
    assert_eq!(missing.status, 401);
    assert_eq!(missing.error_code(), "unauthorized");

    let wrong = raw(
        addr,
        &host,
        "GET",
        "/api/v1/status",
        "Authorization: Bearer nope\r\n",
        "",
    )
    .await;
    assert_eq!(wrong.status, 401);

    let query = format!("/api/v1/status?token={}", api.token());
    let by_query = raw(addr, &host, "GET", &query, "", "").await;
    assert_eq!(
        by_query.status, 401,
        "el token por query solo vale en /events"
    );

    let auth = format!("Authorization: Bearer {}\r\n", api.token());
    let rebinding = raw(
        addr,
        &format!("evil.example:{}", addr.port()),
        "GET",
        "/api/v1/status",
        &auth,
        "",
    )
    .await;
    assert_eq!(rebinding.status, 403);
    assert_eq!(rebinding.error_code(), "forbidden_host");

    let localhost = raw(
        addr,
        &format!("localhost:{}", addr.port()),
        "GET",
        "/api/v1/status",
        &auth,
        "",
    )
    .await;
    assert_eq!(localhost.status, 200);
    assert_eq!(
        localhost.header("cache-control").as_deref(),
        Some("no-store")
    );
    let status = localhost.json();
    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(status["proxy"]["running"], false);
    assert_eq!(status["flows"], 0);
    api.shutdown().await;
}

/// Detalle del flujo `id` (el GET a `start_origin`) y sus dos bodies (CA 7).
async fn assert_flow_detail_and_bodies(api: &ApiServer, id: u64) {
    let flow = call(api, "GET", &format!("/api/v1/flows/{id}"), "")
        .await
        .json();
    assert_eq!(flow["kind"], "http");
    assert_eq!(flow["method"], "GET");
    assert_eq!(flow["status"], 200);
    assert_eq!(flow["in_progress"], false);
    assert_eq!(flow["response"]["body"]["size"], PAGE.len());
    assert_eq!(flow["response"]["body"]["truncated"], false);
    let response_headers = flow["response"]["headers"].as_array().unwrap();
    assert!(
        response_headers
            .iter()
            .any(|h| h[0] == "content-type" && h[1] == "text/html")
    );

    let body = call(api, "GET", &format!("/api/v1/flows/{id}/response/body"), "").await;
    assert_eq!(body.status, 200);
    assert_eq!(body.body, PAGE.as_bytes());
    assert_eq!(body.header("content-type").as_deref(), Some("text/html"));
    assert_eq!(
        body.header("x-content-type-options").as_deref(),
        Some("nosniff")
    );
    assert_eq!(
        body.header("content-security-policy").as_deref(),
        Some("sandbox")
    );
    assert_eq!(body.header("x-proxyrr-truncated").as_deref(), Some("false"));
    let request_body = call(api, "GET", &format!("/api/v1/flows/{id}/request/body"), "").await;
    assert_eq!(request_body.status, 200);
    assert!(request_body.body.is_empty());
}

#[tokio::test]
async fn capture_inspect_restart_and_clear() {
    let (_engine, api) = start(Engine::new(EngineOptions::default())).await;
    let origin = start_origin().await;

    let started = call(
        &api,
        "POST",
        "/api/v1/proxy/start",
        r#"{"listen":"127.0.0.1:0"}"#,
    )
    .await;
    assert_eq!(
        started.status,
        200,
        "{}",
        String::from_utf8_lossy(&started.body)
    );
    let started = started.json();
    assert_eq!(started["running"], true);
    assert_eq!(started["mitm"], false);
    let proxy = proxy_addr(&started);

    let first = through_proxy(&api, proxy, origin, "/uno").await;
    assert_flow_detail_and_bodies(&api, first).await;

    // Apagar conserva los flujos; prender de nuevo no repite ids.
    let stopped = call(&api, "POST", "/api/v1/proxy/stop", "").await.json();
    assert_eq!(stopped["running"], false);
    let again = call(&api, "POST", "/api/v1/proxy/stop", "").await;
    assert_eq!(again.status, 200, "stop es idempotente");
    let status = call(&api, "GET", "/api/v1/status", "").await.json();
    assert_eq!(status["proxy"]["running"], false);
    assert_eq!(status["flows"], 1);

    let restarted = call(
        &api,
        "POST",
        "/api/v1/proxy/start",
        r#"{"listen":"127.0.0.1:0"}"#,
    )
    .await
    .json();
    let second = through_proxy(&api, proxy_addr(&restarted), origin, "/dos").await;
    assert!(
        second > first,
        "ids únicos entre arranques: {first} → {second}"
    );

    let after = call(&api, "GET", &format!("/api/v1/flows?after={first}"), "")
        .await
        .json();
    let ids: Vec<u64> = after
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, [second]);

    let cleared = call(&api, "DELETE", "/api/v1/flows", "").await;
    assert_eq!(cleared.status, 204);
    assert_eq!(
        call(&api, "GET", "/api/v1/flows", "").await.json(),
        Value::Array(Vec::new())
    );
    call(&api, "POST", "/api/v1/proxy/stop", "").await;
    api.shutdown().await;
}

#[tokio::test]
async fn errors_are_json() {
    let (engine, api) = start(Engine::new(EngineOptions::default())).await;

    let no_ca = call(
        &api,
        "POST",
        "/api/v1/proxy/start",
        r#"{"listen":"127.0.0.1:0","mitm":true}"#,
    )
    .await;
    assert_eq!(no_ca.status, 422);
    assert_eq!(no_ca.error_code(), "no_ca");

    let bad_json = call(&api, "POST", "/api/v1/proxy/start", "{no es json").await;
    assert_eq!(bad_json.status, 400);
    assert_eq!(bad_json.error_code(), "bad_request");
    let bad_listen = call(&api, "POST", "/api/v1/proxy/start", r#"{"listen":"acá"}"#).await;
    assert_eq!(bad_listen.error_code(), "bad_request");
    let unknown_field = call(
        &api,
        "POST",
        "/api/v1/proxy/start",
        r#"{"lisen":"127.0.0.1:0"}"#,
    )
    .await;
    assert_eq!(
        unknown_field.status, 400,
        "un typo no debe prender el proxy en el puerto por defecto"
    );

    let ok = call(
        &api,
        "POST",
        "/api/v1/proxy/start",
        r#"{"listen":"127.0.0.1:0"}"#,
    )
    .await;
    assert_eq!(ok.status, 200);
    let twice = call(
        &api,
        "POST",
        "/api/v1/proxy/start",
        r#"{"listen":"127.0.0.1:0"}"#,
    )
    .await;
    assert_eq!(twice.status, 409);
    assert_eq!(twice.error_code(), "already_running");
    engine.stop_proxy().await;

    // Puerto ocupado.
    let taken = TcpListener::bind(loopback()).await.unwrap();
    let body = format!(r#"{{"listen":"{}"}}"#, taken.local_addr().unwrap());
    let busy = call(&api, "POST", "/api/v1/proxy/start", &body).await;
    assert_eq!(busy.status, 409);
    assert_eq!(busy.error_code(), "bind_failed");

    for path in [
        "/api/v1/flows/99",
        "/api/v1/flows/abc",
        "/api/v1/flows/1/response/body",
        "/api/v1/flows/1/otro/body",
        "/nada",
    ] {
        let reply = call(&api, "GET", path, "").await;
        assert_eq!(reply.status, 404, "{path}");
        assert_eq!(reply.error_code(), "not_found", "{path}");
    }
    let wrong_method = call(&api, "PUT", "/api/v1/status", "").await;
    assert_eq!(wrong_method.status, 405);
    assert_eq!(wrong_method.error_code(), "method_not_allowed");
    let not_ws = call(&api, "GET", "/api/v1/events", "").await;
    assert_eq!(not_ws.status, 400);
    assert_eq!(not_ws.error_code(), "websocket_required");
    api.shutdown().await;
}

#[tokio::test]
async fn mitm_starts_when_the_engine_has_a_ca() {
    let dir = tempfile::tempdir().unwrap();
    let ca = CertificateAuthority::load_or_create(dir.path()).unwrap();
    let engine = Engine::new(EngineOptions {
        ca: Some(Arc::new(ca)),
        ..EngineOptions::default()
    });
    let (engine, api) = start(engine).await;
    let reply = call(
        &api,
        "POST",
        "/api/v1/proxy/start",
        r#"{"listen":"127.0.0.1:0","mitm":true,"bypass":["*.example.com"]}"#,
    )
    .await
    .json();
    assert_eq!(reply["mitm"], true);
    assert_eq!(reply["bypass"], serde_json::json!(["*.example.com"]));
    engine.stop_proxy().await;
    api.shutdown().await;
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

async fn next_json(ws: &mut Ws) -> Value {
    loop {
        let message = timeout(LIMIT, ws.next())
            .await
            .expect("sin mensajes")
            .unwrap()
            .unwrap();
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

async fn next_of(ws: &mut Ws, kind: &str) -> Value {
    loop {
        let message = next_json(ws).await;
        if message["type"] == kind {
            return message;
        }
    }
}

#[tokio::test]
async fn websocket_streams_live_notices() {
    let (engine, api) = start(Engine::new(EngineOptions::default())).await;
    let origin = start_origin().await;
    let url = format!("ws://{}/api/v1/events", api.local_addr());

    let denied = tokio_tungstenite::connect_async(url.as_str()).await;
    assert!(denied.is_err(), "sin token no hay WebSocket");

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("{url}?token={}", api.token()))
        .await
        .unwrap();
    let hello = next_json(&mut ws).await;
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["status"]["proxy"]["running"], false);

    let status = engine
        .start_proxy(ProxySettings {
            listen: loopback(),
            ..ProxySettings::default()
        })
        .await
        .unwrap();
    let proxy = next_of(&mut ws, "proxy").await;
    assert_eq!(proxy["proxy"]["running"], true);

    let target = format!("http://{origin}/ws");
    raw(
        status.listen.unwrap(),
        &origin.to_string(),
        "GET",
        &target,
        "",
        "",
    )
    .await;
    let done = timeout(LIMIT, async {
        loop {
            let flow = next_of(&mut ws, "flow").await;
            assert_eq!(flow["flow"]["url"], target.as_str());
            if flow["flow"]["in_progress"] == false {
                return flow;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(done["flow"]["response_size"], PAGE.len());

    engine.clear();
    assert_eq!(next_of(&mut ws, "cleared").await["type"], "cleared");
    engine.stop_proxy().await;
    assert_eq!(next_of(&mut ws, "proxy").await["proxy"]["running"], false);

    // Con un header Authorization también vale.
    let mut request = url.as_str().into_client_request().unwrap();
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", api.token()).parse().unwrap(),
    );
    let (mut by_header, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(next_json(&mut by_header).await["type"], "hello");
    by_header.close(None).await.unwrap();

    // Apagar la API cierra los WebSocket abiertos en vez de quedarse esperando.
    timeout(LIMIT, api.shutdown())
        .await
        .expect("shutdown colgado con un WebSocket abierto");
    let end = timeout(LIMIT, async {
        loop {
            match ws.next().await {
                None | Some(Ok(Message::Close(_)) | Err(_)) => return,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(end.is_ok(), "el cliente no vio el cierre");
}

#[tokio::test]
async fn har_export_contains_captured_flows() {
    let (_engine, api) = start(Engine::new(EngineOptions::default())).await;
    let origin = start_origin().await;
    let started = call(
        &api,
        "POST",
        "/api/v1/proxy/start",
        r#"{"listen":"127.0.0.1:0"}"#,
    )
    .await
    .json();
    let proxy = proxy_addr(&started);
    through_proxy(&api, proxy, origin, "/har?x=1").await;

    let reply = call(&api, "GET", "/api/v1/har", "").await;
    assert_eq!(reply.status, 200);
    assert!(
        reply
            .header("content-disposition")
            .is_some_and(|v| v.contains("proxyrr.har"))
    );
    let har = reply.json();
    assert_eq!(har["log"]["version"], "1.2");
    let entry = &har["log"]["entries"][0];
    assert_eq!(entry["request"]["url"], format!("http://{origin}/har?x=1"));
    assert_eq!(entry["request"]["queryString"][0]["name"], "x");
    assert_eq!(entry["response"]["status"], 200);
    assert!(entry["startedDateTime"].as_str().unwrap().ends_with('Z'));
}
