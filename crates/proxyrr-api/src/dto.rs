//! Formato JSON de la API. Vive acá para que el motor y el store no dependan de `serde`.

use std::time::Duration;

use proxyrr_core::{CapturedBody, Headers};
use proxyrr_store::{FlowSummary, StoredFlow};
use serde::Serialize;

use crate::engine::{EngineStatus, Notice, ProxyStatus};
use crate::log::LogLevel;

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// `GET /api/v1/status`.
#[derive(Debug, Clone, Serialize)]
pub struct StatusDto {
    pub version: &'static str,
    pub proxy: ProxyStatusDto,
    pub flows: usize,
    pub body_bytes: u64,
    pub dropped_events: u64,
}

impl From<EngineStatus> for StatusDto {
    fn from(s: EngineStatus) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            proxy: s.proxy.into(),
            flows: s.flows,
            body_bytes: s.body_bytes,
            dropped_events: s.dropped_events,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProxyStatusDto {
    pub running: bool,
    pub listen: Option<String>,
    pub mitm: bool,
    pub bypass: Vec<String>,
}

impl From<ProxyStatus> for ProxyStatusDto {
    fn from(s: ProxyStatus) -> Self {
        Self {
            running: s.running,
            listen: s.listen.map(|a| a.to_string()),
            mitm: s.mitm,
            bypass: s.bypass,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FlowSummaryDto {
    pub id: u64,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub failed: bool,
    pub elapsed_ms: u64,
    pub response_size: Option<u64>,
    pub in_progress: bool,
    pub tunnel: bool,
    pub intercepted: bool,
    /// Reglas que modificaron el flujo.
    pub rules: Vec<String>,
}

impl From<FlowSummary> for FlowSummaryDto {
    fn from(s: FlowSummary) -> Self {
        Self {
            id: s.id,
            method: s.method,
            url: s.url,
            status: s.status,
            failed: s.failed,
            elapsed_ms: millis(s.elapsed),
            response_size: s.response_size,
            in_progress: s.in_progress,
            tunnel: s.tunnel,
            intercepted: s.intercepted,
            rules: s.rules,
        }
    }
}

/// Metadatos de un body; los bytes se piden aparte.
#[derive(Debug, Clone, Serialize)]
pub struct BodyMetaDto {
    pub size: u64,
    pub captured: usize,
    pub truncated: bool,
    pub complete: bool,
}

impl From<&CapturedBody> for BodyMetaDto {
    fn from(b: &CapturedBody) -> Self {
        Self {
            size: b.size,
            captured: b.data.len(),
            truncated: b.truncated,
            complete: b.complete,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageDto {
    pub headers: Headers,
    /// `None` mientras el flujo sigue en curso.
    pub body: Option<BodyMetaDto>,
}

/// `GET /api/v1/flows/{id}`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FlowDto {
    Http {
        id: u64,
        method: String,
        url: String,
        status: u16,
        error: Option<String>,
        elapsed_ms: u64,
        duration_ms: Option<u64>,
        in_progress: bool,
        rules: Vec<String>,
        request: MessageDto,
        response: MessageDto,
    },
    Tunnel {
        id: u64,
        authority: String,
        status: u16,
        intercepted: bool,
        error: Option<String>,
        elapsed_ms: u64,
    },
}

impl From<StoredFlow> for FlowDto {
    fn from(flow: StoredFlow) -> Self {
        match flow {
            StoredFlow::Http(record) => {
                let head = record.head;
                let bodies = record.bodies;
                Self::Http {
                    id: head.id,
                    method: head.method,
                    url: head.url,
                    status: head.status,
                    error: head.error,
                    elapsed_ms: millis(head.elapsed),
                    duration_ms: bodies.as_ref().map(|b| millis(b.duration)),
                    in_progress: bodies.is_none(),
                    rules: head.rules,
                    request: MessageDto {
                        headers: head.request_headers,
                        body: bodies.as_ref().map(|b| (&b.request).into()),
                    },
                    response: MessageDto {
                        headers: head.response_headers,
                        body: bodies.as_ref().map(|b| (&b.response).into()),
                    },
                }
            }
            StoredFlow::Tunnel(t) => Self::Tunnel {
                id: t.id,
                authority: t.authority,
                status: t.status,
                intercepted: t.intercepted,
                error: t.error,
                elapsed_ms: millis(t.elapsed),
            },
        }
    }
}

/// Mensajes del WebSocket `/api/v1/events` (y de los eventos de la app de escritorio).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    /// Estado inicial al conectar.
    Hello {
        /// Estado.
        status: StatusDto,
    },
    /// Flujo nuevo o actualizado.
    Flow {
        /// Resumen.
        flow: FlowSummaryDto,
    },
    /// Se vació el store.
    Cleared,
    /// El proxy se prendió o apagó.
    Proxy {
        /// Estado del proxy.
        proxy: ProxyStatusDto,
    },
    /// El cliente se atrasó y se perdió `missed` avisos: hay que resincronizar con la lista.
    Lagged {
        /// Avisos perdidos.
        missed: u64,
    },
    /// Advertencia o error del motor.
    Log {
        /// `warn` o `error`.
        level: &'static str,
        /// Texto.
        message: String,
    },
}

impl From<Notice> for WsMessage {
    fn from(notice: Notice) -> Self {
        match notice {
            Notice::Log { level, message } => Self::Log {
                level: match level {
                    LogLevel::Warn => "warn",
                    LogLevel::Error => "error",
                },
                message,
            },
            Notice::Flow(summary) => Self::Flow {
                flow: summary.into(),
            },
            Notice::Cleared => Self::Cleared,
            Notice::Proxy(status) => Self::Proxy {
                proxy: status.into(),
            },
        }
    }
}
