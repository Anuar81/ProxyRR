## Instalar en macOS (13 o más nuevo)

**Con una línea** (elige sola la versión de tu Mac y no muestra avisos de Gatekeeper):

```sh
curl -fsSL https://github.com/Anuar81/ProxyRR/releases/latest/download/install.sh | sh
```

Deja `ProxyRR.app` en Aplicaciones, enlaza la CLI `proxyrr` y verifica el SHA-256 antes de instalar.

**Con el `.dmg`:** bajá el de tu Mac. Para saber cuál es, menú  → Acerca de esta Mac:

- "Chip Apple M…" → `ProxyRR_<versión>_apple-silicon.dmg`
- "Procesador Intel…" → `ProxyRR_<versión>_intel.dmg`

Abrilo y arrastrá ProxyRR a Aplicaciones. Si la release dice "(sin notarizar)", la primera vez macOS
la bloquea: Ajustes del Sistema → Privacidad y seguridad → **Abrir igualmente**, o en la terminal
`xattr -dr com.apple.quarantine /Applications/ProxyRR.app`.

Después: en la app, **Certificado… → Este equipo → Instalar** para ver HTTPS descifrado. Antes de
borrar la app, desinstalá la CA (mismo botón o `proxyrr ca uninstall`).

Verificar lo bajado: `shasum -a 256 ProxyRR_*.dmg` tiene que dar lo mismo que su línea en `SHA256SUMS`.
