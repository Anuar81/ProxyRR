# 0005 — HTTPS MITM · Diseño

## Dependencias nuevas

`rustls` 0.23 (provider `ring`, sin `aws-lc-rs`, igual que 0003), `tokio-rustls` (servidor TLS con
`LazyConfigAcceptor` para leer el SNI antes de elegir certificado), `hyper-rustls` (conector HTTPS del
cliente upstream, con pool), `rustls-native-certs` (raíces del SO: Windows/macOS/Linux), `rustls-pki-types`.
Todas MIT/Apache/ISC. `proxyrr-core` pasa a depender de `proxyrr-cert`.

## Configuración

```rust
ProxyConfig {
    listen,
    mitm: Option<MitmConfig>,        // None = comportamiento 0004
    upstream_roots: Vec<Vec<u8>>,    // DER extra de confianza para orígenes (CA corporativa, tests)
}
MitmConfig { ca: Arc<CertificateAuthority>, bypass: Vec<String> }
```

## Flujo de un CONNECT con MITM

```
CONNECT host:puerto ──► validar destino + loop (0004)
        │ bypass? ──sí──► túnel 0004 (conecta antes de responder, 502 si falla)
        ▼ no
  200 ──► upgrade ──► leer 1er byte (máx 5 s)
        │ ≠ 0x16 o timeout ──► conectar destino + túnel con los bytes ya leídos (Prefixed)
        ▼ 0x16 (handshake TLS)
  LazyConfigAcceptor ──► SNI o host del CONNECT ──► LeafCache ──► ServerConfig (ALPN http/1.1)
        │ falla ──► evento Tunnel con error (7: "el cliente no confía en la CA…")
        ▼ ok
  hyper http1 sobre el TLS ──► por request: URI = https://host[:puerto]/ruta ──► cliente upstream
```

- El destino real siempre es el host del `CONNECT`, nunca el SNI ni el `Host`.
- `Prefixed<S>`: `AsyncRead` que primero devuelve los bytes ya consumidos y después delega; `AsyncWrite` delega.
- El cliente upstream es uno solo para HTTP y HTTPS (`HttpsConnector::https_or_http`), con pool y ALPN `http/1.1`.
- Un evento `Tunnel` por cada CONNECT, con el campo nuevo `intercepted`. El CLI no imprime los túneles
  descifrados sin error: los requests de adentro ya aparecen como `https://…`.
- Detección de rechazo de la CA: el cliente manda un alert (`UnknownCA`, `BadCertificate`, `CertificateUnknown`)
  → `rustls::Error::AlertReceived`.

## Bypass

`HostPattern`: `example.com` coincide solo con ese host; `*.example.com` con cualquier subdominio (no con
`example.com`). Comparación sin mayúsculas y sin punto final.

## Tests

- Unitarios: `HostPattern`, `Prefixed`.
- Integración (`proxyrr-core/tests/https_mitm.rs`): origen HTTPS con un certificado de una CA de test
  (pasada como `upstream_roots`), cliente `tokio-rustls` que confía solo en la CA de ProxyRR.
- CLI: `start --mitm` crea/usa la CA del `--data-dir` e imprime su huella.
