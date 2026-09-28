# Deuda técnica

Registro vivo de atajos y límites conocidos. No se arreglan en la spec donde aparecen para no inflarla:
se anotan acá y los resuelve la spec de cierre `tech-debt-cleanup` (ver roadmap). Si una deuda bloquea
una spec intermedia, se resuelve ahí y se marca como pagada con el número de esa spec.

Cada entrada: id, origen, qué pasa, por qué importa, propuesta. Al pagarla: `Pagada en NNNN`.

| Id | Origen | Estado |
|----|--------|--------|
| TD-001 | 0004 | abierta |
| TD-002 | 0004 | abierta |
| TD-003 | 0004 | abierta |
| TD-004 | 0004 | abierta |
| TD-005 | 0001 | abierta |
| TD-006 | 0004 | abierta |
| TD-007 | 0005 | abierta |
| TD-008 | 0005 | abierta |

## TD-001 — Timeout de conexión al origen de 10 s

- **Origen:** 0004 (`CONNECT_TIMEOUT` en `crates/proxyrr-core/src/handler.rs`).
- **Qué pasa:** con un sitio lento el proxy devuelve 502/504 a los 10 s, antes de lo que se rendiría un
  navegador directo (~20 s o más). Visto con neverssl.com el 2026-09-28. Además `HttpConnector` reparte
  ese timeout entre todas las IPs del host, así que con IPv6 + IPv4 a cada una le toca menos.
- **Propuesta:** subir a 30 s y hacerlo configurable en `ProxyConfig` (y en `proxyrr start --connect-timeout`).
  Test: origen que acepta tarde (o `TcpListener` sin `accept` con backlog lleno) → no corta antes del valor.

## TD-002 — El tamaño del flujo es solo el `Content-Length` declarado

- **Origen:** 0004.
- **Qué pasa:** respuestas chunked o sin `Content-Length` no muestran tamaño.
- **Propuesta:** contar bytes reales con un body envolvente. Probablemente se paga sola en `flow-store`
  al capturar los bodies.

## TD-003 — Detección de loop sin DNS

- **Origen:** 0004.
- **Qué pasa:** se detecta `localhost`, loopback y la IP exacta de escucha. Con `--listen 0.0.0.0` y un
  request a la IP de LAN de la propia máquina, o a un hostname que resuelve a ella, el proxy se reenvía a sí mismo.
- **Propuesta:** comparar contra las IPs de las interfaces locales y contra la dirección ya resuelta al
  conectar; o marcar los requests salientes con un header propio y rechazar los que vuelvan con él.

## TD-004 — `shutdown` no espera las conexiones en curso

- **Origen:** 0004 (`Proxy::shutdown`).
- **Qué pasa:** deja de aceptar y libera el puerto, pero las conexiones abiertas siguen hasta cerrarse
  solas. Para el CLI da igual (el proceso termina); para la app de escritorio, que apaga y prende el proxy
  sin salir, quedarían túneles vivos.
- **Propuesta:** `JoinSet`/`CancellationToken` por conexión, cierre ordenado con un plazo máximo.

## TD-005 — `LICENSE` sin el texto completo

- **Origen:** 0001.
- **Qué pasa:** el archivo tiene el resumen y el link al texto oficial de PolyForm Small Business 1.0.0,
  no el texto completo (las descargas estaban bloqueadas al crearlo).
- **Propuesta:** pegar el texto oficial cuando la licencia deje de ser tentativa.


## TD-006 — El motor no tiene logging: los errores internos no se ven

- **Origen:** 0004 (review del PR #3).
- **Qué pasa:** errores que no son de un flujo, como un `accept` que falla, se absorben con un reintento
  y no quedan registrados en ningún lado. Si el listener falla de forma persistente, nadie se entera.
- **Propuesta:** `tracing` en `proxyrr-core` (warn en errores de `accept` y de conexión, con conteo para
  no inundar), `tracing-subscriber` en el CLI con `--log-level`, y la API de control reenviando esos
  logs a la UI. Momento natural: la spec `cli` o `control-api`, antes del cierre.


## TD-007 — El CLI no permite confiar en CAs extra del lado del origen

- **Origen:** 0005.
- **Qué pasa:** el motor acepta `upstream_roots` (CA corporativa, servidor de desarrollo con certificado
  propio), pero `proxyrr start` no lo expone: esos orígenes dan 502 con `--mitm`.
- **Propuesta:** `--upstream-ca <archivo.pem>` repetible y, aparte y con advertencia fuerte,
  `--insecure-upstream` para no verificar en desarrollo local.

## TD-008 — `ServerConfig` TLS se arma en cada handshake

- **Origen:** 0005 (`Mitm::server_config`).
- **Qué pasa:** la hoja sale de la caché, pero la config de rustls (parseo de la clave incluido) se
  reconstruye por conexión. Es barato, pero se repite en cada túnel.
- **Propuesta:** cachear `Arc<ServerConfig>` (o `Arc<CertifiedKey>` con un `ResolvesServerCert`) por host,
  con la misma LRU. Medir antes: si no aparece en un perfil, cerrar como no-deuda.
