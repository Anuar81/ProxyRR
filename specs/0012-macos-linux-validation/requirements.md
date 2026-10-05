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

---

## A. Común (macOS y Linux)

A1 ⬜ Dependencias de compilación instaladas (ver README → Requisitos de desarrollo).
A2 ⬜ Verificación completa en verde:
```sh
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo test --workspace && cargo deny check \
  && node --test crates/proxyrr-desktop/tests-ui/lib.test.mjs
```
A3 ⬜ `cargo run -p proxyrr-cli -- ca info` crea la CA y muestra nombre y huella; `ca path` muestra el
directorio de datos (`~/Library/Application Support/ProxyRR` o `~/.local/share/proxyrr`) y
`ca.key.pem` tiene permisos `600` (`ls -l`, **sin abrir el archivo**).
A4 ⬜ `ca status` dice "no instalada" y `ca install --dry-run` muestra el plan sin ejecutar nada.
A5 ⬜ Proxy HTTP sin descifrar: `proxyrr start` y en otra terminal
`curl -x http://127.0.0.1:9090 http://example.com/ -o /dev/null -w '%{http_code}\n'` → `200`, y el
request aparece en la terminal del proxy.
A6 ⬜ Túnel sin descifrar: `curl -x http://127.0.0.1:9090 https://example.com/ -o /dev/null -w '%{http_code}\n'` → `200`.

## B. macOS

B1 🔐 ⬜ `proxyrr ca install`: macOS pide la contraseña o Touch ID. Después `ca status` → instalada.
En Acceso a Llaveros → inicio de sesión aparece "ProxyRR CA (…)" con "Confiar siempre" para SSL.
B2 ⬜ Descifrado con el almacén del sistema: `proxyrr start --mitm` y
`curl -x http://127.0.0.1:9090 https://example.com/ -o /dev/null -w '%{http_code}\n'` → `200` **sin**
`-k` (el `curl` de macOS usa el llavero). La terminal muestra `https://example.com/`.
B3 ⬜ Safari o Chrome con el proxy del sistema (Ajustes del Sistema → Red → Detalles → Proxies → Web y
Web segura: `127.0.0.1:9090`): un sitio HTTPS carga sin aviso de certificado y se ve descifrado.
**Al terminar apagá los proxies del sistema.**
B4 ⬜ Firefox (si está): sin configurar, avisa de certificado; `ca install` imprimió cómo activar
`security.enterprise_roots.enabled`. Con eso, carga sin aviso.
B5 ⬜ App de escritorio: `cargo run -p proxyrr-desktop` abre la ventana; Iniciar con "Descifrar HTTPS",
navegar, ver la lista en vivo, abrir un flujo (headers, JSON formateado, imagen), filtro
`status:2xx`, interruptor CONNECT, clic derecho → Map Local editado y guardado → recargar muestra el
mock; Breakpoint → editar y Ejecutar; Exportar HAR deja un archivo en Descargas.
B6 ⬜ App: Certificado… → Este equipo muestra el estado correcto (instalada) y los botones funcionan.
B7 ⬜ Simulador de iOS (si hay Xcode): con un simulador abierto, Certificado… → Simulador de iOS →
"Instalar en el simulador abierto" (o `proxyrr setup ios-simulator --install`); Safari del simulador
con el proxy del sistema ve HTTPS descifrado.
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
