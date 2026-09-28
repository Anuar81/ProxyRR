//! Punta a punta: proxy real → `record` → `FlowStore` (spec 0007, CA 9 y resumen con tamaño real).

use std::sync::Arc;
use std::time::Duration;

use proxyrr_core::{Proxy, ProxyConfig};
use proxyrr_store::{FlowStore, StoredFlow, record};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{Instant, sleep};

#[tokio::test]
async fn proxy_flows_end_up_in_the_store() {
    // Origen que responde chunked: sin Content-Length, el tamaño solo se sabe capturando.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf).await;
        let _ = stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
                  4\r\nhola\r\n0\r\n\r\n",
            )
            .await;
    });

    let proxy = Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        ..ProxyConfig::default()
    })
    .await
    .unwrap();
    let store = Arc::new(FlowStore::default());
    let _recorder = record(Arc::clone(&store), proxy.subscribe());

    let mut client = TcpStream::connect(proxy.local_addr()).await.unwrap();
    client
        .write_all(
            format!(
                "GET http://{origin}/x HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut reply = Vec::new();
    client.read_to_end(&mut reply).await.unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let summary = loop {
        if let Some(summary) = store.list().into_iter().find(|s| !s.in_progress) {
            break summary;
        }
        assert!(
            Instant::now() < deadline,
            "el flujo no llegó completo al store"
        );
        sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(summary.url, format!("http://{origin}/x"));
    assert_eq!(summary.status, 200);
    assert_eq!(summary.response_size, Some(4));
    let Some(StoredFlow::Http(flow)) = store.get(summary.id) else {
        panic!("falta el flujo");
    };
    assert_eq!(flow.bodies.unwrap().response.data, "hola");
    assert_eq!(store.dropped_events(), 0);
}
