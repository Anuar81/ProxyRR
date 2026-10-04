//! Integración: reglas aplicadas por el proxy real contra un origen en loopback (spec 0011).

use std::convert::Infallible;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use proxyrr_core::{FlowEvent, HttpBodies, HttpFlow, Proxy, ProxyConfig, Replay, Verdict};
use proxyrr_rules::{Action, BreakpointEvent, MapLocal, MapRemote, Rule, Rules};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio::time::timeout;

const LIMIT: Duration = Duration::from_secs(10);

/// Origen que describe lo que recibió y responde con validadores de caché.
async fn start_origin() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let service = service_fn(|req: Request<Incoming>| async move {
                    let mut text = format!("method={}\nuri={}\n", req.method(), req.uri());
                    for (name, value) in req.headers() {
                        let _ = writeln!(text, "{name}: {}", value.to_str().unwrap_or("?"));
                    }
                    let body = req.into_body().collect().await.unwrap().to_bytes();
                    let _ = write!(text, "body={}", String::from_utf8_lossy(&body));
                    let mut response = Response::new(Full::new(Bytes::from(text)));
                    response
                        .headers_mut()
                        .insert("etag", "\"v1\"".parse().unwrap());
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

async fn start_proxy(rules: &Arc<Rules>, max_body_capture: Option<usize>) -> Proxy {
    let mut config = ProxyConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        hook: Some(Arc::clone(rules) as _),
        ..ProxyConfig::default()
    };
    if let Some(limit) = max_body_capture {
        config.max_body_capture = limit;
    }
    Proxy::start(config).await.unwrap()
}

struct Reply {
    head: String,
    body: String,
}

impl Reply {
    fn status(&self) -> u16 {
        self.head.split(' ').nth(1).unwrap().parse().unwrap()
    }

    fn header(&self, name: &str) -> Option<String> {
        let prefix = format!("{}:", name.to_ascii_lowercase());
        self.head.lines().find_map(|line| {
            line.to_ascii_lowercase()
                .starts_with(&prefix)
                .then(|| line[prefix.len()..].trim().to_owned())
        })
    }
}

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

fn get(url: &str, host: &str, extra: &str) -> String {
    format!("GET {url} HTTP/1.1\r\nHost: {host}\r\n{extra}Connection: close\r\n\r\n")
}

fn post(url: &str, host: &str, body: &str) -> String {
    format!(
        "POST {url} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

async fn next_http(events: &mut broadcast::Receiver<FlowEvent>) -> HttpFlow {
    loop {
        if let FlowEvent::Http(flow) = timeout(LIMIT, events.recv()).await.unwrap().unwrap() {
            return flow;
        }
    }
}

async fn next_bodies(events: &mut broadcast::Receiver<FlowEvent>) -> HttpBodies {
    loop {
        if let FlowEvent::HttpBodies(b) = timeout(LIMIT, events.recv()).await.unwrap().unwrap() {
            return b;
        }
    }
}

fn rule(name: &str, url: &str, action: Action) -> Rule {
    Rule {
        id: 0,
        name: name.into(),
        enabled: true,
        method: None,
        url: url.into(),
        regex: false,
        action,
    }
}

fn rules_with(list: Vec<Rule>) -> Arc<Rules> {
    let rules = Arc::new(Rules::in_memory());
    rules.replace(list).unwrap();
    rules
}

#[tokio::test]
async fn map_local_responds_without_origin() {
    // Nadie escucha en este puerto: si el proxy fuera al origen, daría 502.
    let closed = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();
    let rules = rules_with(vec![rule(
        "mock",
        &format!("http://{closed}/api/*"),
        Action::MapLocal(MapLocal {
            status: 201,
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("content-length".into(), "999".into()),
            ],
            body: r#"{"mock":true}"#.into(),
            file: None,
        }),
    )]);
    let proxy = start_proxy(&rules, None).await;
    let mut events = proxy.subscribe();
    let reply = send(
        proxy.local_addr(),
        &post(
            &format!("http://{closed}/api/users"),
            &closed.to_string(),
            "datos",
        ),
    )
    .await;
    assert_eq!(reply.status(), 201);
    assert_eq!(reply.body, r#"{"mock":true}"#);
    assert_eq!(
        reply.header("content-length").as_deref(),
        Some("13"),
        "se recalcula"
    );
    let flow = next_http(&mut events).await;
    assert_eq!(flow.rules, ["mock"]);
    assert!(flow.error.is_none());
    let bodies = next_bodies(&mut events).await;
    assert_eq!(
        bodies.request.data, "datos",
        "el body del request se captura igual"
    );
    assert_eq!(bodies.response.data, r#"{"mock":true}"#);
}

#[tokio::test]
async fn map_remote_redirects() {
    let origin = start_origin().await;
    let rules = rules_with(vec![rule(
        "a local",
        "http://api.prod.test/*",
        Action::MapRemote(MapRemote {
            host: Some("127.0.0.1".into()),
            port: Some(origin.port()),
            ..MapRemote::default()
        }),
    )]);
    let proxy = start_proxy(&rules, None).await;
    let mut events = proxy.subscribe();
    let reply = send(
        proxy.local_addr(),
        &get("http://api.prod.test/v1/me?x=1", "api.prod.test", ""),
    )
    .await;
    assert_eq!(reply.status(), 200);
    assert!(reply.body.contains("uri=/v1/me?x=1"), "{}", reply.body);
    assert!(
        reply.body.contains(&format!("host: {origin}")),
        "{}",
        reply.body
    );
    let flow = next_http(&mut events).await;
    assert_eq!(
        flow.url, "http://api.prod.test/v1/me?x=1",
        "la URL que pidió el cliente"
    );
    assert_eq!(flow.rules, ["a local"]);
}

#[tokio::test]
async fn map_remote_loop_is_detected() {
    let rules = Arc::new(Rules::in_memory());
    let proxy = start_proxy(&rules, None).await;
    rules
        .replace(vec![rule(
            "bucle",
            "*",
            Action::MapRemote(MapRemote {
                host: Some("127.0.0.1".into()),
                port: Some(proxy.local_addr().port()),
                ..MapRemote::default()
            }),
        )])
        .unwrap();
    let reply = send(proxy.local_addr(), &get("http://x.test/", "x.test", "")).await;
    assert_eq!(reply.status(), 508);
}

#[tokio::test]
async fn block_and_no_cache() {
    let origin = start_origin().await;
    let rules = rules_with(vec![
        rule("ads", "*://ads.test/*", Action::Block { status: 403 }),
        rule("fresco", "*", Action::NoCache),
    ]);
    let proxy = start_proxy(&rules, None).await;
    let blocked = send(
        proxy.local_addr(),
        &get("http://ads.test/a.js", "ads.test", ""),
    )
    .await;
    assert_eq!(blocked.status(), 403);
    assert!(blocked.body.contains("ads"));

    let fresh = send(
        proxy.local_addr(),
        &get(
            &format!("http://{origin}/x"),
            &origin.to_string(),
            "If-None-Match: \"v1\"\r\n",
        ),
    )
    .await;
    assert_eq!(fresh.status(), 200);
    assert!(
        !fresh.body.to_ascii_lowercase().contains("if-none-match"),
        "{}",
        fresh.body
    );
    assert!(
        fresh.body.contains("cache-control: no-cache"),
        "{}",
        fresh.body
    );
    assert!(fresh.header("etag").is_none(), "{}", fresh.head);
    assert!(fresh.header("cache-control").unwrap().contains("no-store"));
}

#[tokio::test]
async fn rules_apply_live() {
    let origin = start_origin().await;
    let rules = Arc::new(Rules::in_memory());
    let proxy = start_proxy(&rules, None).await;
    let url = format!("http://{origin}/");
    let host = origin.to_string();
    assert_eq!(
        send(proxy.local_addr(), &get(&url, &host, ""))
            .await
            .status(),
        200
    );
    rules
        .replace(vec![rule("ya", "*", Action::Block { status: 451 })])
        .unwrap();
    assert_eq!(
        send(proxy.local_addr(), &get(&url, &host, ""))
            .await
            .status(),
        451
    );
    rules.replace(vec![]).unwrap();
    assert_eq!(
        send(proxy.local_addr(), &get(&url, &host, ""))
            .await
            .status(),
        200
    );
}

fn breakpoint(request: bool, response: bool) -> Rule {
    rule("bp", "*", Action::Breakpoint { request, response })
}

async fn next_pause(ui: &mut broadcast::Receiver<BreakpointEvent>) -> proxyrr_rules::PausedFlow {
    loop {
        if let BreakpointEvent::Paused(flow) = timeout(LIMIT, ui.recv()).await.unwrap().unwrap() {
            return flow;
        }
    }
}

#[tokio::test]
async fn breakpoint_edits_the_request() {
    let origin = start_origin().await;
    let rules = rules_with(vec![breakpoint(true, false)]);
    let mut ui = rules.breakpoints().subscribe();
    let proxy = start_proxy(&rules, None).await;
    let mut events = proxy.subscribe();
    let addr = proxy.local_addr();
    let url = format!("http://{origin}/login");
    let request = post(&url, &origin.to_string(), "user=a");
    let client = tokio::spawn(async move { send(addr, &request).await });

    let paused = next_pause(&mut ui).await;
    assert_eq!(paused.message.body, "user=a");
    assert_eq!(paused.message.url, url);
    let mut edited = paused.message.clone();
    edited.body = Bytes::from_static(b"user=admin");
    edited.url = format!("http://{origin}/otro");
    edited.headers.push(("x-editado".into(), "si".into()));
    assert!(
        rules
            .breakpoints()
            .resolve(paused.key, Verdict::Continue(edited))
    );

    let reply = client.await.unwrap();
    assert_eq!(reply.status(), 200);
    assert!(reply.body.contains("uri=/otro"), "{}", reply.body);
    assert!(reply.body.contains("x-editado: si"), "{}", reply.body);
    assert!(reply.body.contains("content-length: 10"), "{}", reply.body);
    assert!(reply.body.ends_with("body=user=admin"), "{}", reply.body);
    let flow = next_http(&mut events).await;
    assert_eq!(flow.rules, ["bp"]);
    assert!(flow.request_headers.iter().any(|(n, _)| n == "x-editado"));
    assert_eq!(next_bodies(&mut events).await.request.data, "user=admin");
}

#[tokio::test]
async fn breakpoint_edits_the_response() {
    let origin = start_origin().await;
    let rules = rules_with(vec![breakpoint(false, true)]);
    let mut ui = rules.breakpoints().subscribe();
    let proxy = start_proxy(&rules, None).await;
    let mut events = proxy.subscribe();
    let addr = proxy.local_addr();
    let request = get(&format!("http://{origin}/r"), &origin.to_string(), "");
    let client = tokio::spawn(async move { send(addr, &request).await });

    let paused = next_pause(&mut ui).await;
    assert_eq!(paused.message.status, Some(200));
    assert!(String::from_utf8_lossy(&paused.message.body).contains("uri=/r"));
    let mut edited = paused.message.clone();
    edited.status = Some(418);
    edited.body = Bytes::from_static(b"te mock");
    assert!(
        rules
            .breakpoints()
            .resolve(paused.key, Verdict::Continue(edited))
    );

    let reply = client.await.unwrap();
    assert_eq!(reply.status(), 418);
    assert_eq!(reply.body, "te mock");
    assert_eq!(next_http(&mut events).await.status, 418);
    assert_eq!(next_bodies(&mut events).await.response.data, "te mock");
}

#[tokio::test]
async fn breakpoint_abort_returns_503() {
    let origin = start_origin().await;
    let rules = rules_with(vec![breakpoint(true, false)]);
    let mut ui = rules.breakpoints().subscribe();
    let proxy = start_proxy(&rules, None).await;
    let addr = proxy.local_addr();
    let request = get(&format!("http://{origin}/"), &origin.to_string(), "");
    let client = tokio::spawn(async move { send(addr, &request).await });
    let paused = next_pause(&mut ui).await;
    assert!(rules.breakpoints().resolve(paused.key, Verdict::Abort));
    assert_eq!(client.await.unwrap().status(), 503);
}

#[tokio::test]
async fn breakpoint_body_over_the_limit_is_rejected() {
    let origin = start_origin().await;
    let rules = rules_with(vec![breakpoint(true, false)]);
    let _ui = rules.breakpoints().subscribe();
    let proxy = start_proxy(&rules, Some(4)).await;
    let reply = send(
        proxy.local_addr(),
        &post(
            &format!("http://{origin}/"),
            &origin.to_string(),
            "0123456789",
        ),
    )
    .await;
    assert_eq!(reply.status(), 413);
    assert!(rules.breakpoints().pending().is_empty());
}

#[tokio::test]
async fn replay_goes_through_the_proxy() {
    let origin = start_origin().await;
    let rules = rules_with(vec![rule("fresco", "*", Action::NoCache)]);
    let proxy = start_proxy(&rules, None).await;
    let mut events = proxy.subscribe();
    let id = proxy
        .replay(Replay {
            method: "PUT".into(),
            url: format!("http://{origin}/again"),
            headers: vec![
                ("x-replay".into(), "1".into()),
                ("content-length".into(), "1".into()),
            ],
            body: Bytes::from_static(b"cuerpo"),
        })
        .unwrap();
    let flow = next_http(&mut events).await;
    assert_eq!((flow.id, flow.status), (id, 200));
    assert_eq!(flow.method, "PUT");
    assert_eq!(flow.rules, ["fresco"], "las reglas también aplican");
    let bodies = next_bodies(&mut events).await;
    let echoed = String::from_utf8_lossy(&bodies.response.data).into_owned();
    assert!(echoed.contains("x-replay: 1"), "{echoed}");
    assert!(
        echoed.contains(&format!("host: {origin}")),
        "Host desde la URL: {echoed}"
    );
    assert!(echoed.ends_with("body=cuerpo"), "{echoed}");

    assert!(
        proxy
            .replay(Replay {
                method: "GET".into(),
                url: "ftp://x".into(),
                headers: vec![],
                body: Bytes::new(),
            })
            .is_err()
    );
    let looped = proxy.replay(Replay {
        method: "GET".into(),
        url: format!("http://{}/", proxy.local_addr()),
        headers: vec![],
        body: Bytes::new(),
    });
    assert!(looped.unwrap_err().contains("propio proxy"));
}
