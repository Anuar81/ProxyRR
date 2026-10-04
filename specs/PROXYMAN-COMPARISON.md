# Comparación con Proxyman (referencia de ideas)

Fuente: solo la documentación pública de Proxyman ([docs.proxyman.io](https://docs.proxyman.io)) y su uso
como usuario. **Nada sale de descompilar ni de inspeccionar su binario** (su licencia lo prohíbe y ProxyRR
es open source). Copiamos ideas y flujos de trabajo, nunca código, textos ni assets.

Estado: ✅ tenemos · 🟡 parcial · ⬜ falta. Prioridad: P1 = siguiente fase (F3), P2 = después, P3 = quizás.

## Modificar tráfico (núcleo de F3)

| Función | Qué hace en Proxyman | ProxyRR | Prio |
|---|---|---|---|
| Breakpoint | Pausa request y/o response que matchea una regla. Se puede editar URL, método, headers, query, body y status, o en una pestaña "Raw" con el mensaje entero. Acciones: Execute, Cancel (sigue sin cambios), Abort (503). Se crea con clic derecho sobre un flujo, que completa la regla | ⬜ | P1 |
| Map Local (archivo) | Responde con status, headers y body definidos por vos. Se crea desde un flujo y copia su response actual. Acepta un archivo (JSON, texto, binario, imagen) o un mensaje HTTP crudo | ⬜ | P1 |
| Map Local (directorio) | Sirve una carpeta local para un prefijo de URL | ⬜ | P2 |
| Map Remote | Redirige a otro protocolo, host, puerto, path o query. Los campos vacíos no se cambian. Tiene "incluir subpaths" y "preservar Host". HTTP <-> HTTPS | ⬜ | P1 |
| Templates de mensajes | Respuestas guardadas para reusar en breakpoints | ⬜ | P2 |
| Scripting JS | `onRequest(context, url, request)` y `onResponse(...)`, con el body ya parseado según `Content-Type`, addons y estado compartido. Reemplaza a Map Local, Map Remote y Breakpoint con código | ⬜ | P2 (F5) |
| Block List / Allow List | Bloquear hosts o capturar solo algunos | 🟡 `--bypass` | P1 |
| No Caching | Quita los headers de caché para forzar requests frescos | ⬜ | P1 (barato) |
| Network Conditions | Throttling y latencia por regla | ⬜ | P2 |
| DNS Spoofing | Resolver un host a otra IP | ⬜ | P3 |

## Reenviar y crear requests

| Función | ProxyRR | Prio |
|---|---|---|
| Repeat (reenviar tal cual) | ⬜ | P1 |
| Edit & Repeat / Compose (editar y enviar, o crear un request de cero) | ⬜ | P1 |
| Copy as cURL / código (Code Generator) | ⬜ | P1 (barato) |

## Ver y organizar

| Función | ProxyRR | Prio |
|---|---|---|
| Lista en vivo, inspector, JSON formateado, decodificación gzip/br | ✅ | -- |
| Filtro por texto, status y método | ✅ | -- |
| Regex y wildcard en filtros y reglas | ⬜ (usar el mismo matcher que las reglas) | P1 |
| Filtros múltiples y custom | ⬜ | P2 |
| Resaltar con color y comentarios | ⬜ | P2 |
| Columnas de headers custom | ⬜ | P3 |
| JSONPath / JQ sobre el body | ⬜ | P2 |
| Previewer de multipart, GraphQL por queryName, Protobuf | ⬜ | P2/P3 |
| Diff entre dos flujos | ⬜ | P2 |
| Guardar y abrir sesión, Import/Export | 🟡 HAR | P2 |
| Command Palette, pestañas, split view | ⬜ | P3 |

## Dispositivos y protocolos

| Función | ProxyRR | Prio |
|---|---|---|
| Setup automático de Android (emulador) | ✅ 0010 | -- |
| Guías de iOS, Android y desktop con QR | ✅ 0006 | -- |
| Guías para Flutter, React Native, Node, Python, Docker, etc. | ⬜ (`proxyrr setup flutter`...) | P2 |
| HTTP/2, WebSocket, gRPC | ⬜ | F5 |
| Proxy externo (upstream), SOCKS, reverse proxy | ⬜ | F5 |
| TLS key logging (SSLKEYLOGFILE) | ⬜ | P3 |
| CLI y MCP | 🟡 CLI + API | P2 (un MCP sobre la API es barato) |

## Decisión de diseño para F3

Todo lo de la sección "Modificar tráfico" comparte una pieza: una **regla** = matcher (método + URL con
wildcard o regex, opcionalmente solo request o response) + acción. Se implementa una sola vez en
`proxyrr-rules` y cada función es una acción nueva:

1. `proxyrr-rules`: el matcher y las reglas persistidas en JSON en el data dir, con orden y activar/desactivar.
2. Acciones baratas: Map Local, Map Remote, Block, No Caching.
3. Breakpoint: el motor pausa el flujo en un `oneshot`, la app muestra el editor y responde
   Execute/Cancel/Abort. Con timeout para no colgar la conexión para siempre.
4. Repeat, Compose y Copy as cURL, reusando el cliente upstream del motor.
5. En la app: clic derecho sobre un flujo -> "Map Local", "Map Remote", "Breakpoint" y "Repetir", con la
   regla ya completada a partir de ese flujo. Es lo que hace cómodo a Proxyman.
