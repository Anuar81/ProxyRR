//! Eventos que publica el motor por cada flujo.
//!
//! Es el punto de extensión de solo lectura: CLI, API de control y store se suscriben sin tocar
//! el motor.

use std::time::Duration;

/// Un flujo observado por el proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowEvent {
    /// Request HTTP reenviado (o rechazado) por el proxy.
    Http(HttpFlow),
    /// Túnel `CONNECT`: opaco, o descifrado si el MITM está activo.
    Tunnel(TunnelFlow),
}

impl FlowEvent {
    /// Id incremental del flujo, único dentro de una instancia del proxy.
    #[must_use]
    pub fn id(&self) -> u64 {
        match self {
            Self::Http(flow) => flow.id,
            Self::Tunnel(flow) => flow.id,
        }
    }
}

/// Resumen de un request HTTP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpFlow {
    /// Id incremental.
    pub id: u64,
    /// Método (`GET`, `POST`…).
    pub method: String,
    /// URL tal como la pidió el cliente (forma absoluta si es válida).
    pub url: String,
    /// Status devuelto al cliente: el del origen, o el que generó el proxy (400/502/508).
    pub status: u16,
    /// Motivo cuando el status lo generó el proxy por un error.
    pub error: Option<String>,
    /// Tiempo hasta tener los headers de la respuesta.
    pub elapsed: Duration,
    /// `Content-Length` de la respuesta, si el origen lo declaró.
    pub content_length: Option<u64>,
}

/// Resumen de un túnel `CONNECT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelFlow {
    /// Id incremental.
    pub id: u64,
    /// Destino `host:puerto`.
    pub authority: String,
    /// `200` si el túnel se abrió; si no, el status de error que devolvió el proxy.
    pub status: u16,
    /// `true` si el TLS se terminó en el proxy (los requests de adentro llegan como `HttpFlow`).
    pub intercepted: bool,
    /// Motivo cuando no se abrió.
    pub error: Option<String>,
    /// Tiempo en conectar con el destino.
    pub elapsed: Duration,
}
