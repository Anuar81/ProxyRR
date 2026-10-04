# 0002 — Android capture

- Estado: implementada en la 0010 (falta la prueba manual con un emulador "Google APIs")
- Fase: F4
- Depende de: ca-certificates, https-mitm, control-api

## Objetivo

Capturar y descifrar HTTPS de apps Android propias en emulador y en dispositivo físico,
con el menor paso manual posible.

## Modos soportados

**A. Emulador sin Google Play (imágenes "Google APIs" / AOSP) — modo directo, prioritario.**
Estas imágenes permiten `adb root`, así que la CA se instala en el **store del sistema** y todas las apps
confían en ella sin tocar el código de la app.

**B. App propia con `network_security_config`** — para dispositivos físicos o imágenes no rooteables.
La CA se instala como certificado de usuario y la app la acepta vía `res/xml/network_security_config.xml`
referenciado desde el `AndroidManifest.xml`.

**C. Emulador con Google Play** — fuera de alcance de esta spec (requiere Magisk). Spec futura si hace falta.

## Criterios de aceptación

1. El sistema DEBE detectar emuladores y dispositivos conectados vía `adb devices` y listarlos con su nivel de API y si admiten `adb root`.
2. CUANDO el usuario elige "Configurar emulador" sobre un emulador rooteable, el sistema DEBE, en un solo paso:
   configurar el proxy HTTP global del emulador hacia ProxyRR e instalar la CA en el store del sistema.
3. El sistema DEBE nombrar el archivo de la CA como `<subject_hash_old>.0`, calculado en Rust (sin depender de `openssl` instalado).
4. MIENTRAS el emulador tenga API ≥ 34, el sistema DEBE instalar la CA también en el store APEX de Conscrypt
   (`/apex/com.android.conscrypt/cacerts`), montándolo en los procesos existentes, porque desde Android 14 el store `/system` ya no alcanza.
5. CUANDO el usuario elige "Revertir" o cierra ProxyRR con emuladores configurados, el sistema DEBE quitar el proxy del emulador (si no, el emulador queda sin internet).
6. El sistema DEBE ofrecer el snippet de `network_security_config.xml` y la línea de `AndroidManifest.xml` listos para copiar, con la advertencia de limitarlo a builds debug (`<debug-overrides>`).
7. El sistema DEBE servir la CA por HTTP en una URL local fácil (p. ej. `http://proxyrr.cert/`) y por QR, para instalarla desde el navegador del dispositivo físico.
8. Los certificados hoja emitidos DEBEN tener validez ≤ 397 días (WebView de Android rechaza hojas más largas con `ERR_CERT_VALIDITY_TOO_LONG`).
9. SI `adb` no está en el PATH, ENTONCES el sistema DEBE indicarlo y permitir configurar su ruta.
10. Todo lo anterior DEBE funcionar desde Windows, macOS y Linux.

## Notas de investigación

- Referencia pública: guía de Proxyman para Android y la de mitmproxy "system-trusted CA on Android".
- Certificate pinning en la app: fuera de alcance (ninguna config de proxy lo resuelve).
