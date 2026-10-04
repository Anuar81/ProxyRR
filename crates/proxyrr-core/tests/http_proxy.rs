//! Integración: proxy real + origen falso en loopback + cliente TCP crudo (CA 1–7 de la spec 0004).

use std::convert::Infallible;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use proxyrr_core::{FlowEvent, HttpBodies, HttpFlow, Proxy, ProxyConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

const LIMIT: Duration = Duration::from_secs(10);

async fn start_proxy() -> Proxy {
    Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        ..ProxyConfig::default()
    })
    .await
    .unwrap()
}

/// Origen que describe lo que recibió. `/status/<n>` responde ese status. Siempre agrega headers
/// hop-by-hop a la respuesta para verificar que el proxy los quita.
async fn start_origin() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let service = service_fn(|req: Request<Incoming>| async move {
                    let status = req
                        .uri()
                        .path()
                        .strip_prefix("/status/")
                        .and_then(|s| s.parse::<u16>().ok())
                        .and_then(|s| StatusCode::from_u16(s).ok())
                        .unwrap_or(StatusCode::OK);
                    let mut text = format!("method={}\nuri={}\n", req.method(), req.uri());
                    for (name, value) in req.headers() {
                        let _ = writeln!(text, "{name}: {}", value.to_str().unwrap_or("?"));
                    }
                    let body = req.into_body().collect().await.unwrap().to_bytes();
                    let _ = write!(text, "body={}", String::from_utf8_lossy(&body));
                    let mut response = Response::new(Full::new(Bytes::from(text)));
                    *response.status_mut() = status;
                    let headers = response.headers_mut();
                    headers.insert("x-origin", "yes".parse().unwrap());
                    headers.insert("keep-alive", "timeout=5".parse().unwrap());
                    headers.insert("connection", "x-secret".parse().unwrap());
                    headers.insert("x-secret", "1".parse().unwrap());
                    Ok::<_, Infallible>(response)
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    addr
}

/// Servidor TCP que devuelve lo que recibe (destino de los túneles).
async fn start_echo() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let (mut read, mut write) = stream.split();
                let _ = tokio::io::copy(&mut read, &mut write).await;
            });
        }
    });
    addr
}

/// Un puerto donde no escucha nadie.
async fn closed_port() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap()
}

struct Reply {
    head: String,
    body: String,
}

impl Reply {
    fn status(&self) -> u16 {
        self.head.split(' ').nth(1).unwrap().parse().unwrap()
    }

    fn has_header(&self, name: &str) -> bool {
        let prefix = format!("{}:", name.to_ascii_lowercase());
        self.head
            .lines()
            .any(|line| line.to_ascii_lowercase().starts_with(&prefix))
    }
}

/// Manda un request crudo (debe llevar `Connection: close`) y lee la respuesta completa.
async fn send(proxy: SocketAddr, request: &str) -> Reply {
    let mut stream = TcpStream::connect(proxy).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    timeout(LIMIT, stream.read_to_end(&mut raw))
        .await
        .expect("el proxy no cerró la conexión")
        .unwrap();
    let raw = String::from_utf8(raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").expect("respuesta sin headers");
    Reply {
        head: head.to_owned(),
        body: body.to_owned(),
    }
}

async fn next_event(events: &mut tokio::sync::broadcast::Receiver<FlowEvent>) -> FlowEvent {
    timeout(LIMIT, events.recv()).await.unwrap().unwrap()
}

async fn next_http(events: &mut tokio::sync::broadcast::Receiver<FlowEvent>) -> HttpFlow {
    loop {
        if let FlowEvent::Http(flow) = next_event(events).await {
            return flow;
        }
    }
}

async fn next_bodies(events: &mut tokio::sync::broadcast::Receiver<FlowEvent>) -> HttpBodies {
    loop {
        if let FlowEvent::HttpBodies(bodies) = next_event(events).await {
            return bodies;
        }
    }
}

fn get(url: &str, host: &str) -> String {
    format!("GET {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
}

#[tokio::test]
async fn forwards_get_in_origin_form() {
    let origin = start_origin().await;
    let proxy = start_proxy().await;
    let reply = send(
        proxy.local_addr(),
        &format!(
            "GET http://{origin}/hello?x=1 HTTP/1.1\r\nHost: {origin}\r\nX-Custom: abc\r\n\
             Proxy-Connection: keep-alive\r\nConnection: close\r\n\r\n"
        ),
    )
    .await;
    assert_eq!(reply.status(), 200);
    assert!(reply.body.contains("method=GET"));
    assert!(reply.body.contains("uri=/hello?x=1"), "{}", reply.body);
    assert!(reply.body.contains(&format!("host: {origin}")));
    assert!(reply.body.contains("x-custom: abc"));
    assert!(reply.has_header("x-origin"));
}

#[tokio::test]
async fn passes_origin_status_through() {
    let origin = start_origin().await;
    let proxy = start_proxy().await;
    let reply = send(
        proxy.local_addr(),
        &get(&format!("http://{origin}/status/404"), &origin.to_string()),
    )
    .await;
    assert_eq!(reply.status(), 404);
}

#[tokio::test]
async fn forwards_post_body() {
    let origin = start_origin().await;
    let proxy = start_proxy().await;
    let body = "hola mundo";
    let reply = send(
        proxy.local_addr(),
        &format!(
            "POST http://{origin}/submit HTTP/1.1\r\nHost: {origin}\r\nContent-Type: text/plain\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
    .await;
    assert_eq!(reply.status(), 200);
    assert!(reply.body.contains("method=POST"));
    assert!(reply.body.contains("body=hola mundo"), "{}", reply.body);
}

#[tokio::test]
async fn forwards_chunked_post_body() {
    let origin = start_origin().await;
    let proxy = start_proxy().await;
    let reply = send(
        proxy.local_addr(),
        &format!(
            "POST http://{origin}/submit HTTP/1.1\r\nHost: {origin}\r\nTransfer-Encoding: chunked\r\n\
             Connection: close\r\n\r\n4\r\nhola\r\n6\r\n mundo\r\n0\r\n\r\n"
        ),
    )
    .await;
    assert_eq!(reply.status(), 200);
    assert!(reply.body.contains("body=hola mundo"), "{}", reply.body);
}

#[tokio::test]
async fn strips_hop_by_hop_both_ways() {
    let origin = start_origin().await;
    let proxy = start_proxy().await;
    let reply = send(
        proxy.local_addr(),
        &format!(
            "GET http://{origin}/ HTTP/1.1\r\nHost: {origin}\r\nConnection: close, x-drop\r\n\
             X-Drop: 1\r\nKeep-Alive: 300\r\nProxy-Authorization: Basic abc\r\nTE: trailers\r\n\
             X-Keep: 1\r\n\r\n"
        ),
    )
    .await;
    assert_eq!(reply.status(), 200);
    for gone in ["x-drop", "keep-alive", "proxy-authorization", "te:"] {
        assert!(
            !reply.body.to_ascii_lowercase().contains(gone),
            "el origen recibió `{gone}`:\n{}",
            reply.body
        );
    }
    assert!(reply.body.contains("x-keep: 1"));
    // Respuesta: el origen mandó keep-alive y `Connection: x-secret` + `x-secret`.
    assert!(!reply.has_header("keep-alive"));
    assert!(!reply.has_header("x-secret"));
    assert!(reply.has_header("x-origin"));
}

#[tokio::test]
async fn upstream_down_is_502() {
    let dead = closed_port().await;
    let proxy = start_proxy().await;
    let mut events = proxy.subscribe();
    let reply = send(
        proxy.local_addr(),
        &get(&format!("http://{dead}/"), &dead.to_string()),
    )
    .await;
    assert_eq!(reply.status(), 502);
    assert!(reply.body.contains("no pudo llegar al origen"));
    let FlowEvent::Http(flow) = next_event(&mut events).await else {
        panic!("se esperaba un evento HTTP");
    };
    assert_eq!(flow.status, 502);
    assert!(flow.error.is_some());
}

#[tokio::test]
async fn origin_form_is_400() {
    let proxy = start_proxy().await;
    let reply = send(proxy.local_addr(), &get("/", "example.com")).await;
    assert_eq!(reply.status(), 400);
    assert!(reply.body.contains("proxy"));
}

#[tokio::test]
async fn https_scheme_without_connect_is_400() {
    let proxy = start_proxy().await;
    let reply = send(
        proxy.local_addr(),
        &get("https://example.com/", "example.com"),
    )
    .await;
    assert_eq!(reply.status(), 400);
    assert!(reply.body.contains("CONNECT"));
}

#[tokio::test]
async fn self_request_is_508() {
    let proxy = start_proxy().await;
    let me = proxy.local_addr();
    let port = me.port();
    for url in [format!("http://{me}/"), format!("http://localhost:{port}/")] {
        let reply = send(me, &get(&url, &me.to_string())).await;
        assert_eq!(reply.status(), 508, "{url}");
    }
}

#[tokio::test]
async fn emits_events_with_incremental_ids() {
    let origin = start_origin().await;
    let proxy = start_proxy().await;
    let mut events = proxy.subscribe();
    let url = format!("http://{origin}/a");
    for _ in 0..2 {
        send(proxy.local_addr(), &get(&url, &origin.to_string())).await;
    }
    let first = next_http(&mut events).await;
    let second = next_http(&mut events).await;
    assert_eq!((first.id, second.id), (1, 2));
    let flow = first;
    assert_eq!(flow.method, "GET");
    assert_eq!(flow.url, url);
    assert_eq!(flow.status, 200);
    assert_eq!(flow.error, None);
    assert!(flow.content_length.unwrap() > 0);
}

/// Manda un CONNECT y lee hasta el fin de los headers de la respuesta.
async fn connect(proxy: SocketAddr, target: &str) -> (TcpStream, String) {
    let mut stream = TcpStream::connect(proxy).await.unwrap();
    stream
        .write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let n = timeout(LIMIT, stream.read(&mut byte))
            .await
            .unwrap()
            .unwrap();
        assert_ne!(n, 0, "el proxy cerró antes de responder");
        head.push(byte[0]);
    }
    (stream, String::from_utf8(head).unwrap())
}

#[tokio::test]
async fn connect_tunnels_bytes() {
    let echo = start_echo().await;
    let proxy = start_proxy().await;
    let mut events = proxy.subscribe();
    let (mut stream, head) = connect(proxy.local_addr(), &echo.to_string()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    stream.write_all(b"ping por el tunel").await.unwrap();
    let mut buf = [0u8; 17];
    timeout(LIMIT, stream.read_exact(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf, b"ping por el tunel");
    let FlowEvent::Tunnel(flow) = next_event(&mut events).await else {
        panic!("se esperaba un evento de túnel");
    };
    assert_eq!(flow.authority, echo.to_string());
    assert_eq!(flow.status, 200);
}

#[tokio::test]
async fn connect_unreachable_is_502() {
    let dead = closed_port().await;
    let proxy = start_proxy().await;
    let mut events = proxy.subscribe();
    let (_, head) = connect(proxy.local_addr(), &dead.to_string()).await;
    assert!(head.starts_with("HTTP/1.1 502"), "{head}");
    let FlowEvent::Tunnel(flow) = next_event(&mut events).await else {
        panic!("se esperaba un evento de túnel");
    };
    assert_eq!(flow.status, 502);
    assert!(flow.error.is_some());
}

#[tokio::test]
async fn connect_to_self_is_508() {
    let proxy = start_proxy().await;
    let me = proxy.local_addr();
    let (_, head) = connect(me, &me.to_string()).await;
    assert!(head.starts_with("HTTP/1.1 508"), "{head}");
}

#[tokio::test]
async fn shutdown_releases_the_port() {
    let proxy = start_proxy().await;
    let addr = proxy.local_addr();
    proxy.shutdown().await;
    // Si el listener sigue vivo, volver a escuchar en el mismo puerto fallaría.
    let again = Proxy::start(ProxyConfig {
        listen: addr,
        ..ProxyConfig::default()
    })
    .await;
    assert!(again.is_ok(), "{again:?}");
}

// ---- Captura (spec 0007) ----

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

#[tokio::test]
async fn captures_headers_and_bodies() {
    let origin = start_origin().await;
    let proxy = start_proxy().await;
    let mut events = proxy.subscribe();
    let reply = send(
        proxy.local_addr(),
        &format!(
            "POST http://{origin}/cap HTTP/1.1\r\nHost: {origin}\r\nX-Custom: abc\r\n\
             Content-Length: 10\r\nConnection: close\r\n\r\nhola mundo"
        ),
    )
    .await;
    let flow = next_http(&mut events).await;
    // Headers tal como viajaron: los del cliente y los del origen, antes de quitar hop-by-hop.
    assert_eq!(header(&flow.request_headers, "x-custom"), Some("abc"));
    assert_eq!(header(&flow.request_headers, "connection"), Some("close"));
    assert_eq!(header(&flow.response_headers, "x-origin"), Some("yes"));
    assert_eq!(
        header(&flow.response_headers, "keep-alive"),
        Some("timeout=5")
    );

    let bodies = next_bodies(&mut events).await;
    assert_eq!(bodies.id, flow.id);
    assert_eq!(bodies.request.data, "hola mundo");
    assert_eq!(bodies.request.size, 10);
    assert!(bodies.request.complete);
    assert_eq!(bodies.response.data, reply.body.as_bytes());
    assert_eq!(bodies.response.size, reply.body.len() as u64);
    assert!(bodies.response.complete);
    assert!(!bodies.response.truncated);
}

#[tokio::test]
async fn truncates_capture_but_forwards_everything() {
    let origin = start_origin().await;
    let proxy = Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        max_body_capture: 4,
        ..ProxyConfig::default()
    })
    .await
    .unwrap();
    let mut events = proxy.subscribe();
    let reply = send(
        proxy.local_addr(),
        &format!(
            "POST http://{origin}/big HTTP/1.1\r\nHost: {origin}\r\nContent-Length: 10\r\n\
             Connection: close\r\n\r\n0123456789"
        ),
    )
    .await;
    assert!(reply.body.contains("body=0123456789"), "{}", reply.body);
    let bodies = next_bodies(&mut events).await;
    assert_eq!(bodies.request.data, "0123");
    assert_eq!(bodies.request.size, 10);
    assert!(bodies.request.truncated);
    assert_eq!(bodies.response.data.len(), 4);
    assert!(bodies.response.truncated);
    assert_eq!(bodies.response.size, reply.body.len() as u64);
}

#[tokio::test]
async fn rejected_flow_is_captured() {
    let proxy = start_proxy().await;
    let mut events = proxy.subscribe();
    let reply = send(proxy.local_addr(), &get("/", "example.com")).await;
    assert_eq!(reply.status(), 400);
    let flow = next_http(&mut events).await;
    assert_eq!(
        header(&flow.response_headers, "content-type"),
        Some("text/plain; charset=utf-8")
    );
    let bodies = next_bodies(&mut events).await;
    assert!(
        String::from_utf8_lossy(&bodies.response.data).contains("Esto es ProxyRR"),
        "{:?}",
        bodies.response
    );
}

/// Origen crudo: manda headers + `first`, espera la señal y recién ahí manda `rest` (o corta).
async fn start_raw_origin(
    head: &'static str,
    first: &'static str,
    rest: Option<&'static str>,
) -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (go, wait) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf).await;
        stream.write_all(head.as_bytes()).await.unwrap();
        stream.write_all(first.as_bytes()).await.unwrap();
        let _ = wait.await;
        if let Some(rest) = rest {
            stream.write_all(rest.as_bytes()).await.unwrap();
        }
        // Al salir se cierra la conexión: con `rest = None` es un corte a mitad de body.
    });
    (addr, go)
}

#[tokio::test]
async fn capture_does_not_buffer_the_stream() {
    let (origin, go) = start_raw_origin(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
        "7\r\nprimero\r\n",
        Some("7\r\nsegundo\r\n0\r\n\r\n"),
    )
    .await;
    let proxy = start_proxy().await;
    let mut events = proxy.subscribe();
    let mut stream = TcpStream::connect(proxy.local_addr()).await.unwrap();
    stream
        .write_all(get(&format!("http://{origin}/stream"), &origin.to_string()).as_bytes())
        .await
        .unwrap();
    // El primer chunk tiene que llegar al cliente ANTES de que el origen termine.
    let mut seen = Vec::new();
    let mut buf = [0u8; 1024];
    while !String::from_utf8_lossy(&seen).contains("primero") {
        let n = timeout(Duration::from_secs(5), stream.read(&mut buf))
            .await
            .expect("el proxy retuvo el body en vez de reenviarlo")
            .unwrap();
        assert_ne!(n, 0);
        seen.extend_from_slice(&buf[..n]);
    }
    go.send(()).unwrap();
    let mut rest = Vec::new();
    timeout(LIMIT, stream.read_to_end(&mut rest))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&rest).contains("segundo"));
    let bodies = next_bodies(&mut events).await;
    assert_eq!(bodies.response.data, "primerosegundo");
    assert!(bodies.response.complete);
}

#[tokio::test]
async fn origin_cut_mid_body_is_incomplete() {
    let (origin, go) = start_raw_origin(
        "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n",
        "0123456789",
        None,
    )
    .await;
    let proxy = start_proxy().await;
    let mut events = proxy.subscribe();
    let mut stream = TcpStream::connect(proxy.local_addr()).await.unwrap();
    stream
        .write_all(get(&format!("http://{origin}/cut"), &origin.to_string()).as_bytes())
        .await
        .unwrap();
    let mut buf = [0u8; 1024];
    let _ = timeout(LIMIT, stream.read(&mut buf)).await.unwrap();
    go.send(()).unwrap(); // el origen cierra con 90 bytes pendientes
    let bodies = next_bodies(&mut events).await;
    assert_eq!(bodies.response.data, "0123456789");
    assert_eq!(bodies.response.size, 10);
    assert!(!bodies.response.complete);
}

#[tokio::test]
async fn shared_flow_ids_never_repeat_across_instances() {
    let ids = proxyrr_core::FlowIds::default();
    let origin = start_origin().await;
    let mut seen = Vec::new();
    for _ in 0..2 {
        let proxy = Proxy::start(ProxyConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            flow_ids: ids.clone(),
            ..ProxyConfig::default()
        })
        .await
        .unwrap();
        let mut events = proxy.subscribe();
        let request =
            format!("GET http://{origin}/ HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n");
        send(proxy.local_addr(), &request).await;
        seen.push(next_http(&mut events).await.id);
        proxy.shutdown().await;
    }
    assert_eq!(seen, [1, 2]);
}

#[derive(Debug)]
struct EchoSite;

impl proxyrr_core::LocalSite for EchoSite {
    fn respond(&self, request: &proxyrr_core::LocalRequest<'_>) -> proxyrr_core::LocalResponse {
        proxyrr_core::LocalResponse {
            status: 200,
            headers: vec![("content-type".into(), "text/plain".into())],
            body: Bytes::from(format!(
                "direct={} path={} ua={}",
                request.direct,
                request.path,
                request.user_agent.unwrap_or("-")
            )),
        }
    }
}

#[tokio::test]
async fn local_site_is_served_by_the_proxy_and_captured() {
    let proxy = Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        local_site: Some(std::sync::Arc::new(EchoSite)),
        ..ProxyConfig::default()
    })
    .await
    .unwrap();
    let mut events = proxy.subscribe();
    let addr = proxy.local_addr();

    // A través del proxy: `proxyrr.cert` no se resuelve ni se reenvía.
    let via = send(
        addr,
        "GET http://proxyrr.cert/ca.pem HTTP/1.1\r\nHost: proxyrr.cert\r\nUser-Agent: test-ua\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(via.status(), 200, "{}", via.head);
    assert_eq!(via.body, "direct=false path=/ca.pem ua=test-ua");
    let flow = next_http(&mut events).await;
    assert_eq!(flow.url, "http://proxyrr.cert/ca.pem");
    assert_eq!(flow.status, 200);
    assert!(flow.error.is_none());

    // Directo al proxy (dispositivo sin proxy configurado todavía).
    let direct = send(
        addr,
        &format!("GET /cert HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
    )
    .await;
    assert_eq!(direct.body, "direct=true path=/ ua=-");
    let other = send(
        addr,
        &format!("GET / HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
    )
    .await;
    assert_eq!(other.status(), 400, "fuera de /cert sigue siendo un proxy");
    proxy.shutdown().await;
}

// ---- Spec 0010: deuda técnica del motor (TD-001, TD-003, TD-004) ----

#[tokio::test]
async fn connect_timeout_is_configurable() {
    // TEST-NET-1 (RFC 5737): no rutea a ningún lado. Según la red falla al instante (sin ruta)
    // o queda colgado hasta el timeout; en ningún caso puede tardar más que el timeout pedido.
    let proxy = Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        connect_timeout: Duration::from_millis(300),
        ..ProxyConfig::default()
    })
    .await
    .unwrap();
    let started = std::time::Instant::now();
    let (_, head) = connect(proxy.local_addr(), "192.0.2.1:81").await;
    assert!(
        head.starts_with("HTTP/1.1 504") || head.starts_with("HTTP/1.1 502"),
        "{head}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn default_connect_timeout_is_30_seconds() {
    assert_eq!(
        ProxyConfig::default().connect_timeout,
        Duration::from_secs(30)
    );
}

/// Primera IP no loopback de esta máquina (la de la ruta por defecto), si tiene red.
fn lan_ip() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    // `connect` en UDP no manda nada: solo elige la interfaz de salida.
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

#[tokio::test]
async fn all_interfaces_listener_detects_its_own_lan_ip() {
    let proxy = Proxy::start(ProxyConfig {
        listen: "0.0.0.0:0".parse().unwrap(),
        ..ProxyConfig::default()
    })
    .await
    .unwrap();
    let port = proxy.local_addr().port();
    let local = SocketAddr::from(([127, 0, 0, 1], port));
    let (_, head) = connect(local, &format!("127.0.0.1:{port}")).await;
    assert!(head.starts_with("HTTP/1.1 508"), "{head}");
    let Some(ip) = lan_ip() else {
        eprintln!("sin IP de LAN: se omite la parte de la IP propia");
        return;
    };
    let me = SocketAddr::new(ip, port);
    let (_, head) = connect(local, &me.to_string()).await;
    assert!(head.starts_with("HTTP/1.1 508"), "{me}: {head}");
    let reply = send(
        local,
        &format!("GET http://{me}/ HTTP/1.1\r\nHost: {me}\r\nConnection: close\r\n\r\n"),
    )
    .await;
    assert_eq!(reply.status(), 508, "{}", reply.head);
}

#[tokio::test]
async fn shutdown_closes_open_tunnels_promptly() {
    let echo = start_echo().await;
    let proxy = start_proxy().await;
    let (mut stream, head) = connect(proxy.local_addr(), &echo.to_string()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let started = std::time::Instant::now();
    proxy.shutdown_within(Duration::from_secs(10)).await;
    // Un túnel no tiene requests que esperar: se cierra sin consumir el plazo.
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    let mut buf = [0u8; 8];
    let n = timeout(LIMIT, stream.read(&mut buf))
        .await
        .unwrap()
        .unwrap_or(0);
    assert_eq!(n, 0, "el túnel debía quedar cerrado");
}

#[tokio::test]
async fn shutdown_waits_for_in_flight_requests() {
    let (origin, go) = start_raw_origin(
        "HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n",
        "hola ",
        Some("mundo!"),
    )
    .await;
    let proxy = start_proxy().await;
    let mut stream = TcpStream::connect(proxy.local_addr()).await.unwrap();
    stream
        .write_all(get(&format!("http://{origin}/lento"), &origin.to_string()).as_bytes())
        .await
        .unwrap();
    // Esperar a que el request esté en curso (llegaron los headers) antes de apagar.
    let mut seen = Vec::new();
    let mut buf = [0u8; 256];
    while !String::from_utf8_lossy(&seen).contains("hola") {
        let n = timeout(LIMIT, stream.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_ne!(n, 0);
        seen.extend_from_slice(&buf[..n]);
    }
    let shutdown = tokio::spawn(proxy.shutdown_within(Duration::from_secs(10)));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !shutdown.is_finished(),
        "el apagado no esperó el request en curso"
    );
    go.send(()).unwrap();
    timeout(LIMIT, stream.read_to_end(&mut seen))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&seen).ends_with("hola mundo!"));
    timeout(LIMIT, shutdown).await.unwrap().unwrap();
}

#[tokio::test]
async fn shutdown_cuts_what_outlives_the_grace_period() {
    // Origen que manda headers y parte del body, y nunca termina.
    let (origin, _never) = start_raw_origin(
        "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n",
        "parcial",
        None,
    )
    .await;
    let proxy = start_proxy().await;
    let mut stream = TcpStream::connect(proxy.local_addr()).await.unwrap();
    stream
        .write_all(get(&format!("http://{origin}/eterno"), &origin.to_string()).as_bytes())
        .await
        .unwrap();
    let mut buf = [0u8; 256];
    let n = timeout(LIMIT, stream.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_ne!(n, 0);
    let started = std::time::Instant::now();
    proxy.shutdown_within(Duration::from_millis(300)).await;
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(300), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(4), "{elapsed:?}");
    let mut rest = Vec::new();
    // Cortada: el cliente ve el cierre (EOF o reset), no se queda colgado.
    let _ = timeout(LIMIT, stream.read_to_end(&mut rest)).await.unwrap();
}
