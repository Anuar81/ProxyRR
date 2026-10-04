//! Integraciones de dispositivos de ProxyRR (spec 0006).
//!
//! - [`CaFiles`]: la CA en PEM, DER y perfil `.mobileconfig`.
//! - [`CertSite`]: la página `http://proxyrr.cert/`, que el proxy sirve él mismo.
//! - [`trust`]: instalar, quitar y verificar la CA en Windows, macOS y Linux.
//! - [`guide`]: guías paso a paso por destino (iOS, Android, escritorio).
//! - [`lan_ip`] y [`qr_text`]: datos para las guías de dispositivos físicos.
//! - [`android`]: emuladores y dispositivos Android con `adb` (spec 0002).

pub mod android;
mod files;
pub mod guide;
mod net;
mod site;
pub mod trust;

pub use files::CaFiles;
pub use net::{lan_ip, qr_svg, qr_text};
pub use site::{CertSite, Platform};
