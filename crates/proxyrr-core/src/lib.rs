//! Motor del proxy de ProxyRR.
//!
//! Hoy (spec 0004): proxy HTTP/1.1 con reenvío de requests en forma absoluta, túneles `CONNECT`
//! sin descifrar y un canal de eventos por flujo. El MITM TLS llega en la spec `https-mitm` y la
//! cadena de hooks que modifica tráfico con las reglas (F3). Ver `docs/adr/0001-stack-y-arquitectura.md`.
//!
//! ```no_run
//! # async fn demo() -> std::io::Result<()> {
//! use proxyrr_core::{Proxy, ProxyConfig};
//!
//! let proxy = Proxy::start(ProxyConfig::default()).await?;
//! let mut events = proxy.subscribe();
//! while let Ok(event) = events.recv().await {
//!     println!("{event:?}");
//! }
//! # Ok(()) }
//! ```

mod config;
mod event;
mod handler;
mod headers;
mod server;

pub use config::{DEFAULT_PORT, ProxyConfig};
pub use event::{FlowEvent, HttpFlow, TunnelFlow};
pub use headers::strip_hop_by_hop;
pub use server::Proxy;
