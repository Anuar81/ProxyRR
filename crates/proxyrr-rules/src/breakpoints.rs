//! Flujos en pausa por un breakpoint, esperando la decisión de una UI.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use proxyrr_core::{BoxFuture, Paused, Stage, Verdict};
use tokio::sync::{broadcast, oneshot};

/// Tiempo que un flujo espera en pausa antes de seguir sin cambios.
pub const DEFAULT_PAUSE_TIMEOUT: Duration = Duration::from_mins(5);

/// Un flujo en pausa.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PausedFlow {
    /// Clave para resolverlo (no es el id del flujo: un flujo puede pausar en request y en response).
    pub key: u64,
    /// Lado en pausa.
    pub stage: Stage,
    /// Mensaje completo.
    pub message: Paused,
}

/// Aviso para la UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BreakpointEvent {
    /// Un flujo quedó en pausa.
    Paused(PausedFlow),
    /// Ya no está en pausa (resuelto, vencido o el cliente cortó).
    Resolved {
        /// Clave.
        key: u64,
    },
}

#[derive(Debug)]
struct Pending {
    flow: PausedFlow,
    reply: oneshot::Sender<Verdict>,
}

#[derive(Debug)]
struct Hub {
    pending: Mutex<BTreeMap<u64, Pending>>,
    next: AtomicU64,
    events: broadcast::Sender<BreakpointEvent>,
    timeout: Duration,
}

impl Hub {
    fn take(&self, key: u64) -> Option<Pending> {
        let taken = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key);
        if taken.is_some() {
            let _ = self.events.send(BreakpointEvent::Resolved { key });
        }
        taken
    }
}

/// Cola de breakpoints. Clonarla comparte la misma cola.
#[derive(Debug, Clone)]
pub struct Breakpoints(Arc<Hub>);

impl Default for Breakpoints {
    fn default() -> Self {
        Self::with_timeout(DEFAULT_PAUSE_TIMEOUT)
    }
}

impl Breakpoints {
    /// Cola con otro plazo de pausa.
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> Self {
        Self(Arc::new(Hub {
            pending: Mutex::new(BTreeMap::new()),
            next: AtomicU64::new(1),
            events: broadcast::channel(256).0,
            timeout,
        }))
    }

    /// Avisos de pausa. Mientras haya un suscriptor, los breakpoints pausan; sin ninguno (CLI) no.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<BreakpointEvent> {
        self.0.events.subscribe()
    }

    /// `true` si hay una UI escuchando.
    #[must_use]
    pub fn has_listener(&self) -> bool {
        self.0.events.receiver_count() > 0
    }

    /// Flujos en pausa, en orden de llegada.
    #[must_use]
    pub fn pending(&self) -> Vec<PausedFlow> {
        self.0
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .map(|p| p.flow.clone())
            .collect()
    }

    /// Decide sobre un flujo en pausa. `false` si ya no estaba (vencido o el cliente cortó).
    pub fn resolve(&self, key: u64, verdict: Verdict) -> bool {
        match self.0.take(key) {
            Some(pending) => {
                let _ = pending.reply.send(verdict);
                true
            }
            None => false,
        }
    }

    /// Pone un mensaje en pausa y espera la decisión (o el plazo, y sigue sin cambios).
    pub(crate) fn pause(&self, stage: Stage, message: Paused) -> BoxFuture<Verdict> {
        if !self.has_listener() {
            return Box::pin(async move { Verdict::Continue(message) });
        }
        let hub = Arc::clone(&self.0);
        let key = hub.next.fetch_add(1, Ordering::Relaxed);
        let (reply, wait) = oneshot::channel();
        let flow = PausedFlow {
            key,
            stage,
            message: message.clone(),
        };
        hub.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                key,
                Pending {
                    flow: flow.clone(),
                    reply,
                },
            );
        let _ = hub.events.send(BreakpointEvent::Paused(flow));
        // Si el cliente corta, el future se descarta (aunque no se haya ejecutado nunca): el guard
        // saca el flujo de la cola.
        let guard = Cleanup {
            hub: Arc::clone(&hub),
            key,
        };
        Box::pin(async move {
            let verdict = match tokio::time::timeout(hub.timeout, wait).await {
                Ok(Ok(verdict)) => verdict,
                _ => Verdict::Continue(message),
            };
            drop(guard);
            verdict
        })
    }
}

/// Saca un flujo de la cola al terminar su espera, de la forma que sea.
struct Cleanup {
    hub: Arc<Hub>,
    key: u64,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.hub.take(self.key);
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;

    fn message() -> Paused {
        Paused {
            id: 9,
            method: "GET".into(),
            url: "https://x.com/".into(),
            status: None,
            headers: vec![],
            body: Bytes::from_static(b"hola"),
        }
    }

    #[tokio::test]
    async fn breakpoint_without_listener_continues() {
        let bp = Breakpoints::default();
        let verdict = bp.pause(Stage::Request, message()).await;
        assert_eq!(verdict, Verdict::Continue(message()));
        assert!(bp.pending().is_empty());
    }

    #[tokio::test]
    async fn breakpoint_waits_for_the_ui() {
        let bp = Breakpoints::default();
        let mut events = bp.subscribe();
        let waiting = tokio::spawn(bp.pause(Stage::Request, message()));
        let BreakpointEvent::Paused(flow) = events.recv().await.unwrap() else {
            panic!("debía pausar");
        };
        assert_eq!(bp.pending().len(), 1);
        let mut edited = flow.message.clone();
        edited.body = Bytes::from_static(b"chau");
        assert!(bp.resolve(flow.key, Verdict::Continue(edited.clone())));
        assert_eq!(waiting.await.unwrap(), Verdict::Continue(edited));
        assert_eq!(
            events.recv().await.unwrap(),
            BreakpointEvent::Resolved { key: flow.key }
        );
        assert!(!bp.resolve(flow.key, Verdict::Abort), "ya resuelto");
        assert!(bp.pending().is_empty());
    }

    #[tokio::test]
    async fn breakpoint_times_out_unchanged() {
        let bp = Breakpoints::with_timeout(Duration::from_millis(50));
        let _events = bp.subscribe();
        let verdict = bp.pause(Stage::Response, message()).await;
        assert_eq!(verdict, Verdict::Continue(message()));
        assert!(bp.pending().is_empty());
    }

    #[tokio::test]
    async fn breakpoint_dropped_by_the_client_leaves_the_queue() {
        let bp = Breakpoints::default();
        let mut events = bp.subscribe();
        let waiting = tokio::spawn(bp.pause(Stage::Request, message()));
        let BreakpointEvent::Paused(flow) = events.recv().await.unwrap() else {
            panic!("debía pausar");
        };
        waiting.abort();
        let _ = waiting.await;
        assert_eq!(
            events.recv().await.unwrap(),
            BreakpointEvent::Resolved { key: flow.key }
        );
        assert!(bp.pending().is_empty());
    }
}
