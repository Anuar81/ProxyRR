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
use proxyrr_core::{FlowEvent, Proxy, ProxyConfig};
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
    let first = next_event(&mut events).await;
    let second = next_event(&mut events).await;
    assert_eq!((first.id(), second.id()), (1, 2));
    let FlowEvent::Http(flow) = first else {
        panic!("se esperaba un evento HTTP");
    };
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
