//! Formato JSON de la API. Vive acá para que el motor y el store no dependan de `serde`.

use std::time::Duration;

use proxyrr_core::{CapturedBody, Headers};
use proxyrr_store::{FlowSummary, StoredFlow};
use serde::Serialize;

use crate::engine::{EngineStatus, ProxyStatus};

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// `GET /api/v1/status`.
#[derive(Debug, Serialize)]
pub(crate) struct StatusDto {
    version: &'static str,
    proxy: ProxyStatusDto,
    flows: usize,
    body_bytes: u64,
    dropped_events: u64,
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

#[derive(Debug, Serialize)]
pub(crate) struct ProxyStatusDto {
    running: bool,
    listen: Option<String>,
    mitm: bool,
    bypass: Vec<String>,
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

#[derive(Debug, Serialize)]
pub(crate) struct FlowSummaryDto {
    id: u64,
    method: String,
    url: String,
    status: u16,
    failed: bool,
    elapsed_ms: u64,
    response_size: Option<u64>,
    in_progress: bool,
    tunnel: bool,
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
        }
    }
}

/// Metadatos de un body; los bytes se piden aparte.
#[derive(Debug, Serialize)]
pub(crate) struct BodyMetaDto {
    size: u64,
    captured: usize,
    truncated: bool,
    complete: bool,
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

#[derive(Debug, Serialize)]
pub(crate) struct MessageDto {
    headers: Headers,
    /// `None` mientras el flujo sigue en curso.
    body: Option<BodyMetaDto>,
}

/// `GET /api/v1/flows/{id}`.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum FlowDto {
    Http {
        id: u64,
        method: String,
        url: String,
        status: u16,
        error: Option<String>,
        elapsed_ms: u64,
        duration_ms: Option<u64>,
        in_progress: bool,
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

/// Mensajes del WebSocket `/api/v1/events`.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum WsMessage {
    Hello { status: StatusDto },
    Flow { flow: FlowSummaryDto },
    Cleared,
    Proxy { proxy: ProxyStatusDto },
    Lagged { missed: u64 },
}
