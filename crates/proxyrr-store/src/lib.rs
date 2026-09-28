//! Almacenamiento de ProxyRR (spec 0007).
//!
//! [`FlowStore`] junta los eventos del motor por id y guarda los flujos completos en memoria, con
//! límites de cantidad y de bytes. [`record`] conecta un proxy con un store. La persistencia en
//! disco (guardar/abrir sesión) y el export HAR llegan en specs posteriores.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use proxyrr_core::{FlowEvent, HttpBodies, HttpFlow, TunnelFlow};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

/// Límites del almacén. Al pasarse de cualquiera se descartan los flujos más viejos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreLimits {
    /// Cantidad máxima de flujos (HTTP + túneles).
    pub max_flows: usize,
    /// Bytes máximos sumando todos los bodies guardados.
    pub max_body_bytes: u64,
}

impl Default for StoreLimits {
    fn default() -> Self {
        Self {
            max_flows: 10_000,
            max_body_bytes: 512 * 1024 * 1024,
        }
    }
}

/// Un flujo HTTP: sus headers y, cuando terminaron, sus bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRecord {
    /// Request y headers de la respuesta.
    pub head: HttpFlow,
    /// Bodies; `None` mientras la respuesta sigue llegando.
    pub bodies: Option<HttpBodies>,
}

/// Un flujo guardado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredFlow {
    /// Request HTTP (plano o descifrado).
    Http(Box<HttpRecord>),
    /// Túnel `CONNECT`.
    Tunnel(TunnelFlow),
}

impl StoredFlow {
    fn body_bytes(&self) -> u64 {
        match self {
            Self::Http(record) => record.bodies.as_ref().map_or(0, bodies_len),
            Self::Tunnel(_) => 0,
        }
    }
}

fn bodies_len(bodies: &HttpBodies) -> u64 {
    (bodies.request.data.len() + bodies.response.data.len()) as u64
}

/// Resumen de un flujo para listas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowSummary {
    /// Id del flujo.
    pub id: u64,
    /// Método HTTP, o `CONNECT` para túneles.
    pub method: String,
    /// URL, o `host:puerto` para túneles.
    pub url: String,
    /// Status de la respuesta.
    pub status: u16,
    /// `true` si el proxy registró un error.
    pub failed: bool,
    /// Tiempo hasta los headers de la respuesta (o hasta conectar, en túneles).
    pub elapsed: Duration,
    /// Tamaño real del body de respuesta si ya terminó; si no, el `Content-Length` declarado.
    pub response_size: Option<u64>,
    /// `true` mientras el body de la respuesta sigue llegando.
    pub in_progress: bool,
    /// `true` si es un túnel `CONNECT`.
    pub tunnel: bool,
}

impl From<&StoredFlow> for FlowSummary {
    fn from(flow: &StoredFlow) -> Self {
        match flow {
            StoredFlow::Http(record) => Self {
                id: record.head.id,
                method: record.head.method.clone(),
                url: record.head.url.clone(),
                status: record.head.status,
                failed: record.head.error.is_some(),
                elapsed: record.head.elapsed,
                response_size: record
                    .bodies
                    .as_ref()
                    .map(|b| b.response.size)
                    .or(record.head.content_length),
                in_progress: record.bodies.is_none(),
                tunnel: false,
            },
            StoredFlow::Tunnel(tunnel) => Self {
                id: tunnel.id,
                method: "CONNECT".to_owned(),
                url: tunnel.authority.clone(),
                status: tunnel.status,
                failed: tunnel.error.is_some(),
                elapsed: tunnel.elapsed,
                response_size: None,
                in_progress: false,
                tunnel: true,
            },
        }
    }
}

#[derive(Debug, Default)]
struct Inner {
    flows: BTreeMap<u64, StoredFlow>,
    body_bytes: u64,
}

/// Almacén en memoria de flujos, seguro entre hilos.
#[derive(Debug)]
pub struct FlowStore {
    limits: StoreLimits,
    inner: Mutex<Inner>,
    dropped_events: AtomicU64,
}

impl Default for FlowStore {
    fn default() -> Self {
        Self::new(StoreLimits::default())
    }
}

impl FlowStore {
    /// Almacén vacío con `limits`.
    #[must_use]
    pub fn new(limits: StoreLimits) -> Self {
        Self {
            limits,
            inner: Mutex::new(Inner::default()),
            dropped_events: AtomicU64::new(0),
        }
    }

    /// Aplica un evento del motor. Los bodies de un flujo que ya no está (descartado o vaciado) se ignoran.
    pub fn apply(&self, event: &FlowEvent) {
        let mut inner = self.lock();
        match event {
            FlowEvent::Http(head) => {
                let flow = StoredFlow::Http(Box::new(HttpRecord {
                    head: head.clone(),
                    bodies: None,
                }));
                insert(&mut inner, head.id, flow);
            }
            FlowEvent::Tunnel(tunnel) => {
                insert(&mut inner, tunnel.id, StoredFlow::Tunnel(tunnel.clone()));
            }
            FlowEvent::HttpBodies(bodies) => {
                let Some(StoredFlow::Http(record)) = inner.flows.get_mut(&bodies.id) else {
                    return;
                };
                let old = record.bodies.as_ref().map_or(0, bodies_len);
                record.bodies = Some(bodies.clone());
                inner.body_bytes = inner.body_bytes - old + bodies_len(bodies);
            }
        }
        self.evict(&mut inner);
    }

    /// Resúmenes de todos los flujos, en orden de llegada (id).
    #[must_use]
    pub fn list(&self) -> Vec<FlowSummary> {
        self.lock().flows.values().map(FlowSummary::from).collect()
    }

    /// Flujo completo por id. Clonarlo no copia los bodies (`Bytes` comparte el buffer).
    #[must_use]
    pub fn get(&self, id: u64) -> Option<StoredFlow> {
        self.lock().flows.get(&id).cloned()
    }

    /// Cantidad de flujos guardados.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().flows.len()
    }

    /// `true` si no hay flujos.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().flows.is_empty()
    }

    /// Bytes de bodies guardados.
    #[must_use]
    pub fn body_bytes(&self) -> u64 {
        self.lock().body_bytes
    }

    /// Borra todos los flujos.
    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.flows.clear();
        inner.body_bytes = 0;
    }

    /// Eventos que el canal descartó porque el almacén se atrasó.
    #[must_use]
    pub fn dropped_events(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }

    fn note_dropped(&self, count: u64) {
        self.dropped_events.fetch_add(count, Ordering::Relaxed);
    }

    fn evict(&self, inner: &mut Inner) {
        while inner.flows.len() > self.limits.max_flows
            || inner.body_bytes > self.limits.max_body_bytes
        {
            let Some((_, oldest)) = inner.flows.pop_first() else {
                break;
            };
            inner.body_bytes -= oldest.body_bytes();
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // Un pánico con el lock tomado no deja el mapa inconsistente: se sigue usando.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn insert(inner: &mut Inner, id: u64, flow: StoredFlow) {
    inner.body_bytes += flow.body_bytes();
    if let Some(previous) = inner.flows.insert(id, flow) {
        inner.body_bytes -= previous.body_bytes();
    }
}

/// Aplica en `store` todos los eventos de `events` hasta que el proxy se apague.
pub fn record(store: Arc<FlowStore>, mut events: Receiver<FlowEvent>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => store.apply(&event),
                Err(RecvError::Lagged(count)) => store.note_dropped(count),
                Err(RecvError::Closed) => break,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use proxyrr_core::CapturedBody;

    use super::*;

    fn http(id: u64) -> FlowEvent {
        FlowEvent::Http(HttpFlow {
            id,
            method: "GET".into(),
            url: format!("http://example.com/{id}"),
            request_headers: vec![("host".into(), "example.com".into())],
            status: 200,
            response_headers: Vec::new(),
            error: None,
            elapsed: Duration::from_millis(5),
            content_length: Some(999),
        })
    }

    fn bodies(id: u64, response: &'static str) -> FlowEvent {
        FlowEvent::HttpBodies(HttpBodies {
            id,
            request: CapturedBody {
                complete: true,
                ..CapturedBody::default()
            },
            response: CapturedBody {
                data: Bytes::from_static(response.as_bytes()),
                size: response.len() as u64,
                truncated: false,
                complete: true,
            },
            duration: Duration::from_millis(9),
        })
    }

    fn tunnel(id: u64) -> FlowEvent {
        FlowEvent::Tunnel(TunnelFlow {
            id,
            authority: "example.com:443".into(),
            status: 200,
            intercepted: false,
            error: None,
            elapsed: Duration::from_millis(1),
        })
    }

    #[test]
    fn merges_head_and_bodies_by_id() {
        let store = FlowStore::default();
        store.apply(&http(1));
        assert!(store.list()[0].in_progress);
        assert_eq!(
            store.list()[0].response_size,
            Some(999),
            "sin body, el declarado"
        );
        store.apply(&bodies(1, "hola"));
        let Some(StoredFlow::Http(record)) = store.get(1) else {
            panic!("falta el flujo 1");
        };
        assert_eq!(record.bodies.unwrap().response.data, "hola");
        let summary = &store.list()[0];
        assert!(!summary.in_progress);
        assert_eq!(
            summary.response_size,
            Some(4),
            "con body, el tamaño real (TD-002)"
        );
        assert_eq!(store.body_bytes(), 4);
    }

    #[test]
    fn lists_in_arrival_order_with_tunnels() {
        let store = FlowStore::default();
        store.apply(&http(3));
        store.apply(&tunnel(1));
        store.apply(&http(2));
        let ids: Vec<u64> = store.list().iter().map(|s| s.id).collect();
        assert_eq!(ids, [1, 2, 3]);
        assert!(store.list()[0].tunnel);
        assert_eq!(store.list()[0].method, "CONNECT");
    }

    #[test]
    fn orphan_bodies_are_ignored() {
        let store = FlowStore::default();
        store.apply(&bodies(42, "nadie"));
        assert!(store.is_empty());
        assert_eq!(store.body_bytes(), 0);
    }

    #[test]
    fn evicts_oldest_by_count() {
        let store = FlowStore::new(StoreLimits {
            max_flows: 2,
            ..StoreLimits::default()
        });
        for id in 1..=3 {
            store.apply(&http(id));
        }
        let ids: Vec<u64> = store.list().iter().map(|s| s.id).collect();
        assert_eq!(ids, [2, 3]);
    }

    #[test]
    fn evicts_oldest_by_body_bytes() {
        let store = FlowStore::new(StoreLimits {
            max_flows: 100,
            max_body_bytes: 10,
        });
        store.apply(&http(1));
        store.apply(&bodies(1, "123456"));
        store.apply(&http(2));
        store.apply(&bodies(2, "789012"));
        let ids: Vec<u64> = store.list().iter().map(|s| s.id).collect();
        assert_eq!(ids, [2]);
        assert_eq!(store.body_bytes(), 6);
    }

    #[test]
    fn clear_empties_everything() {
        let store = FlowStore::default();
        store.apply(&http(1));
        store.apply(&bodies(1, "x"));
        store.clear();
        assert!(store.is_empty());
        assert_eq!(store.body_bytes(), 0);
        store.apply(&bodies(1, "tarde"));
        assert!(
            store.is_empty(),
            "bodies de un flujo borrado no lo resucitan"
        );
    }

    #[tokio::test]
    async fn record_counts_dropped_events() {
        let (tx, rx) = tokio::sync::broadcast::channel(2);
        for id in 1..=5 {
            tx.send(http(id)).unwrap();
        }
        drop(tx);
        let store = Arc::new(FlowStore::default());
        record(Arc::clone(&store), rx).await.unwrap();
        assert_eq!(store.dropped_events(), 3);
        assert_eq!(store.len(), 2);
    }
}
