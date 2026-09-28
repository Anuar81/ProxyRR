# 0008 — Control API · Diseño

## Piezas (crate `proxyrr-api`)

- **`Engine`**: dueño del `FlowStore` y del `Proxy` actual (`Option`). `start_proxy(ProxySettings)`,
  `stop_proxy()`, `status()`, `clear()`. Tiene dos canales propios que sobreviven a los reinicios del proxy:
  - `subscribe()` → `Notice` (`Flow(FlowSummary)`, `Cleared`, `Proxy(ProxyStatus)`), para la API/UI.
  - `subscribe_flows()` → `FlowEvent` crudo, para el CLI.
  Por cada proxy corre un recorder: aplica el evento al store y **después** publica el resumen ya
  fusionado (así `in_progress`/tamaño real llegan bien a la UI). Al apagar, el recorder sigue hasta que
  terminan las conexiones abiertas: lo que capturen se guarda igual (TD-004 sigue abierta).
- **Ids entre arranques:** el engine crea un `FlowIds` (contador compartible, nuevo en `proxyrr-core`) y
  se lo pasa a cada `Proxy`. Así los ids no se repiten aunque un proxy apagado siga cerrando conexiones.
  (Se descartó sumar un offset al reiniciar: chocaba con las conexiones viejas que siguen emitiendo.)
- **`ApiServer`**: axum sobre el `Engine`. `start(ApiConfig, Arc<Engine>)`, `base_url()`, `token()`, `shutdown()`.
  Los WebSocket escuchan un `watch` de apagado para no quedar colgados.

## Seguridad

- Bind loopback obligatorio; token de 32 bytes de `ring::rand` en hex; comparación en tiempo constante.
- Middleware en orden: `Host` permitido → token. El token por query solo en `/events` (aceptable en loopback;
  no queda en logs porque no hay logs de acceso).
- Bodies servidos con `X-Content-Type-Options: nosniff` y `Content-Security-Policy: sandbox`.
- Sin CORS: una página web no puede leer respuestas; tampoco conoce el token.

## Formato

JSON con `serde`. Duraciones en `*_ms`. Headers como `[[nombre, valor], ...]`. Los DTO viven en la API,
así `proxyrr-store` y `proxyrr-core` no dependen de `serde`.

## Dependencias nuevas

`axum` 0.8 (sin features por defecto: `http1`, `json`, `query`, `tokio`, `ws`), `serde`, `serde_json`.
Tests: `tokio-tungstenite` (la misma versión que usa axum) y `futures-util`. Todas MIT/Apache.

## CLI

`proxyrr start` pasa a usar el `Engine` siempre (una sola ruta de código). `--api` agrega el servidor;
`--api-listen` cambia la dirección (por defecto `127.0.0.1:9091`).
