# 0001 — Foundation

- Estado: borrador
- Fase: F0
- Depende de: —

## Objetivo

Dejar un workspace de Cargo vacío pero correcto, con calidad automatizada en los 3 SO,
para que toda spec siguiente arranque escribiendo lógica y no infraestructura.

## Historias

**H1.** Como desarrollador, quiero clonar y correr `cargo test` en cualquier SO y que funcione sin pasos extra.

**H2.** Como mantenedor, quiero que cada PR se valide automáticamente en Windows, macOS y Linux.

**H3.** Como mantenedor, quiero que estilo, lints, licencias y vulnerabilidades se controlen solos.

## Criterios de aceptación

1. El repositorio DEBE ser un workspace de Cargo con los crates de ADR 0001 creados como esqueletos que compilan.
2. El repositorio DEBE fijar la versión de Rust en `rust-toolchain.toml` (canal stable + `clippy`, `rustfmt`).
3. CUANDO se abre o actualiza un PR, CI DEBE ejecutar `fmt --check`, `clippy -D warnings`, `test` en `windows-latest`, `macos-latest` y `ubuntu-latest`.
4. CUANDO se abre o actualiza un PR, CI DEBE ejecutar `cargo deny check` (licencias, advisories, fuentes).
5. SI algún paso de CI falla, ENTONCES el PR DEBE quedar marcado como fallido.
6. El workspace DEBE declarar en `[workspace.lints]` que `unsafe_code` está prohibido salvo excepción documentada.
7. El workspace DEBE incluir un test de integración de humo que lance el binario `proxyrr --version` y verifique la salida.
8. El repositorio DEBE incluir `.editorconfig`, `.gitattributes` (normalizar finales de línea) y `.gitignore`.
9. La licencia del proyecto DEBE declararse en `LICENSE` y en `Cargo.toml` de cada crate.

## Fuera de alcance

Cualquier lógica de proxy, certificados o UI.

## Decisiones

- **Licencia (tentativa)**: `PolyForm-Small-Business-1.0.0`. En `Cargo.toml` se declara como
  `license-file = "LICENSE"` (no es un identificador SPDX estándar). Ver ADR 0001.
