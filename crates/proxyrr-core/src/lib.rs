//! Motor del proxy de ProxyRR.
//!
//! Proxy HTTP/1.1 (spec 0004) con descifrado HTTPS opcional (spec 0005): reenvío de requests en
//! forma absoluta, túneles `CONNECT` opacos o terminados con certificados de la CA de ProxyRR, y un
//! canal de eventos por flujo. La cadena de hooks que modifica tráfico llega con las reglas (F3).
//! Ver `docs/adr/0001-stack-y-arquitectura.md`.
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

mod capture;
mod config;
mod event;
mod handler;
mod headers;
mod mitm;
mod server;

pub use capture::DEFAULT_MAX_BODY_CAPTURE;
pub use config::{DEFAULT_PORT, MitmConfig, ProxyConfig};
pub use event::{CapturedBody, FlowEvent, Headers, HttpBodies, HttpFlow, TunnelFlow};
pub use headers::strip_hop_by_hop;
pub use server::Proxy;
