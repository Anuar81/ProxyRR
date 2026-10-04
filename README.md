# ProxyRR

Proxy HTTP(S) de depuración multiplataforma (Windows, macOS, Linux) escrito en Rust, al estilo
Proxyman/Charles: intercepta, inspecciona y modifica el tráfico de navegadores, apps de escritorio,
Android e iOS. Sin límite de dominios.

> Estado: F0–F3 terminadas (proxy, HTTPS, captura, API, app de escritorio, Android, reglas). Probado
> en Windows; macOS y Linux pasan el CI pero falta probarlos a mano (TD-009). Todavía no hay
> instalador: se corre desde el código.

## Arrancar

Requisitos: Rust stable y las herramientas de compilación de tu sistema (ver
[Requisitos de desarrollo](#requisitos-de-desarrollo)).

```powershell
git clone https://github.com/Anuar81/ProxyRR; cd ProxyRR
cargo run -p proxyrr-desktop        # la app de escritorio
cargo run -p proxyrr-cli -- --help  # la terminal (`proxyrr`)
```

La primera compilación tarda unos minutos. Los datos (CA, `rules.json`, ajustes) quedan en el
directorio de datos del usuario: `%APPDATA%\ProxyRR`, `~/Library/Application Support/ProxyRR` o
`~/.local/share/proxyrr`.

## Uso rápido (app de escritorio)

1. **Certificado… → Este equipo → Instalar**: la CA de ProxyRR queda como raíz de confianza para tu
   usuario (en Windows no pide admin).
2. Dejá tildado **Descifrar HTTPS** y tocá **Iniciar** (escucha en `127.0.0.1:9090`).
3. Apuntá el navegador o el sistema al proxy `127.0.0.1:9090`. Para probar sin tocar tu navegador:
   ```powershell
   & "$env:ProgramFiles\Google\Chrome\Application\chrome.exe" --proxy-server="http://127.0.0.1:9090" --user-data-dir="$env:TEMP\proxyrr-chrome"
   ```
4. Los requests aparecen en vivo. Clic en uno para ver headers y body (JSON formateado; gzip, br y
   deflate decodificados; imágenes; vista hex). Filtro: texto, `status:4xx`, `method:post`, `-excluir`.

Los `CONNECT` ya descifrados se ocultan porque sus requests están en la lista como `https://`; el
interruptor **CONNECT** de la barra los muestra. Los que tienen un aviso se ven siempre.

## Android

**Emulador "Google APIs" (sin Play Store), un clic:** Certificado… → **Android con un clic** →
**Configurar**. Pone el proxy y la CA en el almacén del sistema (también en Android 14+), así que
todas las apps confían en ella. Se pierde al reiniciar el emulador; al cerrar la app se quita el proxy.

**Teléfono por USB, un clic:** con la depuración USB activada, el mismo botón usa `adb reverse` (no
depende de la IP ni del firewall) y copia la CA a `Download/proxyrr-ca.crt`. Instalala una vez a
mano: Ajustes → Seguridad → Instalar desde el almacenamiento → **Certificado de CA**.

**Teléfono por Wi-Fi:** Certificado… → Android físico. El proxy tiene que escuchar en la red
(`0.0.0.0:9090`, la guía tiene el botón); el QR abre la página con la CA.

En un teléfono sin root la CA queda como certificado de usuario: Chrome la acepta, y tus apps solo si
lo declaran en su `network_security_config` (solo debug):

```xml
<network-security-config>
    <debug-overrides>
        <trust-anchors>
            <certificates src="user" />
            <certificates src="system" />
        </trust-anchors>
    </debug-overrides>
</network-security-config>
```

Desde la terminal: `proxyrr android devices | setup | revert`, o `proxyrr start --mitm --android`.

### Si un host no se ve

- **"el cliente no confía en la CA"**: falta instalar la CA en el dispositivo, o la app no la acepta
  (`network_security_config`, build que no es debug).
- **Túnel descifrado sin requests, con aviso de "posible certificate pinning"**: la app verifica su
  propio certificado (OkHttp `CertificatePinner`, `<pin-set>`) y corta. Desactivá el pinning en debug
  o excluí el host con **Sin descifrar** / `--bypass`.
- **Flutter** no usa el proxy del sistema; **WebSocket (`wss://`)**, **HTTP/2-gRPC** y **HTTP/3**
  todavía no se descifran (F5).

## iPhone, macOS, Linux

Certificado… tiene una guía por destino (iPhone con perfil `.mobileconfig` y el paso de "confianza
total", simulador de iOS, Windows, macOS, Linux). Lo mismo en la terminal:
`proxyrr setup <ios|ios-simulator|android-emulator|android-device|windows|macos|linux>`. Cualquier
dispositivo que ya use el proxy puede abrir `http://proxyrr.cert` para bajar la CA.

## Modificar el tráfico (reglas)

Clic derecho sobre un flujo crea la regla con sus datos; **Reglas…** las ordena, activa y edita. El
patrón es un wildcard (`*`) o una regex; si no tiene `?`, la query no cuenta. Aplican al próximo
request, sin reiniciar.

| Regla | Qué hace |
| --- | --- |
| **Map Local** | Responde con un status, headers y body tuyos (arranca con la respuesta real). |
| **Map Remote** | Redirige a otro host, puerto, path o protocolo (p. ej. producción → localhost). |
| **Breakpoint** | Pausa el request o la respuesta para editarlos: Ejecutar, Continuar o Abortar (503). Si nadie decide en 5 min, sigue sin cambios. |
| **Block** | Corta el request con 403. |
| **No Caching** | Quita los headers de caché del request y de la respuesta. |

También: **Repetir**, **Editar y repetir**, **Compose…** (request de cero, pasa por el proxy) y
**Copiar como cURL** (bash o PowerShell). **Exportar HAR** guarda todo en Descargas.

Las reglas viven en `rules.json` y el CLI usa el mismo archivo (`--rules` para otro).

## Terminal y API

```text
proxyrr start [--mitm] [--listen 127.0.0.1:9090] [--bypass host] [--rules archivo]
              [--har salida.har] [--api] [--log-level warn] [--connect-timeout 30]
proxyrr ca info | export | path | install | uninstall | status
proxyrr setup <destino>
proxyrr android devices | setup | revert
```

`--api` levanta una API local (solo loopback, con token) en `/api/v1`: `status`, `proxy/start|stop`,
`flows`, `flows/{id}` y sus bodies, `flows/{id}/replay`, `rules`, `har`, `breakpoints` y el WebSocket
`events` (flujos en vivo, avisos y breakpoints; mientras haya un cliente conectado, los breakpoints
pausan y se resuelven con `POST /api/v1/breakpoints/{key}`).

## Seguridad

- Mientras la CA esté instalada, quien tenga su clave (`ca.key.pem` en el directorio de datos) puede
  leer tu HTTPS. Desinstalala cuando no la uses (`proxyrr ca uninstall` o el menú Certificado).
- El proxy escucha en loopback salvo que pidas `0.0.0.0` para un teléfono por Wi-Fi: en ese caso
  cualquiera en tu red puede usarlo mientras esté prendido.
- Lo capturado nunca se ejecuta en la app (todo entra como texto, CSP estricta).

## Cómo se trabaja en este repo

Todo cambio nace de una **spec** en [`specs/`](specs/README.md). Las specs son pequeñas,
numeradas y quedan como histórico: una spec terminada no se reescribe, se supera con otra.

- Roadmap: [`specs/0000-roadmap.md`](specs/0000-roadmap.md)
- Deuda técnica: [`specs/TECH-DEBT.md`](specs/TECH-DEBT.md)
- Comparación con Proxyman: [`specs/PROXYMAN-COMPARISON.md`](specs/PROXYMAN-COMPARISON.md)
- Decisiones de arquitectura: [`docs/adr/`](docs/adr/)

Verificación local: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`, `cargo deny check` y `node --test crates/proxyrr-desktop/tests-ui/lib.test.mjs`.

## Requisitos de desarrollo

- Rust stable (`rust-toolchain.toml`)
- Windows: Visual Studio Build Tools con la carga "Desktop development with C++" (WebView2 ya viene con Windows 10/11)
- macOS: Xcode Command Line Tools
- Linux: `build-essential`, `pkg-config` y las dependencias de Tauri (`libwebkit2gtk-4.1-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev`, `libxdo-dev`)
- Android: Android SDK con `adb` (la app lo busca sola o te deja elegir la ruta)

## Licencia

[PolyForm Small Business 1.0.0](https://polyformproject.org/licenses/small-business/1.0.0): gratis para
personas y organizaciones de menos de 100 personas y menos de US$1M de facturación; las más grandes
necesitan una licencia comercial aparte. Texto completo en [`LICENSE`](LICENSE).
