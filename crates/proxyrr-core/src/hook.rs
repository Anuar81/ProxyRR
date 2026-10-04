//! Punto de extensión que modifica tráfico (spec 0011): Map Local/Remote, Block, No Caching, Breakpoint.
//!
//! El motor no conoce las reglas: por cada request le pide a un [`FlowHook`] un [`Plan`] (rápido, sin
//! bloquear) y lo ejecuta. Solo si el plan pide pausar, el motor junta el mensaje completo y espera la
//! decisión del hook ([`FlowHook::pause`]). Las reglas viven en `proxyrr-rules`.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use bytes::Bytes;

use crate::event::Headers;
use crate::local::LocalResponse;

/// Future en caja que devuelve un hook.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Request tal como llegó del cliente, antes de cualquier regla.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestHead {
    /// Id del flujo.
    pub id: u64,
    /// Método.
    pub method: String,
    /// URL absoluta.
    pub url: String,
    /// Headers.
    pub headers: Headers,
}

/// Cambios de headers: primero se quitan (sin importar mayúsculas), después se fijan (reemplazando).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderEdits {
    /// Nombres a quitar.
    pub remove: Vec<String>,
    /// Headers a fijar.
    pub set: Vec<(String, String)>,
}

impl HeaderEdits {
    /// `true` si no cambia nada.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.remove.is_empty() && self.set.is_empty()
    }
}

/// Qué hacer con un flujo. `Plan::default()` deja el flujo como está.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Nombres de las reglas aplicadas (se muestran en el flujo).
    pub rules: Vec<String>,
    /// Responder esto sin ir al origen (Map Local, Block).
    pub respond: Option<LocalResponse>,
    /// URL absoluta nueva (Map Remote).
    pub redirect: Option<String>,
    /// Con `redirect`, mantener el `Host` original.
    pub preserve_host: bool,
    /// Cambios en los headers del request.
    pub request_headers: HeaderEdits,
    /// Cambios en los headers de la respuesta.
    pub response_headers: HeaderEdits,
    /// Pausar el request antes de mandarlo.
    pub pause_request: bool,
    /// Pausar la respuesta antes de devolverla.
    pub pause_response: bool,
}

/// Lado de un flujo en pausa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Antes de mandar el request al origen.
    Request,
    /// Antes de devolver la respuesta al cliente.
    Response,
}

/// Mensaje completo en pausa. En `Request` se pueden cambiar método, URL, headers y body; en `Response`,
/// status, headers y body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paused {
    /// Id del flujo.
    pub id: u64,
    /// Método del request.
    pub method: String,
    /// URL del request.
    pub url: String,
    /// Status de la respuesta (`None` en `Request`).
    pub status: Option<u16>,
    /// Headers del lado en pausa.
    pub headers: Headers,
    /// Body completo del lado en pausa (sin decodificar).
    pub body: Bytes,
}

/// Decisión sobre un mensaje en pausa.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Seguir con este mensaje (el original o uno editado).
    Continue(Paused),
    /// Cortar: el cliente recibe 503.
    Abort,
}

/// Hook que decide los cambios de cada flujo.
pub trait FlowHook: Send + Sync + fmt::Debug {
    /// Plan para un request. Corre en el hilo de la conexión: tiene que ser rápido.
    fn plan(&self, head: &RequestHead) -> Plan;

    /// Espera la decisión sobre un mensaje en pausa. Por defecto sigue sin cambios.
    fn pause(&self, stage: Stage, message: Paused) -> BoxFuture<Verdict> {
        let _ = stage;
        Box::pin(async move { Verdict::Continue(message) })
    }
}
