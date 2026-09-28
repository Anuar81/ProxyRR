# 0006 — Certificate setup · Diseño

## Piezas

- **`proxyrr-core::LocalSite`** (punto de extensión nuevo): sitios que el proxy responde él mismo. El motor
  solo enruta: `http://proxyrr.cert/…` a través del proxy (y `https://` con MITM, porque los requests
  descifrados pasan por el mismo `exchange`), y `/cert…` pedido directo al proxy. Se sirve antes de validar
  el destino (el directo no tiene forma absoluta) y se captura como cualquier flujo. Sin sitio configurado,
  el comportamiento es el de siempre.
- **`proxyrr-devices`**:
  - `CaFiles`: PEM, DER, `.mobileconfig` (UUIDs derivados de la CA, así reinstalar reemplaza), SHA-256,
    SHA-1 (la que usan `certutil` y `security` para identificar un cert), nombre de Android.
  - `CertSite`: página + descargas (`/ca.pem`, `/ca.crt`, `/ca.mobileconfig`). Instrucciones ordenadas según
    el `User-Agent`, las demás plataformas abajo. `nosniff`, `no-store` y CSP sin scripts.
  - `trust`: cada operación es un **plan** de `Step`s (programa + args + si necesita sudo). Se puede mostrar
    (`--dry-run`), probar en cualquier SO y ejecutar con un `Runner` (real o falso en tests).
  - `guide`: una guía por destino con IP de LAN y puerto completados; `lan_ip` (UDP `connect`, no manda
    paquetes) y `qr_text` (QR en bloques Unicode).
- **CLI**: `ca install|uninstall [--dry-run]`, `ca status`, `setup <destino> [--host] [--port] [--install] [--no-qr]`.
  `start` carga siempre la CA y engancha `CertSite` en la plantilla del `Engine`, así la página existe
  también cuando la API prende el proxy.

## Almacenes

| SO | Instalar | Quitar | Verificar |
|----|----------|--------|-----------|
| Windows | `certutil -user -addstore Root ca.pem` (sin admin; Windows pide confirmar) | `certutil -user -delstore Root <sha1>` | `certutil -user -verifystore Root <sha1>` |
| macOS | `security add-trusted-cert -r trustRoot -p ssl -k login.keychain-db ca.pem` | `remove-trusted-cert` + `delete-certificate -Z <sha1>` | `find-certificate -a -Z` + `verify-cert -p ssl` |
| Linux Debian | `sudo install … /usr/local/share/ca-certificates/proxyrr-<id>.crt` + `update-ca-certificates` | `rm` + `update-ca-certificates --fresh` | buscar el PEM en los bundles del sistema |
| Linux p11-kit | `sudo trust anchor --store ca.pem` | `trust anchor --remove` | ídem |
| Linux NSS (Chrome) | `certutil -d sql:~/.pki/nssdb -A -t C,, -n "<CN único>"` | `-D -n "<CN único>"` | `-L -n` |

El CN lleva un id por instalación (`ProxyRR CA (8b3e1f23)`), así que quitar por nombre en NSS no puede
tocar otra CA.

## Decisiones

- No se prueba instalar de verdad en los tests: cambiaría el almacén de la máquina que los corre. Se prueban
  los planes por SO (unitarios), el `--dry-run` y `status` contra el binario en los 3 SO del CI.
- `--install` solo donde no hay un humano en el aparato: simulador de iOS y este mismo equipo. En teléfonos
  físicos lo confirma el usuario por diseño del SO.
