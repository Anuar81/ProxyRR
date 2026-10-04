//! `Engine`: proxy + store con ciclo de vida propio (spec 0008, CA 5).

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use proxyrr_cert::CertificateAuthority;
use proxyrr_core::{FlowEvent, FlowHook, MitmConfig, Proxy, ProxyConfig, Replay};
use proxyrr_rules::Rules;
use proxyrr_store::{FlowStore, FlowSummary, StoreLimits, StoredFlow};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Mutex, broadcast};

use crate::log::{LogLayer, LogLevel};

/// Capacidad de los canales del engine. Un suscriptor más atrasado recibe `Lagged`.
const CHANNEL_CAPACITY: usize = 4096;

/// Cómo prender el proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxySettings {
    /// Dirección de escucha.
    pub listen: SocketAddr,
    /// Descifrar HTTPS (requiere CA en el engine).
    pub mitm: bool,
    /// Hosts sin descifrar (solo con `mitm`).
    pub bypass: Vec<String>,
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self {
            listen: ProxyConfig::default().listen,
            mitm: false,
            bypass: Vec::new(),
        }
    }
}

/// Estado del proxy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyStatus {
    /// `true` si está escuchando.
    pub running: bool,
    /// Dirección efectiva, si está prendido.
    pub listen: Option<SocketAddr>,
    /// Descifrado activo.
    pub mitm: bool,
    /// Hosts sin descifrar.
    pub bypass: Vec<String>,
}

/// Estado completo del engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineStatus {
    /// Proxy.
    pub proxy: ProxyStatus,
    /// Flujos guardados.
    pub flows: usize,
    /// Bytes de bodies guardados.
    pub body_bytes: u64,
    /// Eventos del motor que el store no llegó a leer.
    pub dropped_events: u64,
}

/// Aviso para UIs, publicado después de aplicar el cambio en el store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// Flujo nuevo o actualizado (resumen ya fusionado).
    Flow(FlowSummary),
    /// Se vació el store.
    Cleared,
    /// El proxy se prendió o apagó.
    Proxy(ProxyStatus),
    /// Advertencia o error del motor (TD-006).
    Log {
        /// Nivel.
        level: LogLevel,
        /// Texto, con los campos del evento.
        message: String,
    },
}

/// Errores al manejar el proxy.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// Ya hay un proxy prendido.
    #[error("el proxy ya está prendido en {0}")]
    AlreadyRunning(SocketAddr),
    /// No se pudo abrir el puerto.
    #[error("no se pudo escuchar en {addr}: {source}")]
    Bind {
        /// Dirección pedida.
        addr: SocketAddr,
        /// Causa.
        source: io::Error,
    },
    /// Se pidió MITM sin CA.
    #[error("se pidió descifrar HTTPS pero no hay CA disponible")]
    NoCa,
}

/// Opciones del engine.
#[derive(Default)]
pub struct EngineOptions {
    /// Límites del store.
    pub limits: StoreLimits,
    /// CA para el MITM. Sin CA, `mitm: true` da [`EngineError::NoCa`].
    pub ca: Option<Arc<CertificateAuthority>>,
    /// Plantilla del proxy (`upstream_roots`, `max_body_capture`, `flow_ids`). `listen` y `mitm` se
    /// toman de cada [`ProxySettings`]; `hook` se reemplaza por `rules`.
    pub proxy: ProxyConfig,
    /// Reglas (spec 0011). Sin esto, unas en memoria y vacías.
    pub rules: Option<Arc<Rules>>,
}

impl fmt::Debug for EngineOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineOptions")
            .field("limits", &self.limits)
            .field("ca", &self.ca.is_some())
            .field("proxy", &self.proxy)
            .field("rules", &self.rules.is_some())
            .finish()
    }
}

#[derive(Debug, Default)]
struct State {
    proxy: Option<Proxy>,
    status: ProxyStatus,
}

/// Proxy + store. Los canales y el contador de ids sobreviven a apagar y prender el proxy.
pub struct Engine {
    store: Arc<FlowStore>,
    ca: Option<Arc<CertificateAuthority>>,
    rules: Arc<Rules>,
    template: ProxyConfig,
    state: Mutex<State>,
    notices: broadcast::Sender<Notice>,
    flows: broadcast::Sender<FlowEvent>,
}

impl fmt::Debug for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("flows", &self.store.len())
            .field("ca", &self.ca.is_some())
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// Engine con el proxy apagado.
    #[must_use]
    pub fn new(options: EngineOptions) -> Self {
        let mut template = options.proxy;
        template.mitm = None;
        let rules = options
            .rules
            .unwrap_or_else(|| Arc::new(Rules::in_memory()));
        template.hook = Some(Arc::clone(&rules) as Arc<dyn FlowHook>);
        Self {
            store: Arc::new(FlowStore::new(options.limits)),
            ca: options.ca,
            rules,
            template,
            state: Mutex::new(State::default()),
            notices: broadcast::channel(CHANNEL_CAPACITY).0,
            flows: broadcast::channel(CHANNEL_CAPACITY).0,
        }
    }

    /// Reglas vivas: cambiarlas aplica al próximo request.
    #[must_use]
    pub fn rules(&self) -> &Arc<Rules> {
        &self.rules
    }

    /// Manda un request armado a mano por el proxy (se le aplican las reglas y se captura).
    ///
    /// # Errors
    /// Proxy apagado, o método/URL inválidos.
    pub async fn replay(&self, request: Replay) -> Result<u64, String> {
        let state = self.state.lock().await;
        let proxy = state
            .proxy
            .as_ref()
            .ok_or("el proxy está apagado: prendelo para repetir requests")?;
        proxy.replay(request)
    }

    /// Repite un flujo guardado tal cual (método, URL, headers y body del request).
    ///
    /// # Errors
    /// Flujo que ya no está, túnel, body truncado o proxy apagado.
    pub async fn replay_flow(&self, id: u64) -> Result<u64, String> {
        let request = self.replay_request(id)?;
        self.replay(request).await
    }

    /// El request de un flujo guardado, listo para repetirlo o editarlo.
    ///
    /// # Errors
    /// Flujo que ya no está, túnel o body truncado.
    pub fn replay_request(&self, id: u64) -> Result<Replay, String> {
        let Some(StoredFlow::Http(record)) = self.store.get(id) else {
            return Err(format!("el flujo {id} no está o es un túnel"));
        };
        let body = match &record.bodies {
            Some(b) if b.request.truncated => {
                return Err(format!(
                    "el body del request {id} se guardó recortado: no se puede repetir igual"
                ));
            }
            Some(b) => b.request.data.clone(),
            None => bytes::Bytes::new(),
        };
        Ok(Replay {
            method: record.head.method.clone(),
            url: record.head.url.clone(),
            headers: record.head.request_headers.clone(),
            body,
        })
    }

    /// Store de flujos.
    #[must_use]
    pub fn store(&self) -> &Arc<FlowStore> {
        &self.store
    }

    /// `true` si el engine tiene CA para descifrar.
    #[must_use]
    pub fn has_ca(&self) -> bool {
        self.ca.is_some()
    }

    /// Avisos para UIs. Solo los posteriores a la suscripción.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Notice> {
        self.notices.subscribe()
    }

    /// Eventos crudos del motor (con ids ya únicos entre arranques), después de guardarlos.
    #[must_use]
    pub fn subscribe_flows(&self) -> broadcast::Receiver<FlowEvent> {
        self.flows.subscribe()
    }

    /// Prende el proxy.
    ///
    /// # Errors
    ///
    /// Si ya está prendido, si el puerto no se puede abrir o si se pide MITM sin CA.
    pub async fn start_proxy(&self, settings: ProxySettings) -> Result<ProxyStatus, EngineError> {
        let mut state = self.state.lock().await;
        if let Some(proxy) = &state.proxy {
            return Err(EngineError::AlreadyRunning(proxy.local_addr()));
        }
        let mitm = if settings.mitm {
            let ca = self.ca.clone().ok_or(EngineError::NoCa)?;
            let mut mitm = MitmConfig::new(ca);
            mitm.bypass.clone_from(&settings.bypass);
            Some(mitm)
        } else {
            None
        };
        let config = ProxyConfig {
            listen: settings.listen,
            mitm,
            ..self.template.clone()
        };
        let proxy = Proxy::start(config)
            .await
            .map_err(|source| EngineError::Bind {
                addr: settings.listen,
                source,
            })?;
        tokio::spawn(record(
            proxy.subscribe(),
            Arc::clone(&self.store),
            self.notices.clone(),
            self.flows.clone(),
        ));
        let status = ProxyStatus {
            running: true,
            listen: Some(proxy.local_addr()),
            mitm: settings.mitm,
            bypass: if settings.mitm {
                settings.bypass
            } else {
                Vec::new()
            },
        };
        state.proxy = Some(proxy);
        state.status = status.clone();
        let _ = self.notices.send(Notice::Proxy(status.clone()));
        Ok(status)
    }

    /// Apaga el proxy. Idempotente. Lo que capturen las conexiones que siguen abiertas se guarda igual.
    pub async fn stop_proxy(&self) -> ProxyStatus {
        let mut state = self.state.lock().await;
        if let Some(proxy) = state.proxy.take() {
            proxy.shutdown().await;
            state.status = ProxyStatus::default();
            let _ = self.notices.send(Notice::Proxy(state.status.clone()));
        }
        state.status.clone()
    }

    /// Estado del proxy y del store.
    pub async fn status(&self) -> EngineStatus {
        let proxy = self.state.lock().await.status.clone();
        EngineStatus {
            proxy,
            flows: self.store.len(),
            body_bytes: self.store.body_bytes(),
            dropped_events: self.store.dropped_events(),
        }
    }

    /// Vacía el store.
    pub fn clear(&self) {
        self.store.clear();
        let _ = self.notices.send(Notice::Cleared);
    }

    /// Capa de `tracing` que publica los `warn`/`error` del motor como [`Notice::Log`]. Se instala
    /// una vez, junto al subscriber del proceso.
    #[must_use]
    pub fn log_layer(&self) -> LogLayer {
        LogLayer::new(self.notices.clone())
    }

    /// HAR 1.2 con todos los flujos HTTP guardados.
    #[must_use]
    pub fn har(&self) -> serde_json::Value {
        crate::har::to_har(&self.store.snapshot())
    }
}

/// Guarda cada evento de un proxy y lo republica. Termina cuando el proxy y todas sus conexiones se cierran.
async fn record(
    mut events: broadcast::Receiver<FlowEvent>,
    store: Arc<FlowStore>,
    notices: broadcast::Sender<Notice>,
    flows: broadcast::Sender<FlowEvent>,
) {
    loop {
        match events.recv().await {
            Ok(event) => {
                store.apply(&event);
                let id = event.id();
                let _ = flows.send(event);
                if let Some(summary) = store.summary(id) {
                    let _ = notices.send(Notice::Flow(summary));
                }
            }
            Err(RecvError::Lagged(count)) => store.note_dropped(count),
            Err(RecvError::Closed) => break,
        }
    }
}
