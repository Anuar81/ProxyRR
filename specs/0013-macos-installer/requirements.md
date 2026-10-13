# 0013 — Instalador para macOS

- Estado: borrador
- Fase: distribución (la habilita 0012: macOS validado a mano)
- Depende de: 0009, 0012
- Rama: `spec/0013-macos-installer` (un único PR al final)

## Por qué

Hoy ProxyRR se usa compilándolo (`cargo run`). El primer objetivo es que el usuario pueda pasarle una
versión armada a compañeros de trabajo para que la prueben, **sin pagar** la cuenta de Apple Developer:
si la herramienta sirve, se paga después y se notariza. Windows y Linux quedan para specs propias.

**Decisión: gratis primero.** Sin Developer ID no hay notarización, así que Gatekeeper bloquea la
primera apertura de un `.dmg` bajado de internet. Se distribuye igual de dos formas:

- `.dmg` sin notarizar (firma ad-hoc): una vez por Mac, "Abrir igualmente" en Ajustes del Sistema.
- `install.sh`: baja el tarball con `curl`, que no marca cuarentena, así que no hay aviso.

La firma con Developer ID y la notarización quedan listas en el workflow y se activan solas cuando haya
secretos cargados (criterio 9).

## Historias

- **H1.** Como usuario, quiero pasarle a un compañero un archivo o un link, y que lo instale sin
  compilar nada.
- **H2.** Como compañero que lo prueba, quiero instalar con una línea en la terminal, o con el `.dmg` y
  un paso explicado.
- **H3.** Como dev, quiero la CLI `proxyrr` en el `PATH`.
- **H4.** Como mantenedor, quiero que empujar un tag `vX.Y.Z` arme y publique la release sola.
- **H5.** Como usuario, quiero verificar lo que bajé y desinstalarlo sin dejar la CA instalada.

## Criterios de aceptación

1. **Versión única.** La versión sale del `Cargo.toml` del workspace; si `tauri.conf.json` la repite y
   difiere, falla un test. El tag `vX.Y.Z` DEBE coincidir, o el workflow falla antes de compilar.
   Test: `version_matches_workspace` (proxyrr-desktop).
2. **Build local.** `scripts/build-macos.sh` DEBE armar en una Mac, sin CI ni cuenta de Apple:
   `ProxyRR.app`, `ProxyRR_<versión>_universal.dmg` (Apple Silicon + Intel),
   `proxyrr-<versión>-macos-universal.tar.gz` (app + CLI) y `SHA256SUMS`. Así el usuario puede pasar
   una versión antes de tener la release automática.
3. **Bundle.** El `.app` DEBE tener ícono `.icns`, identificador `com.proxyrr.desktop`,
   `CFBundleShortVersionString` = versión y `LSMinimumSystemVersion` = 13.0, y ser universal.
   Test: `scripts/check-macos-bundle.sh` (`Info.plist`, `lipo -archs`).
4. **Firma ad-hoc válida.** Sin Developer ID, el `.app` y la CLI DEBEN quedar firmados ad-hoc **después**
   de armar el bundle (`codesign --force --deep -s -`), y `codesign --verify --deep --strict` DEBE pasar.
   Evita el error "ProxyRR está dañado y no se puede abrir". Test: el mismo script del criterio 3.
5. **CLI incluida.** El `.app` DEBE traer `Contents/MacOS/proxyrr` (universal). Certificado… → Este
   equipo DEBE ofrecer "Instalar la CLI en la terminal", que crea `/usr/local/bin/proxyrr` con el diálogo
   de administrador del sistema (o muestra el comando si se cancela). La CLI y la app comparten el
   directorio de datos y la CA. Test: `cli_link_plan` (plan sin ejecutar, como `ca install --dry-run`).
6. **`install.sh`.** `curl -fsSL <url>/install.sh | sh` DEBE: bajar el tarball de la última release (o
   `PROXYRR_VERSION`), verificar su SHA-256 contra `SHA256SUMS` y abortar si no coincide, copiar
   `ProxyRR.app` a `/Applications` (o `~/Applications` si no hay permiso), enlazar la CLI en
   `~/.local/bin` o `/usr/local/bin` y decir cuál. No usa `sudo` sin avisar; no instala la CA. Corre con
   el `sh` de macOS (POSIX). Test: `scripts/test-install.sh` contra un servidor local en `127.0.0.1` con
   un tarball de prueba (checksum bueno → instala en un directorio temporal; malo → aborta sin tocar).
7. **Release.** CUANDO se empuja un tag `v*`, `.github/workflows/release.yml` DEBE correr la
   verificación completa en `macos-latest`, armar con el script del criterio 2, correr el del 3/4 y subir
   a una GitHub Release en **borrador**: `.dmg`, tarball, `SHA256SUMS` e `install.sh`. Si no está
   notarizado, el título dice "(sin notarizar)". Actions fijadas por SHA, como en `ci.yml`.
8. **PR no publica.** En cada PR, CI DEBE armar el bundle y correr los criterios 3 y 4, sin subir nada.
9. **Notarización (opcional).** MIENTRAS existan los secretos de Apple en GitHub, el workflow DEBE
   firmar con Developer ID y hardened runtime, notarizar y grapar: `spctl -a -vv -t install` →
   "Notarized Developer ID" y `xcrun stapler validate` pasan. Sin secretos, este paso se saltea con un
   aviso, no falla. Los secretos nunca aparecen en logs ni en el repo.
10. **README.** DEBE explicar: instalar con `install.sh` o con el `.dmg`; si no está notarizado, abrirlo
    con Ajustes del Sistema → Privacidad y seguridad → "Abrir igualmente", o con
    `xattr -dr com.apple.quarantine /Applications/ProxyRR.app`; verificar `SHA256SUMS`; desinstalar
    (`proxyrr ca uninstall` **antes** de borrar la app, borrar el enlace de la CLI y, opcional, el
    directorio de datos con la clave de la CA). Y avisar que una Mac **administrada por MDM** puede
    bloquear apps sin notarizar o la instalación de una CA propia; en ese caso hace falta la versión
    notarizada o permiso de IT.
11. **Validación a mano** (checklist al final, como 0012): en otra Mac o en otra cuenta de usuario sin el
    repo, instalar por `install.sh` y por `.dmg`, abrir, Iniciar descifrando, instalar y desinstalar la
    CA desde la app, instalar la CLI y correr `proxyrr --version`.

## Diseño

- **Herramienta:** `tauri-cli` fijada a la 2.x que corresponde a `tauri =2.12.0`
  (`cargo install tauri-cli --version =<x> --locked`). `cargo tauri icon icons/icon.png` genera el
  `.icns`; se commitea.
- **Universal:** targets `aarch64-apple-darwin` y `x86_64-apple-darwin` (en `rust-toolchain.toml`); la
  CLI se une con `lipo`; la app con `cargo tauri build --target universal-apple-darwin`.
- **CLI dentro del `.app`:** `bundle.macOS.files` la copia a `Contents/MacOS/`. No es sidecar: la app
  no la lanza.
- **Firma ad-hoc:** el linker de Apple Silicon ya firma ad-hoc cada binario, pero copiar la CLI dentro
  del bundle invalida el sello del `.app`; por eso se re-firma el bundle entero al final.
- **Developer ID (criterio 9):** Tauri firma con `APPLE_SIGNING_IDENTITY`, `APPLE_CERTIFICATE`,
  `APPLE_CERTIFICATE_PASSWORD` y notariza con `APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_PATH`
  (API key de App Store Connect). Entitlements (`crates/proxyrr-desktop/macos/entitlements.plist`):
  `com.apple.security.network.client` y `.server`; sin App Sandbox (el proxy escucha en un puerto y toca
  el llavero y `networksetup`).
- **Por qué `install.sh` no tiene aviso:** Gatekeeper solo revisa archivos con el atributo
  `com.apple.quarantine`, que ponen los navegadores, Slack, AirDrop, etc. `curl` no lo pone. La
  garantía pasa a ser el checksum y que el usuario confíe en la URL.
- **Repo:** para que alguien baje sin cuenta de GitHub, la release tiene que estar en un repo público.
  Con el repo privado se pasan los archivos a mano (criterio 2).
- **Riesgos:** si una Mac con MDM bloquea apps sin notarizar no hay solución gratis. La notarización
  rechaza binarios sin `--options runtime --timestamp`.

## Tareas

1. `.icns` y `tauri.conf.json` con `bundle.macOS` (mínimo 13.0, files). Targets en `rust-toolchain.toml`.
2. Versión única + test `version_matches_workspace`.
3. `scripts/build-macos.sh` (criterio 2) y `scripts/check-macos-bundle.sh` (criterios 3 y 4).
4. "Instalar la CLI en la terminal" en la app + `cli_link_plan`.
5. `install.sh` + `scripts/test-install.sh`.
6. Job de bundle en `ci.yml` y `release.yml` con notarización condicional y entitlements.
7. README (criterio 10).
8. Checklist manual (criterio 11) ejecutada y anotada acá.

## Necesita del usuario

- Decidir si el repo pasa a público (para el link de descarga) o se pasan los archivos a mano.
- Más adelante, si se notariza: cuenta Apple Developer (99 USD/año), certificado **Developer ID
  Application** `.p12` y API key de App Store Connect, cargados en GitHub → Settings → Secrets.

## Fuera de alcance

Mac App Store, auto-actualización (updater de Tauri), Homebrew cask (desde 2026-09-01 exige
notarización), instaladores de Windows y Linux.
