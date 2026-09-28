# 0001 — Foundation · Tareas

- [x] T1. `Cargo.toml` del workspace + `rust-toolchain.toml` + perfiles y lints. (CA 1, 2, 6)
- [x] T2. Esqueletos de los 7 crates con `lib.rs`/`main.rs` mínimos y doc de módulo. (CA 1)
- [x] T3. CLI mínimo con `--version` (`clap`). Test: `tests/smoke.rs::prints_version`. (CA 7)
- [x] T4. `.editorconfig`, `.gitattributes`, `.gitignore`, `LICENSE`. (CA 8, 9)
- [x] T5. `deny.toml` con licencias permitidas y advisories. Verificar local con `cargo deny check`. (CA 4)
- [x] T6. `.github/workflows/ci.yml` con matriz de 3 SO + job deny. (CA 3, 4, 5)
- [x] T7. Verificación local en Windows: `fmt --check`, `clippy -D warnings`, `test` verdes.
- [ ] T8. PR `spec/0001-foundation` con CI verde en los 3 SO. Marcar spec como `terminada`.
