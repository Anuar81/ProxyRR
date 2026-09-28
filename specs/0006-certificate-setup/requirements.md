# 0006 — Certificate setup

- Estado: terminada (PR #7; CA 9 se completa en 0002, CA 14 en 0009)
- Alcance de esta entrega: CA 1–8 y 10–13 (CLI, página de la CA, guías). CA 9 se completa en 0002
  (automatización del emulador con adb) y CA 14 en `desktop-mvp` (menú Certificado sobre las mismas piezas).
- Fase: F1 (CLI + página de la CA) / F2 (menú en la app) / F4 (automatización por dispositivo)
- Depende de: 0003, 0005, `control-api`
- Relacionada: 0002 (Android: la automatización con `adb` vive allá)

## Objetivo

Que instalar la CA de ProxyRR sea **trivial en cada plataforma**, como en Proxyman: un menú
"Certificado" con una entrada por destino ("Instalar en este equipo", "iOS", "Android emulador",
"Android físico"…), y cada una hace el trabajo sola o da instrucciones paso a paso con el archivo
correcto listo. Requisito explícito del usuario: nada de pasos manuales rebuscados.

## Historias

**H1.** Como usuario, quiero instalar (y desinstalar) la CA en mi Windows, macOS o Linux con un solo
comando o botón, y saber si ya está instalada y es de confianza.

**H2.** Como usuario, quiero elegir "iOS" o "Android" y recibir instrucciones paso a paso, el archivo de
la CA en el formato que ese sistema necesita y un QR para abrirlo desde el teléfono.

**H3.** Como usuario de un dispositivo configurado con el proxy, quiero abrir una URL fácil de recordar
en su navegador y descargar desde ahí la CA, sin copiar archivos a mano.

## Criterios de aceptación

### Este equipo (Windows / macOS / Linux)

1. `proxyrr ca install` DEBE instalar la CA como raíz de confianza **del usuario actual** (sin admin cuando
   el SO lo permite): Windows con el store `CurrentUser\Root`; macOS en el llavero de inicio de sesión con
   confianza SSL; Linux en el store del sistema (`update-ca-certificates` / `trust anchor`, pidiendo
   elevación) y en la base NSS del usuario (Chrome/Firefox) si `certutil` de NSS está disponible.
2. `proxyrr ca uninstall` DEBE quitar exactamente esa CA (por huella, nunca por nombre suelto).
3. `proxyrr ca status` DEBE decir si la CA está instalada y es de confianza en este equipo, y en qué stores.
4. SI Firefox usa su propio store, ENTONCES el sistema DEBE avisarlo y explicar cómo hacerlo confiar
   (`security.enterprise_roots.enabled` o importación manual).
5. Instalar DEBE mostrar antes una advertencia corta: quién tenga la clave de la CA puede leer tu HTTPS;
   desinstalala cuando no la uses.

### Dispositivos

6. `proxyrr setup <destino>` con `ios`, `ios-simulator`, `android-emulator`, `android-device`, `windows`,
   `macos`, `linux` DEBE imprimir la guía paso a paso para ese destino, con la IP de LAN y el puerto reales
   del proxy ya completados.
7. **iOS físico:** la guía DEBE cubrir configurar el proxy en Wi-Fi, descargar el perfil (`.mobileconfig`),
   instalarlo en Ajustes y **activar la confianza total** en Ajustes → General → Información → Ajustes de
   confianza de certificados (el paso que todos olvidan).
8. **iOS Simulator (solo macOS):** DEBE poder instalarse automáticamente con `xcrun simctl keychain booted add-root-cert`.
9. **Android emulador:** automático vía `adb` (0002, CA 2–5). Esta spec solo expone la entrada del menú.
10. **Android físico:** la guía DEBE cubrir proxy en Wi-Fi, descarga de la CA (DER `.crt`), instalación en
    Ajustes → Seguridad → Cifrado y credenciales → Instalar certificado de CA, y el snippet de
    `network_security_config` (0002, CA 6), porque las apps no confían en CAs de usuario desde Android 7.

### Servir la CA

11. MIENTRAS el proxy corre, un request a `http://proxyrr.cert/` **a través del proxy** DEBE responder una
    página propia (no se reenvía a internet) con descargas de la CA en PEM, DER y `.mobileconfig`, y las
    instrucciones según el `User-Agent` (iOS / Android / escritorio).
12. El CLI (y luego la app) DEBE poder mostrar un QR con esa URL y otro con `http://<ip-lan>:<puerto>/cert`
    para dispositivos que todavía no tienen el proxy configurado.
13. Todo lo servido DEBE ser solo el certificado público; la clave de la CA nunca sale del equipo.

### UI (F2)

14. La app de escritorio DEBE tener un menú "Certificado" con las mismas entradas (instalar/desinstalar en
    este equipo, iOS, iOS Simulator, Android emulador, Android físico), cada una abriendo la guía de CA 6–10
    con botones para copiar, exportar y mostrar el QR.

## Fuera de alcance

Evitar certificate pinning (Frida/objection), perfiles MDM, instalar en dispositivos sin interacción del usuario
(iOS y Android físicos la exigen por diseño).
