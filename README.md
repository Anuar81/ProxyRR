# ProxyRR

Proxy HTTP(S) de depuración multiplataforma (Windows, macOS, Linux) escrito en Rust.
Intercepta, inspecciona y modifica tráfico de apps web, Android e iOS. Gratis para uso personal y empresas chicas.

> Estado: fase F0 (fundaciones). Nada es usable todavía.

## Cómo se trabaja en este repo

Todo cambio nace de una **spec** en [`specs/`](specs/README.md). Las specs son pequeñas,
numeradas y quedan como histórico: una spec terminada no se reescribe, se supera con otra.

- Roadmap y mapa de features: [`specs/0000-roadmap.md`](specs/0000-roadmap.md)
- Decisiones de arquitectura: [`docs/adr/`](docs/adr/)

## Requisitos de desarrollo

- Rust stable (ver `rust-toolchain.toml` cuando exista)
- Windows: Visual Studio Build Tools con la carga "Desktop development with C++"
- macOS: Xcode Command Line Tools
- Linux: `build-essential`, `pkg-config` (y las dependencias de Tauri para la UI en F2)


## Licencia

[PolyForm Small Business 1.0.0](https://polyformproject.org/licenses/small-business/1.0.0): gratis para personas y empresas chicas (menos de 100 personas y de US$1M de facturación); las más grandes necesitan una licencia comercial aparte. Texto completo en `LICENSE`.
Gratis para personas y organizaciones de menos de 100 personas y menos de US$1M de facturación;
las más grandes necesitan una licencia comercial. Ver [`LICENSE`](LICENSE).
