# 0004 — HTTP proxy · Diseño

## Dependencias nuevas (todas MIT)

`tokio` (runtime, TCP, señales), `hyper` 1.x (servidor y cliente HTTP/1.1), `hyper-util` (cliente con
pool de conexiones, adaptadores tokio), `http-body-util` (bodies), `bytes`.

## Módulos de `proxyrr-core`

```
config.rs   ProxyConfig { listen }            (default 127.0.0.1:9090)
event.rs    FlowEvent { Http(HttpFlow) | Tunnel(TunnelFlow) } + contador de ids
headers.rs  strip_hop_by_hop(&mut HeaderMap)  (función pura, testeada aparte)
server.rs   Proxy::start(config) -> RunningProxy; loop de accept + un task por conexión
handler.rs  un request -> respuesta: forward HTTP, CONNECT, errores 400/502/508
```

### API pública

```rust
let proxy = Proxy::start(ProxyConfig::default()).await?;   // bind + spawn
proxy.local_addr();                                        // dirección efectiva
let mut events = proxy.subscribe();                        // broadcast::Receiver<FlowEvent>
proxy.shutdown().await;                                    // deja de aceptar y espera el loop
```

Los eventos van por un `tokio::sync::broadcast` (capacidad 1024). Un suscriptor lento recibe `Lagged`
y sigue; el proxy nunca se frena por un consumidor. Este canal es el punto de extensión de H3: el CLI
hoy, la API de control y el store mañana. El pipeline de hooks que **modifica** tráfico se define con
las reglas (F3); acá solo se observa.

### Forward HTTP

- Servidor: `hyper::server::conn::http1` con `with_upgrades()` (necesario para `CONNECT`).
- Cliente upstream: `hyper_util::client::legacy::Client` con `HttpConnector` (pool keep-alive,
  timeout de conexión 10 s). Envía la URI en forma de origen y conserva el `Host` del cliente.
- Bodies en streaming en ambos sentidos (`Incoming` directo), sin cargarlos en memoria.
- Respuestas propias (400/502/508) con `text/plain` y `Connection: close`.

### CONNECT

1. Validar autoridad `host:puerto` y loop.
2. `TcpStream::connect` al destino **antes** de responder, para poder devolver 502.
3. Responder `200`, y en un task: `hyper::upgrade::on(req)` → `copy_bidirectional`.

### Detección de loop

Destino con host `localhost`/loopback **o** igual a la IP de escucha, y puerto igual al del proxy → 508.
Cubre el caso común (abrir `http://127.0.0.1:9090` a través del propio proxy); no resuelve DNS.

### Nota de implementación

`Proxy` (no `RunningProxy`) es el nombre final del handle. Además del 400/502/508 de la spec, un
`CONNECT` cuyo destino no contesta en 10 s devuelve `504 Gateway Timeout` (distinto de "rechazó la conexión").

## Tests

- Unitarios: `headers.rs` (hop-by-hop, incluidos los nombrados en `Connection`).
- Integración (`proxyrr-core/tests/http_proxy.rs`): origen falso con hyper en `127.0.0.1:0`, cliente TCP
  crudo que habla con el proxy. Cubren CA 1–7.
- CLI (`proxyrr-cli/tests/start.rs`): levanta el binario con `--listen 127.0.0.1:0`, lee la dirección
  de stdout, manda un request y verifica la línea del evento (CA 8–9).
