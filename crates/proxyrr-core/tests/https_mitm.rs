//! Integración del MITM HTTPS (spec 0005): origen HTTPS con una CA de test, proxy con la CA de
//! ProxyRR y un cliente `tokio-rustls` que confía solo en lo que cada test indica.

use std::convert::Infallible;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use proxyrr_cert::CertificateAuthority;
use proxyrr_core::{FlowEvent, MitmConfig, Proxy, ProxyConfig, TunnelFlow};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast::Receiver;
use tokio::time::timeout;
use tokio_rustls::client::TlsStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};

const LIMIT: Duration = Duration::from_secs(15);

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Origen HTTPS que describe lo que recibió. Su certificado lo firma una CA propia del test.
struct Origin {
    addr: SocketAddr,
    ca: CertificateAuthority,
}

async fn start_https_origin(cert_host: &str) -> Origin {
    let ca = CertificateAuthority::generate().unwrap();
    let leaf = ca.issue_leaf(cert_host).unwrap();
    let mut config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(leaf.cert_der.clone())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf.key_der.clone())),
        )
        .unwrap();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let service = service_fn(|req: Request<Incoming>| async move {
                    let mut text = format!("method={}\nuri={}\n", req.method(), req.uri());
                    for (name, value) in req.headers() {
                        let _ = writeln!(text, "{name}: {}", value.to_str().unwrap_or("?"));
                    }
                    let body = req.into_body().collect().await.unwrap().to_bytes();
                    let _ = write!(text, "body={}", String::from_utf8_lossy(&body));
                    Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(text))))
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(tls), service)
                    .await;
            });
        }
    });
    Origin { addr, ca }
}

/// Proxy con MITM. `trust_origin`: si el proxy confía en la CA del origen (upstream).
async fn start_mitm_proxy(origin: &Origin, trust_origin: bool, bypass: &[&str]) -> Proxy {
    let ca = Arc::new(CertificateAuthority::generate().unwrap());
    let mut mitm = MitmConfig::new(ca);
    mitm.bypass = bypass.iter().map(ToString::to_string).collect();
    Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        mitm: Some(mitm),
        upstream_roots: if trust_origin {
            vec![origin.ca.cert_der().to_vec()]
        } else {
            Vec::new()
        },
        ..ProxyConfig::default()
    })
    .await
    .unwrap()
}

/// Proxy con MITM que además devuelve su CA, para que el cliente del test pueda confiar en ella.
async fn start_mitm_proxy_with_ca(origin: &Origin) -> (Proxy, Arc<CertificateAuthority>) {
    let ca = Arc::new(CertificateAuthority::generate().unwrap());
    let proxy = Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        mitm: Some(MitmConfig::new(Arc::clone(&ca))),
        upstream_roots: vec![origin.ca.cert_der().to_vec()],
        ..ProxyConfig::default()
    })
    .await
    .unwrap();
    (proxy, ca)
}

/// Abre un túnel CONNECT y devuelve el socket listo para hablar con el destino.
async fn connect(proxy: SocketAddr, target: &str) -> TcpStream {
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
    let head = String::from_utf8(head).unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    stream
}

/// Handshake TLS del cliente, confiando solo en `trust`. Ofrece h2 y http/1.1 por ALPN.
async fn tls(
    stream: TcpStream,
    name: &str,
    trust: &CertificateAuthority,
) -> std::io::Result<TlsStream<TcpStream>> {
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(trust.cert_der().to_vec()))
        .unwrap();
    let mut config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let name = ServerName::try_from(name.to_owned()).unwrap();
    timeout(
        LIMIT,
        TlsConnector::from(Arc::new(config)).connect(name, stream),
    )
    .await
    .unwrap()
}

/// Cliente HTTP/1.1 sobre un TLS ya establecido; permite varios requests en la misma conexión.
async fn http_client(
    stream: TlsStream<TcpStream>,
) -> hyper::client::conn::http1::SendRequest<Empty<Bytes>> {
    let (sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(conn);
    sender
}

async fn get(
    sender: &mut hyper::client::conn::http1::SendRequest<Empty<Bytes>>,
    host: &str,
    path: &str,
) -> (u16, String) {
    sender.ready().await.unwrap();
    let req = Request::get(path)
        .header("host", host)
        .body(Empty::new())
        .unwrap();
    let response = timeout(LIMIT, sender.send_request(req))
        .await
        .unwrap()
        .unwrap();
    let status = response.status().as_u16();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn next_event(events: &mut Receiver<FlowEvent>) -> FlowEvent {
    timeout(LIMIT, events.recv()).await.unwrap().unwrap()
}

async fn next_tunnel(events: &mut Receiver<FlowEvent>) -> TunnelFlow {
    loop {
        if let FlowEvent::Tunnel(flow) = next_event(events).await {
            return flow;
        }
    }
}

#[tokio::test]
async fn mitm_decrypts_and_forwards() {
    let origin = start_https_origin("localhost").await;
    let (proxy, ca) = start_mitm_proxy_with_ca(&origin).await;
    let mut events = proxy.subscribe();
    let port = origin.addr.port();
    let target = format!("localhost:{port}");

    let stream = connect(proxy.local_addr(), &target).await;
    // El cliente confía SOLO en la CA de ProxyRR: si el handshake pasa, la hoja la firmó ella.
    let stream = tls(stream, "localhost", &ca).await.expect("handshake MITM");
    assert_eq!(
        stream.get_ref().1.alpn_protocol(),
        Some(&b"http/1.1"[..]),
        "el proxy debe negociar solo HTTP/1.1"
    );
    let mut client = http_client(stream).await;
    for _ in 0..2 {
        let (status, body) = get(&mut client, &target, "/hello?x=1").await;
        assert_eq!(status, 200);
        assert!(body.contains("uri=/hello?x=1"), "{body}");
        assert!(body.contains(&format!("host: {target}")), "{body}");
    }

    let tunnel = next_tunnel(&mut events).await;
    assert!(tunnel.intercepted);
    assert_eq!(tunnel.error, None);
    let mut urls = Vec::new();
    while urls.len() < 2 {
        if let FlowEvent::Http(flow) = next_event(&mut events).await {
            assert_eq!(flow.status, 200);
            urls.push(flow.url);
        }
    }
    let expected = format!("https://localhost:{port}/hello?x=1");
    assert_eq!(urls, [expected.clone(), expected]);
}

#[tokio::test]
async fn ip_target_without_sni_gets_ip_leaf() {
    let origin = start_https_origin("127.0.0.1").await;
    let (proxy, ca) = start_mitm_proxy_with_ca(&origin).await;
    let target = origin.addr.to_string();
    let stream = connect(proxy.local_addr(), &target).await;
    // Con ServerName IP el cliente no manda SNI: la hoja sale del host del CONNECT.
    let stream = tls(stream, "127.0.0.1", &ca).await.expect("handshake MITM");
    let mut client = http_client(stream).await;
    let (status, body) = get(&mut client, &target, "/ip").await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn bypassed_host_is_not_decrypted() {
    let origin = start_https_origin("localhost").await;
    let proxy = start_mitm_proxy(&origin, true, &["localhost"]).await;
    let mut events = proxy.subscribe();
    let target = format!("localhost:{}", origin.addr.port());
    let stream = connect(proxy.local_addr(), &target).await;
    // El cliente confía solo en la CA del ORIGEN: pasa únicamente si el proxy no se metió.
    let stream = tls(stream, "localhost", &origin.ca)
        .await
        .expect("TLS directo con el origen");
    let mut client = http_client(stream).await;
    let (status, _) = get(&mut client, &target, "/").await;
    assert_eq!(status, 200);
    let tunnel = next_tunnel(&mut events).await;
    assert!(!tunnel.intercepted);
    assert_eq!(tunnel.status, 200);
}

#[tokio::test]
async fn client_rejecting_ca_reports_tls_error() {
    let origin = start_https_origin("localhost").await;
    let proxy = start_mitm_proxy(&origin, true, &[]).await;
    let mut events = proxy.subscribe();
    let target = format!("localhost:{}", origin.addr.port());
    let stream = connect(proxy.local_addr(), &target).await;
    // Cliente que NO tiene instalada la CA de ProxyRR.
    let result = tls(stream, "localhost", &origin.ca).await;
    assert!(result.is_err(), "el cliente no debía aceptar la hoja");
    let tunnel = next_tunnel(&mut events).await;
    assert!(tunnel.intercepted);
    let error = tunnel.error.expect("el evento debe explicar el fallo");
    assert!(error.contains("no confía en la CA"), "{error}");
}

#[tokio::test]
async fn untrusted_upstream_is_502() {
    let origin = start_https_origin("localhost").await;
    let ca = Arc::new(CertificateAuthority::generate().unwrap());
    // Sin la CA del origen en upstream_roots, el proxy no debe confiar en él.
    let proxy = Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        mitm: Some(MitmConfig::new(Arc::clone(&ca))),
        ..ProxyConfig::default()
    })
    .await
    .unwrap();
    let mut events = proxy.subscribe();
    let target = format!("localhost:{}", origin.addr.port());
    let stream = connect(proxy.local_addr(), &target).await;
    let stream = tls(stream, "localhost", &ca).await.expect("handshake MITM");
    let mut client = http_client(stream).await;
    let (status, body) = get(&mut client, &target, "/").await;
    assert_eq!(status, 502);
    assert!(body.contains("no pudo llegar al origen"), "{body}");
    loop {
        if let FlowEvent::Http(flow) = next_event(&mut events).await {
            assert_eq!(flow.status, 502);
            assert!(flow.error.is_some());
            break;
        }
    }
}

#[tokio::test]
async fn non_tls_over_connect_is_tunneled() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (mut read, mut write) = stream.split();
        let _ = tokio::io::copy(&mut read, &mut write).await;
    });
    let origin = start_https_origin("localhost").await;
    let proxy = start_mitm_proxy(&origin, true, &[]).await;
    let mut events = proxy.subscribe();
    let mut stream = connect(proxy.local_addr(), &echo.to_string()).await;
    stream.write_all(b"ping sin tls").await.unwrap();
    let mut buf = [0u8; 12];
    timeout(LIMIT, stream.read_exact(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        &buf, b"ping sin tls",
        "el primer byte leído no debe perderse"
    );
    let tunnel = next_tunnel(&mut events).await;
    assert!(!tunnel.intercepted);
}

#[tokio::test]
async fn server_first_protocol_is_tunneled_after_wait() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let banner = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(b"220 hola\r\n").await.unwrap();
        let mut sink = [0u8; 16];
        let _ = stream.read(&mut sink).await;
    });
    let origin = start_https_origin("localhost").await;
    let proxy = start_mitm_proxy(&origin, true, &[]).await;
    let mut stream = connect(proxy.local_addr(), &banner.to_string()).await;
    // El cliente no manda nada: el proxy espera ~5 s y tuneliza.
    let mut buf = [0u8; 10];
    timeout(LIMIT, stream.read_exact(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf, b"220 hola\r\n");
}

#[tokio::test]
async fn decrypted_bodies_are_captured() {
    let origin = start_https_origin("localhost").await;
    let (proxy, ca) = start_mitm_proxy_with_ca(&origin).await;
    let mut events = proxy.subscribe();
    let target = format!("localhost:{}", origin.addr.port());
    let stream = connect(proxy.local_addr(), &target).await;
    let stream = tls(stream, "localhost", &ca).await.expect("handshake MITM");
    let mut client = http_client(stream).await;
    let (_, body) = get(&mut client, &target, "/captura").await;
    let bodies = loop {
        if let FlowEvent::HttpBodies(bodies) = next_event(&mut events).await {
            break bodies;
        }
    };
    assert_eq!(bodies.response.data, body.as_bytes());
    assert!(body.contains("uri=/captura"));
    assert!(bodies.response.complete);
}
