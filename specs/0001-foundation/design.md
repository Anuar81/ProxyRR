# 0001 — Foundation · Diseño

## Layout

```
Cargo.toml                 [workspace] resolver = "3", members = ["crates/*"]
rust-toolchain.toml
deny.toml
.editorconfig  .gitattributes  .gitignore  LICENSE*
.github/workflows/ci.yml
crates/
  proxyrr-core/  proxyrr-cert/  proxyrr-rules/  proxyrr-store/
  proxyrr-api/   proxyrr-devices/
  proxyrr-cli/   (bin "proxyrr")
    tests/smoke.rs
```

## Workspace

- `[workspace.package]`: `edition = "2024"`, `license`, `repository`, `rust-version` (MSRV = la versión fijada).
- `[workspace.dependencies]`: todas las versiones se declaran UNA vez acá, fijadas; los crates usan `dep.workspace = true`.
- `[workspace.lints.rust]`: `unsafe_code = "forbid"`, `missing_debug_implementations = "warn"`.
- `[workspace.lints.clippy]`: `all = "warn"`, `pedantic = "warn"` (con `module_name_repetitions` permitido).
- Perfil `release`: `lto = "thin"`, `codegen-units = 1`, `strip = true`.

## CI (`ci.yml`)

- Matriz `os: [ubuntu-latest, windows-latest, macos-latest]`.
- Pasos: checkout → toolchain vía `rustup` (lee `rust-toolchain.toml`; ya viene en los runners, así que
  no hace falta una action extra) → `Swatinem/rust-cache`
  → `cargo fmt --all --check` (solo Linux) → `cargo clippy --workspace --all-targets -- -D warnings`
  → `cargo test --workspace`.
- Job aparte `deny` (Linux): `EmbarkStudios/cargo-deny-action`.
- Actions fijadas por SHA de commit, no por tag.

## Smoke test

`crates/proxyrr-cli/tests/smoke.rs` usa `env!("CARGO_BIN_EXE_proxyrr")` para ejecutar el binario
real y comprobar que `--version` imprime `proxyrr <CARGO_PKG_VERSION>`. Sin dependencias extra.

## Riesgos

- Rutas y finales de línea en Windows: `.gitattributes` con `* text=auto eol=lf`.
- Tiempo de CI en macOS: la caché de `rust-cache` lo mitiga.
