# ADR 0001 — Stack y arquitectura base

- Estado: propuesta
- Fecha: 2026-09-26

## Contexto

Queremos un proxy de depuración MITM que corra igual en Windows, macOS y Linux,
con UI de escritorio, modo headless para CI, y que escale a muchas features (reglas,
scripting, protocolos) sin reescribir el núcleo.

## Decisión

- **Lenguaje**: Rust stable. Rendimiento, seguridad de memoria, un solo binario por SO.
- **Runtime async**: `tokio`. **HTTP**: `hyper` 1.x. **TLS**: `rustls` (sin OpenSSL, compila igual en los 3 SO).
- **Certificados**: `rcgen` para CA y hojas; caché LRU de hojas por SNI.
- **Persistencia**: SQLite (`rusqlite`, feature `bundled`) para sesiones; bodies grandes en archivos.
- **UI**: Tauri 2 + frontend web. Una sola UI para los 3 SO, binario liviano.
- **Licencia (tentativa)**: PolyForm Small Business 1.0.0 + licencia comercial aparte para empresas grandes.
  Las dependencias deben ser permisivas (MIT/Apache/BSD/ISC…); `cargo deny` rechaza GPL/AGPL/LGPL.
  Excepción (spec 0009): MPL-2.0 solo para 5 crates transitivos de Tauri (`cssparser`, `cssparser-macros`,
  `dtoa-short`, `selectors`, `option-ext`). Es copyleft por archivo: mientras no se modifiquen, no obliga
  a nada sobre el código de ProxyRR. Cada excepción está nombrada en `deny.toml`.
- **Separación**: el motor no conoce la UI. Todo cliente (desktop, CLI, tests, MCP) habla con una
  **API de control local** (HTTP + WebSocket, solo `127.0.0.1`, token por sesión).

### Workspace

```
crates/
  proxyrr-core     motor: listeners, HTTP/1.1, CONNECT, MITM TLS, pipeline de hooks
  proxyrr-cert     CA raíz, emisión de hojas, export PEM/DER, subject_hash_old (Android)
  proxyrr-rules    matching (wildcard/regex) y acciones: breakpoint, map local/remote, block…
  proxyrr-store    modelo de flujos, SQLite, export HAR
  proxyrr-api      API de control local (axum)
  proxyrr-cli      binario `proxyrr`
  proxyrr-devices  integraciones: adb (Android), proxy del sistema, iOS
apps/
  desktop          Tauri (F2)
```

El pipeline de `proxyrr-core` es una cadena de **hooks** (`on_request`, `on_response`, `on_ws_frame`)
detrás de un trait. Reglas, scripting y breakpoints son implementaciones del trait: agregar una
feature no toca el motor.

## Consecuencias

- `rustls` no soporta algunos cifrados legacy; aceptable para depuración moderna. Se revisa si aparece un caso real.
- Tauri en Windows requiere WebView2 (preinstalado en Win 10/11) y MSVC.
- La API local es superficie de ataque: se liga solo a loopback y exige token.
