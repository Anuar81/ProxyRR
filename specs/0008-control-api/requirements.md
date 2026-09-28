# 0008 — Control API

- Estado: en curso
- Fase: F1
- Depende de: 0005, 0007

## Objetivo

Una API local (HTTP + WebSocket) por la que la app de escritorio, el CLI, scripts y tests controlan el
motor: prender y apagar el proxy, listar e inspeccionar flujos, y recibir los flujos nuevos en vivo. Es la
única vía de entrada al motor para las UIs (ADR 0001). Además, un `Engine` que junta proxy + store con un
ciclo de vida propio, para que la app de escritorio lo embeba sin reimplementarlo.

## Historias

**H1.** Como app de escritorio, quiero ver cada flujo apenas pasa, sin consultar en bucle.

**H2.** Como app de escritorio, quiero prender, apagar y reconfigurar el proxy sin reiniciar el proceso,
sin perder los flujos ya capturados.

**H3.** Como usuario, quiero que ninguna página web ni otro usuario de mi red pueda leer mi tráfico
capturado a través de esta API.

**H4.** Como dev, quiero usar la API desde `curl` o un script para automatizar capturas.

## Criterios de aceptación

1. La API DEBE escuchar solo en loopback. SI se le pide una dirección que no es loopback, ENTONCES DEBE
   negarse a arrancar con un error claro.
2. Cada arranque DEBE generar un token aleatorio (256 bits) salvo que se pase uno explícito. Todo request
   sin `Authorization: Bearer <token>` válido DEBE responder `401`. Solo `/api/v1/events` acepta además
   `?token=` (los WebSocket del navegador no pueden mandar headers). La comparación DEBE ser en tiempo constante.
3. SI el header `Host` no es `127.0.0.1:<puerto>`, `localhost:<puerto>` o `[::1]:<puerto>`, ENTONCES DEBE
   responder `403` (protección contra DNS rebinding).
4. `GET /api/v1/status` DEBE devolver la versión, el estado del proxy (prendido, dirección, MITM, bypass),
   la cantidad de flujos, los bytes de bodies y los eventos perdidos.
5. `POST /api/v1/proxy/start` DEBE prender el proxy con la dirección, MITM y bypass pedidos; `409` si ya
   está prendido o el puerto está ocupado; `422` si se pide MITM sin CA disponible.
   `POST /api/v1/proxy/stop` DEBE apagarlo y ser idempotente. Los flujos capturados DEBEN sobrevivir a
   apagar y volver a prender, y los ids NO DEBEN repetirse entre arranques.
6. `GET /api/v1/flows` DEBE devolver los resúmenes en orden de id; con `?after=<id>`, solo los posteriores
   (para resincronizar). `DELETE /api/v1/flows` DEBE vaciar el store.
7. `GET /api/v1/flows/{id}` DEBE devolver el flujo completo (headers en orden y con repetidos, metadatos de
   los bodies) o `404`. `GET /api/v1/flows/{id}/request/body` y `/response/body` DEBEN devolver los bytes
   tal como viajaron, con su `Content-Type`, y con `nosniff` + `CSP sandbox` para que un HTML capturado
   no se ejecute en el origen de la API.
8. `GET /api/v1/events` (WebSocket) DEBE mandar un `hello` con el estado y después un mensaje por cada
   flujo nuevo o actualizado (`flow`), al vaciar (`cleared`) y al cambiar el proxy (`proxy`). SI el cliente
   se atrasa, ENTONCES DEBE recibir `lagged` con la cantidad perdida, para que resincronice con `?after=`.
9. Los errores DEBEN ser JSON `{"error": {"code", "message"}}`.
10. `proxyrr start --api` DEBE levantar la API junto al proxy e imprimir la URL y el token. Con
    `PROXYRR_API_TOKEN` se fija el token (scripts). Sin `--api`, el CLI se comporta como antes.

## Fuera de alcance

Reglas, breakpoints y Compose (F3); logs del motor por la API (TD-006, spec `cli`); guardar/abrir sesión;
CORS para orígenes web (lo define `desktop-mvp` si la UI no usa IPC).
