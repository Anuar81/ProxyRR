# 0000 — Roadmap

Documento vivo: se actualiza cuando una spec se crea o termina.
Referencia funcional: el set de features de Proxyman (implementación propia, sin código ni assets ajenos).

## Fases

| Fase | Objetivo | Resultado usable |
|------|----------|------------------|
| F0 | Fundaciones: workspace, CI en 3 SO, calidad | `cargo test` verde en Win/Mac/Linux |
| F1 | Núcleo MITM + CLI | Capturar HTTP/HTTPS desde terminal y exportar HAR |
| F2 | App de escritorio (Tauri) | Lista de flujos, inspector, filtros, búsqueda |
| F3 | Herramientas de depuración | Breakpoints, Map Local/Remote, Compose, Repeat, Block/Allow |
| F4 | Dispositivos | Android (emulador + físico), iOS, proxy del sistema automático |
| F5 | Avanzado | Scripting JS, WebSocket, HTTP/2, gRPC/Protobuf, GraphQL, throttling, reverse/SOCKS, diff, MCP |

## Specs

| Nº | Spec | Fase | Estado |
|----|------|------|--------|
| 0001 | [foundation](0001-foundation/requirements.md) | F0 | borrador |
| 0002 | [android-capture](0002-android-capture/requirements.md) | F4 | borrador (requisitos capturados temprano) |

Próximas previstas (se escriben al cerrar la anterior): `ca-certificates`, `http-proxy`, `https-mitm`,
`flow-store`, `control-api`, `cli`, `har-export`.

## Mapa de features → fase

**F1**: proxy HTTP, CONNECT/TLS MITM, CA raíz propia + certificados hoja al vuelo con caché, bypass list,
captura en memoria + SQLite, export HAR / cURL, CLI headless.

**F2**: listado en vivo, inspector (headers, body con resaltado, JSON tree, hex, imagen, multipart),
filtros múltiples (wildcard/regex, protocolo, content-type, headers, body), búsqueda full-text,
pestañas, layout horizontal/vertical, color + comentarios, columnas custom, instalación de CA en el SO,
aviso de donación estilo WinRAR (cada N días, cerrable, nunca bloquea el uso).

**F3**: breakpoints request/response (URL, headers, body, status, abort), Map Local (archivo y directorio),
Map Remote, Compose (con import cURL), Repeat / Edit & Repeat, No Caching, Block/Allow list,
guardar/abrir sesión, reglas con wildcard/regex, code generator.

**F4**: Android emulador automático vía `adb` (ver 0002), Android físico (guía + QR + servidor de CA local),
snippet `network_security_config`, iOS físico (guía + perfil), iOS Simulator (macOS),
proxy del sistema auto on/off (Win/Mac/Linux).

**F5**: scripting JS (on_request / on_response, estado compartido), WebSocket (frames en vivo),
HTTP/2, gRPC/Protobuf, GraphQL (match por operationName), network conditions, reverse proxy,
SOCKS5, external proxy / PAC, DNS spoofing, diff, import Charles, TLS key logging (SSLKEYLOGFILE),
servidor MCP para agentes de IA.
