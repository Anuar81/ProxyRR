# 0013 — Instalador para macOS

- Estado: borrador
- Fase: distribución (la habilita 0012: macOS validado a mano)
- Depende de: 0009, 0012
- Rama: `spec/0013-macos-installer` (un único PR al final)

## Por qué

Hoy ProxyRR se usa compilándolo (`cargo run`). Para que alguien lo baje y lo use hace falta un `.dmg`
que se abra sin que Gatekeeper lo bloquee, con la CLI incluida y publicado de forma reproducible desde
CI. Windows y Linux quedan para specs propias.

## Historias

- **H1.** Como usuario de Mac, quiero bajar un `.dmg`, arrastrar ProxyRR a Aplicaciones y abrirlo sin
  avisos de "desarrollador no identificado".
- **H2.** Como dev, quiero la CLI `proxyrr` en el `PATH` sin compilarla.
- **H3.** Como mantenedor, quiero que empujar un tag `vX.Y.Z` arme, firme, notarice y publique la
  release solo, sin pasos a mano ni secretos en el repo.
- **H4.** Como usuario, quiero poder verificar lo que bajé (checksum) y desinstalarlo sin dejar la CA
  instalada.

## Criterios de aceptación

1. **Versión única.** La versión sale de `Cargo.toml` del workspace; `tauri.conf.json` no la repite (o
   un test falla si difieren). El tag `vX.Y.Z` DEBE coincidir con ella, o el workflow de release falla
   antes de compilar. Test: `version_matches_workspace` (proxyrr-desktop).
2. **Bundle.** `cargo tauri build --target universal-apple-darwin` DEBE producir `ProxyRR.app` y
   `ProxyRR_<versión>_universal.dmg` (Apple Silicon + Intel), con ícono `.icns`, identificador
   `com.proxyrr.desktop`, `CFBundleShortVersionString` = versión y `LSMinimumSystemVersion` = 13.0.
   Test: `scripts/check-macos-bundle.sh` (lee el `Info.plist` y `lipo -archs`).
3. **CLI incluida.** El `.app` DEBE traer `Contents/MacOS/proxyrr` (la CLI, universal). La ventana
   Certificado… → Este equipo DEBE ofrecer "Instalar la CLI en la terminal", que crea el enlace
   `/usr/local/bin/proxyrr` con el diálogo de administrador del sistema (o explica el comando si se
   cancela). La CLI y la app comparten el directorio de datos y la CA. Test: `cli_link_plan` (plan sin
   ejecutar, como `ca install --dry-run`).
4. **Firma y notarización.** MIENTRAS haya secretos de Apple en CI, el `.app`, la CLI y el `.dmg`
   DEBEN quedar firmados con Developer ID y hardened runtime, notarizados y con el ticket grapado:
   `codesign --verify --deep --strict`, `spctl -a -vv -t install` → "accepted · Notarized Developer ID"
   y `xcrun stapler validate` pasan. Entitlements mínimos: red cliente y servidor; sin sandbox.
5. **Sin cuenta de Apple.** SI no hay secretos, ENTONCES el workflow DEBE igual publicar el `.dmg` con
   firma ad-hoc, marcado "sin notarizar" en el nombre de la release, y el README DEBE explicar cómo
   abrirlo (Ajustes del Sistema → Privacidad y seguridad → "Abrir igualmente").
6. **Release.** CUANDO se empuja un tag `v*`, `.github/workflows/release.yml` DEBE compilar en
   `macos-latest`, correr la verificación completa, firmar/notarizar (criterio 4 o 5) y subir a una
   GitHub Release en borrador: el `.dmg`, `proxyrr-<versión>-macos-universal.tar.gz` (CLI sola) y
   `SHA256SUMS`. Los secretos se leen solo de GitHub Secrets; ningún log los imprime. Actions fijadas
   por SHA, como en `ci.yml`.
7. **PR no publica.** En cada PR, CI DEBE armar el bundle sin firmar y correr el criterio 2, sin subir
   nada.
8. **Desinstalar.** El README DEBE decir cómo desinstalar: `proxyrr ca uninstall` (o el botón de la
   app) ANTES de borrar la app, borrar el enlace de la CLI y, opcional, el directorio de datos (con la
   clave de la CA).
9. **Validación a mano** (checklist al final, como 0012): en una cuenta de usuario sin el repo, bajar
   la release, abrir sin avisos, Iniciar descifrando, instalar/desinstalar la CA desde la app, instalar
   la CLI y correr `proxyrr --version`.

## Diseño

- **Herramienta:** `tauri-cli` fijada a la versión 2.x que corresponde a `tauri =2.12.0`
  (`cargo install tauri-cli --version =<x> --locked`). Íconos: `cargo tauri icon icons/icon.png` genera
  el `.icns`; se commitea.
- **CLI dentro del `.app`:** `bundle.macOS.files` copia el binario `proxyrr` ya compilado universal
  (`lipo` de los dos targets) a `Contents/MacOS/`. No es sidecar de Tauri: la app no la lanza.
- **Firma:** Tauri firma con las variables `APPLE_SIGNING_IDENTITY`, `APPLE_CERTIFICATE`,
  `APPLE_CERTIFICATE_PASSWORD` y notariza con `APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_PATH`
  (API key de App Store Connect, no la contraseña del Apple ID). La CLI suelta se firma y notariza
  aparte (`codesign` + `notarytool submit --wait`; un binario suelto no se puede grapar).
- **Entitlements** (`crates/proxyrr-desktop/macos/entitlements.plist`):
  `com.apple.security.network.client` y `.server`. Sin App Sandbox: el proxy escucha en un puerto y
  toca el llavero y `networksetup`.
- **Riesgos:** la notarización puede rechazar si algún binario no tiene hardened runtime o tiene
  timestamps faltantes (`--options runtime --timestamp`). `universal-apple-darwin` necesita los dos
  targets de rustup en el runner.

## Tareas

1. `cargo tauri icon` → `.icns` en `icons/`; `tauri.conf.json` con `bundle.macOS` (mínimo 13.0,
   entitlements, files). Test: criterio 2.
2. Versión única + test `version_matches_workspace`.
3. Script `scripts/build-macos.sh`: CLI universal con `lipo`, `cargo tauri build --target
   universal-apple-darwin`, tarball de la CLI y `SHA256SUMS`. Corre local igual que en CI.
4. `scripts/check-macos-bundle.sh` (criterio 2; con `--signed` agrega los chequeos del 4).
5. "Instalar la CLI en la terminal" en la app + `cli_link_plan`.
6. `release.yml` (tag `v*`, firma condicional, release en borrador) y job de bundle en `ci.yml`.
7. README: instalar, verificar el checksum, abrir sin notarizar, desinstalar.
8. Checklist manual (criterio 9) ejecutada y anotada acá.

## Necesita del usuario (antes de la tarea 6 con firma)

- Cuenta Apple Developer (99 USD/año), certificado **Developer ID Application** exportado como `.p12`
  y una API key de App Store Connect. Se cargan en GitHub → Settings → Secrets; nunca en el repo.
  Sin eso se sigue el criterio 5.

## Fuera de alcance

Mac App Store, auto-actualización (updater de Tauri), Homebrew cask, instaladores de Windows y Linux.
