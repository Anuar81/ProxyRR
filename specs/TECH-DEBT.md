# Deuda técnica

Registro vivo de atajos y límites conocidos. No se arreglan en la spec donde aparecen para no inflarla:
se anotan acá y los resuelve la spec de cierre `tech-debt-cleanup` (ver roadmap). Si una deuda bloquea
una spec intermedia, se resuelve ahí y se marca como pagada con el número de esa spec.

Cada entrada: id, origen, qué pasa, por qué importa, propuesta. Al pagarla: `Pagada en NNNN`.

| Id | Origen | Estado |
|----|--------|--------|
| TD-001 | 0004 | pagada en 0010 |
| TD-002 | 0004 | pagada en 0007 |
| TD-003 | 0004 | pagada en 0010 |
| TD-004 | 0004 | pagada en 0010 |
| TD-005 | 0001 | abierta (depende de la licencia definitiva) |
| TD-006 | 0004 | pagada en 0010 |
| TD-007 | 0005 | pagada en 0010 |
| TD-008 | 0005 | pagada en 0010 |
| TD-009 | 0006 | abierta (necesita una Mac y un Linux reales) |
| TD-010 | 0006 | pagada en 0010 |
| TD-011 | 0011 | abierta |
| TD-012 | 0011 | abierta |

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
- **Pagada en 0007:** `CapturedBody.size` cuenta los bytes reales y `FlowSummary.response_size` lo usa.
  El CLI sigue mostrando el `Content-Length` porque imprime al llegar los headers; la app y la API usan el real.

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
  logs a la UI. Momento natural: la spec `cli` (la 0008 `control-api` la dejó afuera para no inflarse;
  la API ya tiene el canal de avisos donde se sumarían).


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

## TD-009 — Instalar la CA en macOS y Linux no se ejecutó nunca de verdad

- **Origen:** 0006.
- **Qué pasa:** los planes de `ca install|uninstall` se prueban por SO (unitarios) y el `--dry-run`/`status`
  corre en los 3 SO del CI, pero instalar de verdad solo se puede probar a mano (cambia el almacén de la
  máquina). En Windows el plan es el mismo `certutil -user -addstore Root` ya usado en la 0005. En macOS las
  flags de `security verify-cert` (`-p ssl -L`) y en Linux la ruta NSS no se corrieron contra un equipo real.
- **Propuesta:** probarlo a mano en una Mac y un Ubuntu (o en un job de CI efímero que sí puede tocar su
  propio almacén: los runners se descartan) y ajustar lo que falle.

## TD-010 — `https://proxyrr.cert` sin `--mitm` da 502

- **Origen:** 0006.
- **Qué pasa:** la página se sirve por HTTP plano, y por HTTPS solo con MITM activo (el túnel sin descifrar
  intenta conectar a un host que no existe). Un navegador que fuerza HTTPS muestra error en vez de la página.
- **Propuesta:** con un `CONNECT proxyrr.cert:443` y sin MITM, terminar el TLS igual con una hoja de la CA
  solo para ese host (el navegador avisará que no confía, pero con un mensaje que explica qué hacer).


## Pagos de la 0010

- **TD-001:** `ProxyConfig::connect_timeout`, 30 s por defecto, y `proxyrr start --connect-timeout`.
  Test `connect_timeout_is_configurable`.
- **TD-003:** `SelfGuard` (`proxyrr-core/src/guard.rs`): con `0.0.0.0`/`::` cualquier IP propia es el
  proxy (se decide con un `bind` UDP, sin libc), y después de conectar se compara la dirección ya
  resuelta, así que un hostname que resuelve al proxy también da 508. Tests
  `all_interfaces_listener_detects_its_own_lan_ip` y `connector_rejects_hostnames_that_resolve_to_the_proxy`.
- **TD-004:** `Proxy::shutdown_within(grace)`: deja de aceptar, cierra túneles opacos y keep-alive
  ociosas, espera los requests en curso y corta lo que pase el plazo (5 s por defecto). Tests
  `shutdown_*`.
- **TD-006:** `tracing` en el motor (errores de `accept` agrupados con conteo), `--log-level` en el CLI y
  `Engine::log_layer()`, que manda `warn`/`error` a las UIs como aviso `log` (WebSocket y app).
- **TD-007:** `--upstream-ca <pem|der>` repetible y `--insecure-upstream` (con aviso).
- **TD-008:** `Mitm` cachea el `Arc<ServerConfig>` por host (LRU de 1024). Se hizo sin perfil previo
  porque el cambio es chico y quita un parseo de clave por handshake.
- **TD-010:** con `local_site_ca`, un `CONNECT proxyrr.cert:443` se termina con una hoja de la CA aunque
  el MITM esté apagado; el resto del tráfico sigue en túnel. Tests `local_site_over_https_without_mitm`
  y `local_ca_never_decrypts_other_hosts`.


## TD-011 — Breakpoints solo desde la app

- **Origen:** 0011.
- **Qué pasa:** los breakpoints pausan solo si la app de escritorio está abierta. Con `proxyrr start --api`
  no hay forma de verlos ni resolverlos por la API, así que en el CLI no pausan (se avisa al arrancar).
- **Propuesta:** `GET /api/v1/breakpoints`, `POST /api/v1/breakpoints/{key}` y avisos `paused`/`resolved`
  por el WebSocket. Mientras haya un cliente del WebSocket suscripto, pausan.

## TD-012 — Copiar como cURL solo en sintaxis bash; Compose no edita bodies binarios

- **Origen:** 0011.
- **Qué pasa:** el comando usa comillas simples POSIX: anda en bash/zsh/Git Bash, no en `cmd` ni en
  PowerShell 5. Compose y "Editar y repetir" muestran el body como texto: si el original es binario, se
  avisa y se manda vacío salvo que el usuario lo complete.
- **Propuesta:** variantes "cURL (PowerShell)" y "cURL (cmd)"; en Compose, opción "mantener body original".
