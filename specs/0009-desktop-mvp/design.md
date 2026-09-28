# 0009 — Desktop MVP · Diseño

## Piezas

- **`crates/proxyrr-desktop`** (Tauri 2.12, binario `proxyrr-desktop`):
  - `backend.rs`: toda la lógica, sin tipos de Tauri (`Backend` sobre el `Engine` de la 0008, con la página
    `proxyrr.cert` de la 0006). Se prueba con `cargo test`, sin ventana, incluido un flujo de punta a punta.
  - `main.rs`: comandos `#[tauri::command]` finos + un evento `proxyrr://notice` con los avisos del engine
    (mismo formato que el WebSocket de la API: `flow`, `cleared`, `proxy`, `lagged`).
  - `ui/`: HTML + CSS + JS de módulos, **sin bundler ni npm**. `lib.js` tiene la lógica pura (filtro, lista
    ordenada, formatos, hex) y se prueba con `node --test`; `app.js` es solo DOM.
- **`proxyrr-store::decode_body`**: gzip/deflate (zlib y crudo)/br, encadenados, tope de 64 MiB contra bombas,
  streams cortados como parciales.
- **DTOs públicos** en `proxyrr-api` (los usa la app para no duplicar el formato).
- **`proxyrr_cert::default_data_dir`**: mismo directorio que el CLI (CA 8).

## Decisión: IPC de Tauri, no la API HTTP

El ADR 0001 decía que todas las UIs hablan por la API local. Para la app embebida se usa el IPC de Tauri
sobre el mismo `Engine`: no abre ningún puerto, no hay token que manejar en JS, y no hace falta CORS. La API
HTTP (0008) sigue para CLI, scripts y una futura UI externa. El formato de los datos es el mismo (DTOs).

## Seguridad de la ventana (CA 7)

- Todo dato capturado entra con `textContent`/atributos; el único `innerHTML` es el QR, un SVG que genera
  ProxyRR a partir de una URL propia.
- CSP: `default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'`.
- Imágenes por `data:` URL; `image/svg+xml` se muestra como texto (un SVG puede llevar scripts).
- Los comandos del SO (certutil, security, pkexec) corren con `spawn_blocking`; en Linux la elevación usa
  `pkexec` (diálogo gráfico) en vez de `sudo`.

## Licencias

Tauri trae 5 crates MPL-2.0 (copyleft por archivo): excepción nombrada en `deny.toml` y el ADR 0001.
`proc-macro-error` (unmaintained, solo de compilación, vía GTK3 en Linux) queda ignorado con motivo.

## CI

Linux instala WebKitGTK 4.1 y dependencias; los 3 SO compilan y prueban la app; Linux corre además los tests de UI.
