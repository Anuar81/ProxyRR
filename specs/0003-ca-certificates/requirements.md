# 0003 — CA certificates

- Estado: en curso
- Fase: F1
- Depende de: 0001

## Objetivo

Que ProxyRR tenga su propia CA raíz persistente y pueda emitir al vuelo certificados hoja válidos
para cualquier host, rápido y reutilizándolos. Es la base del MITM TLS (spec `https-mitm`) y de la
instalación de la CA en dispositivos (0002).

## Historias

**H1.** Como usuario, quiero que la primera vez ProxyRR genere una CA y las siguientes reutilice la misma,
para instalarla una sola vez en mi SO / emulador.

**H2.** Como motor del proxy, necesito un certificado hoja por host en milisegundos y cacheado.

**H3.** Como usuario, quiero exportar la CA (PEM/DER) y ver sus datos (huella, vencimiento, hash para Android).

## Criterios de aceptación

1. CUANDO se pide la CA y no existe en el directorio de datos, el sistema DEBE generar una CA nueva
   (clave ECDSA P-256, `CA:TRUE`, `pathlen:0`, `keyCertSign` + `cRLSign`) y guardarla como `ca.pem` + `ca.key.pem`.
2. CUANDO la CA ya existe, el sistema DEBE cargarla y NO regenerarla (misma huella SHA-256 entre ejecuciones).
3. El nombre de la CA DEBE ser único por instalación (`ProxyRR CA (<id corto aleatorio>)`), para que dos instalaciones no se pisen en un mismo store.
4. En Unix, `ca.key.pem` DEBE crearse con permisos `0600`.
5. SI los archivos de la CA existen pero están corruptos o no coinciden entre sí, ENTONCES el sistema DEBE devolver un error claro y NO sobrescribirlos.
6. CUANDO se emite una hoja para un host, el certificado DEBE: estar firmado por la CA, tener el host en el SAN
   (DNS o IP según corresponda), `extendedKeyUsage = serverAuth`, `CA:FALSE`, y validez ≤ 397 días
   (arranca 1 día antes de ahora para tolerar relojes desfasados).
7. CUANDO se pide una hoja para un host ya emitido, el sistema DEBE devolver la hoja cacheada (caché LRU con capacidad configurable, segura entre hilos).
8. El sistema DEBE exportar la CA en PEM y DER.
9. El sistema DEBE calcular `subject_hash_old` de la CA igual que `openssl x509 -subject_hash_old` (nombre de archivo para Android, 0002 CA 3), sin depender de OpenSSL.
10. El CLI DEBE ofrecer `proxyrr ca info`, `proxyrr ca export [--der] [--out <archivo>]` y `proxyrr ca path`, con `--data-dir` para cambiar el directorio.

## Fuera de alcance

Instalar la CA en el SO o dispositivos (F2/F4), rotación/revocación de la CA, TLS en sí.
