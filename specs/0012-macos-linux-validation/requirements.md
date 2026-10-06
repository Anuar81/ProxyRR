# 0012 — Validación manual en macOS y Linux

- Estado: aprobada
- Fase: cierre (paga TD-009; habilita el instalador)
- Rama: `spec/0012-macos-linux-validation` (un único PR al final, con los resultados y los arreglos)

## Por qué

El CI compila y testea en los 3 sistemas, pero hay cosas que solo se pueden probar a mano porque tocan
el almacén de confianza de la máquina, la red o una ventana real: instalar/desinstalar la CA (TD-009),
la app de escritorio, el proxy del sistema y Android. Antes de armar instaladores hay que saber que
eso anda en una Mac y en un Linux de verdad.

## Cómo usar esta checklist (agente o persona)

1. `git clone https://github.com/Anuar81/ProxyRR && cd ProxyRR && git switch spec/0012-macos-linux-validation`
   (si la rama todavía no existe en el remoto, creala desde `main`).
2. Hacé los pasos **en orden** de la sección de tu sistema. Después de cada uno, reemplazá `⬜` por
   `✅` o `❌` y pegá debajo la salida relevante (recortada, sin rutas con datos personales).
3. Si algo falla: anotá el ❌, diagnosticá, arreglá en esta rama con su test cuando se pueda, y agregá
   la línea `Arreglo: <commit> <qué>`. Si no se puede arreglar ahora, abrí un `TD-NNN` en
   [`TECH-DEBT.md`](../TECH-DEBT.md) y referencialo.
4. Los pasos marcados **🔐** cambian el almacén de confianza o piden `sudo`: el agente **pide
   confirmación al usuario** antes de correrlos. Al terminar, la CA queda **desinstalada** (paso final).
5. Commit y push en la rama después de cada sección. El PR lo abre el usuario.

Datos de la máquina (completar): sistema y versión, arquitectura (`uname -m`), `rustc --version`,
navegadores instalados.

- macOS: macOS 27.0 (26A428), `arm64`, Xcode instalado.
  `rustc 1.98.1 (48a229cea 2026-09-01)` (de `rust-toolchain.toml`), node v26.10.0, cargo-deny 0.20.2.
  Navegadores: Safari, Microsoft Edge (Chromium). Sin Firefox ni Chrome.

---

## A. Común (macOS y Linux)

A1 ✅ Dependencias de compilación instaladas (ver README → Requisitos de desarrollo).
- macOS: rustup (perfil `minimal`; el toolchain y los componentes `clippy`/`rustfmt` los baja solo
  `rust-toolchain.toml`), `brew install node`, `cargo install cargo-deny --locked`. Xcode ya estaba.

A2 ✅ Verificación completa en verde:
```sh
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo test --workspace && cargo deny check \
  && node --test crates/proxyrr-desktop/tests-ui/lib.test.mjs
```
- macOS: fmt y clippy sin salida; `cargo test`: 225 passed, 0 failed; `cargo deny`:
  `advisories ok, bans ok, licenses ok, sources ok` (solo avisos `warning[duplicate]`, no fallan);
  `node --test`: 11 pass, 0 fail.

A3 ✅ `cargo run -p proxyrr-cli -- ca info` crea la CA y muestra nombre y huella; `ca path` muestra el
directorio de datos (`~/Library/Application Support/ProxyRR` o `~/.local/share/proxyrr`) y
`ca.key.pem` tiene permisos `600` (`ls -l`, **sin abrir el archivo**).
- macOS:
  ```
  Subject:        CN=ProxyRR CA (f950a201), O=ProxyRR
  Válida desde:   2026-10-05
  Válida hasta:   2036-10-03
  SHA-256:        A5:1D:7D:7C:…:ED:94:A6:BF
  Android:        4964abf2.0
  $ ca path → ~/Library/Application Support/ProxyRR
  -rw-------@ 1 … ca.key.pem
  -rw-r--r--@ 1 … ca.pem
  ```

A4 ✅ `ca status` dice "no instalada" y `ca install --dry-run` muestra el plan sin ejecutar nada.
- macOS:
  ```
  [ ] llavero de inicio de sesión
  [ ] confianza SSL
  No está instalada: `proxyrr ca install`.
  ---
  Agregar la CA al llavero de inicio de sesión con confianza SSL
    $ security add-trusted-cert -r trustRoot -p ssl -k ~/Library/Keychains/login.keychain-db "~/Library/Application Support/ProxyRR/ca.pem"
  --dry-run: no se ejecutó nada.
  ```

A5 ✅ Proxy HTTP sin descifrar: `proxyrr start` y en otra terminal
`curl -x http://127.0.0.1:9090 http://example.com/ -o /dev/null -w '%{http_code}\n'` → `200`, y el
request aparece en la terminal del proxy.
- macOS: `200`; en el proxy: `#1     GET     200  http://example.com/  231 ms`.

A6 ✅ Túnel sin descifrar: `curl -x http://127.0.0.1:9090 https://example.com/ -o /dev/null -w '%{http_code}\n'` → `200`.
- macOS: `200`; en el proxy: `#2     CONNECT 200  example.com:443  24 ms  túnel sin descifrar`.

## B. macOS

B1 🔐 ✅ `proxyrr ca install`: macOS pide la contraseña o Touch ID. Después `ca status` → instalada.
En Acceso a Llaveros → inicio de sesión aparece "ProxyRR CA (…)" con "Confiar siempre" para SSL.
- macOS: pidió autenticación y terminó con `Listo: ProxyRR CA (f950a201) es de confianza en este equipo.`
  ```
  $ ca status
    [x] llavero de inicio de sesión
    [x] confianza SSL
  $ security find-certificate -a -c "ProxyRR CA" -Z ~/Library/Keychains/login.keychain-db
    SHA-256 hash: A51D7D7C…ED94A6BF   "labl"="ProxyRR CA (f950a201)"
  $ security dump-trust-settings
    Cert 0: ProxyRR CA (f950a201) — Trust Setting 0: Policy OID: SSL
  ```

B2 ✅ Descifrado con el almacén del sistema: `proxyrr start --mitm` y
`curl -x http://127.0.0.1:9090 https://example.com/ -o /dev/null -w '%{http_code}\n'` → `200` **sin**
`-k` (el `curl` de macOS usa el llavero). La terminal muestra `https://example.com/`.
- macOS: `200`; `curl -v`: `issuer: CN=ProxyRR CA (f950a201); O=ProxyRR` y `SSL certificate verify ok.`
  En el proxy: `#2     GET     200  https://example.com/  231 ms`.

B3 ✅ Safari o Chrome con el proxy del sistema (Ajustes del Sistema → Red → Detalles → Proxies → Web y
Web segura: `127.0.0.1:9090`): un sitio HTTPS carga sin aviso de certificado y se ve descifrado.
**Al terminar apagá los proxies del sistema.**
- macOS: proxies Web y Web segura de Wi-Fi puestos con `networksetup`. Safari (`https://www.wikipedia.org/`)
  y Microsoft Edge, como caso Chromium (`https://httpbin.org/get`), cargaron sin aviso y se ven descifrados:
  ```
  #30    GET     200  https://www.wikipedia.org/  330 ms  22.2 KB
  #51    GET     200  https://www.wikipedia.org/portal/wikipedia.org/assets/img/Wikipedia-logo-v2@2x.png  139 ms  36.6 KB
  #158   GET     200  https://httpbin.org/get  704 ms  999 B
  ```
  Observación (no es falla): los servicios del sistema con pinning (`gateway.icloud.com`, `wps.apple.com`,
  `configuration.apple.com`, `p192-quota.icloud.com`) dieron 53 `falló el handshake TLS con el cliente`
  en ~1 min. Con `--bypass '*.icloud.com' --bypass '*.apple.com'` pasan por túnel y quedan 0 fallos.
  Proxies del sistema apagados al terminar (`Enabled: No` en ambos).
B4 ➖ Firefox (si está): sin configurar, avisa de certificado; `ca install` imprimió cómo activar
`security.enterprise_roots.enabled`. Con eso, carga sin aviso.
- macOS: no aplica, Firefox no está instalado (decisión del usuario). Queda en
  [TD-013](../TECH-DEBT.md#td-013--firefox-no-se-probó-nunca-de-verdad) con el procedimiento.
B5 ⬜ App de escritorio: `cargo run -p proxyrr-desktop` abre la ventana; Iniciar con "Descifrar HTTPS",
navegar, ver la lista en vivo, abrir un flujo (headers, JSON formateado, imagen), filtro
`status:2xx`, interruptor CONNECT, clic derecho → Map Local editado y guardado → recargar muestra el
mock; Breakpoint → editar y Ejecutar; Exportar HAR deja un archivo en Descargas.
B6 ⬜ App: Certificado… → Este equipo muestra el estado correcto (instalada) y los botones funcionan.
B7 ✅ Simulador de iOS (si hay Xcode): con un simulador abierto, Certificado… → Simulador de iOS →
"Instalar en el simulador abierto" (o `proxyrr setup ios-simulator --install`); Safari del simulador
con el proxy del sistema ve HTTPS descifrado.
- macOS: Xcode no traía runtimes; se bajó iOS 27.0 (24A434). iPhone 18 Pro booteado con `simctl`.
  Probado por CLI (el botón de la app queda para B5/B6). `setup ios-simulator --install` corrió
  `xcrun simctl keychain booted add-root-cert …/ca.pem` → "Listo". Con el proxy del sistema en
  `127.0.0.1:9090` y `start --mitm --bypass '*.apple.com' --bypass '*.icloud.com'`, Safari del
  simulador abrió las páginas sin aviso de certificado (`simctl openurl`) y el proxy las vio descifradas:
  ```
  #42    GET     200  https://example.com/  221 ms
  #58    GET     200  https://httpbin.org/json  958 ms  429 B
  ```
  Proxies del sistema apagados al terminar (`Enabled: No` en ambos).
B8 ⬜ Android (si hay SDK y emulador "Google APIs"): Certificado… → Android con un clic → Configurar;
una app del emulador se ve descifrada; al cerrar la app el emulador sigue con internet.
B9 🔐 ⬜ `proxyrr ca uninstall` → `ca status` dice "no instalada" y desapareció del llavero. No borró
ninguna otra CA.

## C. Linux (Ubuntu/Debian; anotar si es otra distro)

C1 🔐 ⬜ `proxyrr ca install`: pide `sudo` para el store del sistema (copia a
`/usr/local/share/ca-certificates/proxyrr-<id>.crt` y corre `update-ca-certificates`). Si existe
`~/.pki/nssdb` y está `certutil` (`libnss3-tools`), la agrega también a la base NSS de Chrome.
`ca status` muestra cada almacén.
C2 ⬜ `proxyrr start --mitm` y
`curl -x http://127.0.0.1:9090 https://example.com/ -o /dev/null -w '%{http_code}\n'` → `200` sin `-k`.
C3 ⬜ Chrome/Chromium: `google-chrome --proxy-server=http://127.0.0.1:9090 --user-data-dir=$(mktemp -d)`
carga HTTPS sin aviso. Si NSS no estaba, anotarlo y ver si el aviso es claro.
C4 ⬜ Firefox (si está): mismo comportamiento que B4.
C5 ⬜ App de escritorio: como B5 (WebKitGTK). Anotar si algo se ve distinto (fuentes, diálogos,
portapapeles al "Copiar", menú contextual, `Esc`).
C6 ⬜ App: Certificado… → Este equipo → Instalar desde la ventana usa `pkexec` (pide la contraseña
gráficamente) y el estado se actualiza.
C7 ⬜ Android (si hay SDK): como B8.
C8 🔐 ⬜ `proxyrr ca uninstall` quita la CA del sistema y de NSS; `ca status` → "no instalada";
`ls /usr/local/share/ca-certificates/` no deja restos.

## Resultado

- [ ] macOS validado (versión: …)
- [ ] Linux validado (distro: …)
- [ ] TD-009 marcada como pagada en `TECH-DEBT.md`
- [ ] Lo que falló quedó arreglado en esta rama o registrado como `TD-NNN`
- [ ] La CA quedó desinstalada en las máquinas de prueba
