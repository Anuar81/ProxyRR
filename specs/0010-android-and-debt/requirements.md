# 0010 — Android + deuda técnica + HAR (rama de pruebas integrales)

- Estado: en curso
- Fase: F1/F4
- Depende de: 0002 (requisitos), 0006, 0008, 0009
- Rama: `spec/0010-android-and-debt`

## Objetivo

Juntar en una sola rama todo lo necesario para una prueba de punta a punta: capturar el HTTPS de apps
Android desde la app de escritorio o el CLI. Excepción explícita a la regla 6 (una spec por rama), pedida
por el usuario para probar todo junto.

## Alcance

1. **0002 Android** (implementa sus criterios 1–6, 9 y 10; 7 y 8 ya estaban en 0006 y 0003):
   `proxyrr-devices::android`, `proxyrr android devices|setup|revert`, `proxyrr start --android`, y el
   panel "Android con un clic" en la app (revierte al cerrar).
2. **Deuda técnica:** TD-001, 003, 004, 006, 007, 008 y 010 (detalle en `TECH-DEBT.md`). Quedan abiertas
   TD-005 (licencia) y TD-009 (Mac/Linux reales), que dependen del usuario.
3. **Export HAR 1.2:** `GET /api/v1/har`, `proxyrr start --har <archivo>` y botón "Exportar HAR".
4. Marcar la 0009 como terminada.

## Criterios de aceptación

1. CUANDO hay un emulador rooteable (imagen "Google APIs"), "Configurar" DEBE dejar el proxy en
   `10.0.2.2:<puerto>` y la CA en `/system/etc/security/cacerts` y, con API ≥ 34, en
   `/apex/com.android.conscrypt/cacerts` de zygote y de las apps ya abiertas. Test: `android::tests::rootable_emulator_*`.
2. SI `adb root` no está permitido (imagen con Play Store, físico), ENTONCES el sistema DEBE configurar el
   proxy (con `adb reverse` en físicos), copiar la CA a `Download/` y explicar los pasos manuales. Tests:
   `production_build_falls_back_to_user_ca`, `physical_device_uses_adb_reverse_and_user_ca`.
3. SI el script de instalación no confirma la CA, ENTONCES el sistema NO DEBE poner el proxy. Test:
   `failed_system_install_is_an_error`.
4. CUANDO la app se cierra o el CLI recibe Ctrl+C, el sistema DEBE quitar el proxy de los dispositivos que
   configuró. Tests: `remembers_configured_devices_and_reverts_them`, `revert_clears_proxy_and_reverse`.
5. SI no se encuentra `adb`, ENTONCES el sistema DEBE decir dónde buscó y aceptar una ruta (`--adb`,
   `PROXYRR_ADB`, campo en la app, que se guarda). Tests: `explicit_adb_path_must_exist`, `adb_path_is_validated_and_saved`.
6. El HAR DEBE ser 1.2 válido, con bodies decodificados (base64 si son binarios) y sin túneles. Tests:
   `har::tests::*`, `har_export_contains_captured_flows`.
7. Los criterios de cada deuda pagada están en `TECH-DEBT.md` con su test.

## Prueba manual pendiente (usuario)

- Emulador Pixel 9 con imagen **Google APIs** (API 35): app → Iniciar con "Descifrar HTTPS" →
  Certificado… → Android con un clic → Configurar → abrir apps en el emulador y ver su tráfico.
- Cerrar la app y verificar que el emulador vuelve a tener internet.
