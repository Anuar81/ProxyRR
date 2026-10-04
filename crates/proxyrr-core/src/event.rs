//! Eventos que publica el motor por cada flujo.
//!
//! Es el punto de extensión de solo lectura: CLI, API de control y store se suscriben sin tocar
//! el motor.

use std::time::Duration;

use bytes::Bytes;

/// Lista de headers en el orden en que viajaron, con repetidos. Los valores que no son UTF-8 se
/// convierten con reemplazo.
pub type Headers = Vec<(String, String)>;

/// Un flujo observado por el proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowEvent {
    /// Request HTTP reenviado (o rechazado) por el proxy, al tener los headers de la respuesta.
    Http(HttpFlow),
    /// Bodies de un `Http` ya emitido (mismo id), cuando terminaron los dos. Siempre llega después.
    HttpBodies(HttpBodies),
    /// Túnel `CONNECT`: opaco, o descifrado si el MITM está activo.
    Tunnel(TunnelFlow),
}

impl FlowEvent {
    /// Id incremental del flujo, único dentro de una instancia del proxy.
    #[must_use]
    pub fn id(&self) -> u64 {
        match self {
            Self::Http(flow) => flow.id,
            Self::HttpBodies(bodies) => bodies.id,
            Self::Tunnel(flow) => flow.id,
        }
    }
}

/// Request HTTP y los headers de su respuesta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpFlow {
    /// Id incremental.
    pub id: u64,
    /// Método (`GET`, `POST`…).
    pub method: String,
    /// URL tal como la pidió el cliente (forma absoluta si es válida), aunque una regla la redirija.
    pub url: String,
    /// Headers del request tal como salieron hacia el origen (con los cambios de las reglas).
    pub request_headers: Headers,
    /// Status devuelto al cliente: el del origen, el de una regla, o el que generó el proxy (400/502/508).
    pub status: u16,
    /// Headers de la respuesta tal como llegaron al cliente (antes de quitar los hop-by-hop).
    pub response_headers: Headers,
    /// Reglas que modificaron el flujo (spec 0011), en el orden en que se aplicaron.
    pub rules: Vec<String>,
    /// Motivo cuando el status lo generó el proxy por un error.
    pub error: Option<String>,
    /// Tiempo hasta tener los headers de la respuesta.
    pub elapsed: Duration,
    /// `Content-Length` de la respuesta, si se declaró.
    pub content_length: Option<u64>,
}

/// Bodies capturados de un flujo HTTP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpBodies {
    /// Id del `HttpFlow` al que pertenecen.
    pub id: u64,
    /// Body del request.
    pub request: CapturedBody,
    /// Body de la respuesta.
    pub response: CapturedBody,
    /// Tiempo desde que llegó el request hasta que terminaron los dos bodies.
    pub duration: Duration,
}

/// Un body capturado. Los bytes son los que viajaron (sin descomprimir).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapturedBody {
    /// Bytes guardados (hasta el límite de captura).
    pub data: Bytes,
    /// Tamaño real que pasó por el proxy, aunque se haya truncado.
    pub size: u64,
    /// `true` si `data` tiene menos bytes que `size`.
    pub truncated: bool,
    /// `false` si el body se cortó antes de terminar (cliente u origen que cerraron).
    pub complete: bool,
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
