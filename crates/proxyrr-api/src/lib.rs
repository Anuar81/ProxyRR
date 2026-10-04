//! API de control local de ProxyRR (spec 0008).
//!
//! - [`Engine`]: proxy + store con ciclo de vida propio (prender, apagar, vaciar) y avisos en vivo.
//!   La app de escritorio lo embebe; el CLI también.
//! - [`ApiServer`]: HTTP + WebSocket solo en loopback, con token por sesión y control de `Host`.
//!   Es la única vía por la que UI, scripts y tests hablan con el motor.
//!
//! Rutas (todas bajo `/api/v1`, con `Authorization: Bearer <token>`):
//! `GET /status`, `POST /proxy/start`, `POST /proxy/stop`, `GET|DELETE /flows` (`?after=<id>`),
//! `GET /flows/{id}`, `GET /flows/{id}/request/body`, `GET /flows/{id}/response/body`,
//! `POST /flows/{id}/replay`, `GET|PUT /rules`, `GET /har`,
//! `GET /events` (WebSocket; acepta `?token=`).

pub mod breakpoint;
mod dto;
mod engine;
mod har;
mod log;
mod server;

pub use dto::{
    BodyMetaDto, FlowDto, FlowSummaryDto, MessageDto, ProxyStatusDto, StatusDto, WsMessage,
};

pub use engine::{
    Engine, EngineError, EngineOptions, EngineStatus, Notice, ProxySettings, ProxyStatus,
};
pub use har::to_har;
pub use log::{LogLayer, LogLevel};
pub use server::{ApiConfig, ApiError, ApiServer, DEFAULT_API_PORT};
