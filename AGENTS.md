# ProxyRR — instrucciones para agentes

Este archivo lo leen Kiro (kiro-cli / Kiro Crew) y otros agentes de código al abrir el repo. Rige en
cualquier máquina (Windows, macOS, Linux). Lo que diga el usuario en la conversación tiene prioridad.

## Qué es

Proxy HTTP(S) de depuración al estilo Proxyman, en Rust. Workspace `crates/`: `proxyrr-core` (motor),
`-cert` (CA), `-store` (flujos), `-rules` (reglas y breakpoints), `-api` (API local + modelo compartido),
`-devices` (guías, instalar la CA, Android), `-cli` (`proxyrr`), `-desktop` (app Tauri, UI en JS sin
bundler en `ui/`). Uso: [`README.md`](README.md).

## Cómo se trabaja (obligatorio)

- Todo cambio sale de una spec en `specs/` (reglas en [`specs/README.md`](specs/README.md)); una spec
  terminada no se reescribe. Deuda conocida → [`specs/TECH-DEBT.md`](specs/TECH-DEBT.md) con id `TD-NNN`.
- **Una fase entera en una sola rama `spec/NNNN-nombre` y un único PR al final.** El usuario no quiere
  PRs chicos. En la rama de trabajo, commiteá y pusheá sin pedir permiso (`git push origin <rama>`).
  Nunca push a `main`. El PR lo abre y mergea el usuario.
- Idioma: español rioplatense (vos), en código, commits, specs y respuestas.
- Antes de commitear, todo verde:
  ```sh
  cargo fmt --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  cargo deny check
  node --test crates/proxyrr-desktop/tests-ui/lib.test.mjs
  ```
- Vectores de compresión, hashes o cripto en tests: generarlos con una herramienta de referencia
  (node, openssl), nunca de memoria.
- No edites el mismo archivo con varias herramientas en paralelo: se pisan.

## Seguridad

- Instalar la CA de ProxyRR cambia el almacén de confianza de la máquina: **pedí confirmación antes**
  de `proxyrr ca install` (o cualquier `security`, `update-ca-certificates`, `certutil`) y desinstalala
  al terminar si el usuario no dice lo contrario. Nunca leas, muestres ni commitees `ca.key.pem`.
- Servidores y proxies de prueba siempre en `127.0.0.1`, salvo pedido explícito del usuario.
- `sudo` solo con aviso previo y para lo que la checklist pide.

## Si estás en una Mac o en Linux

El objetivo pendiente es validar a mano lo que el CI no puede: seguí
[`specs/0012-macos-linux-validation/requirements.md`](specs/0012-macos-linux-validation/requirements.md)
paso a paso, anotá cada resultado ahí mismo (✅/❌ + salida relevante) y arreglá lo que falle en la rama
`spec/0012-macos-linux-validation`.
