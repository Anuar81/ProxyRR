//! Listener y ciclo de vida de una instancia del proxy.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;

use crate::config::ProxyConfig;
use crate::event::FlowEvent;
use crate::handler::{self, Context, HEADER_READ_TIMEOUT, Replay};
use crate::lifecycle::{Lifecycle, Phase};

/// Capacidad del canal de eventos. Un suscriptor que se atrasa más que esto recibe `Lagged`.
const EVENT_CAPACITY: usize = 1024;
/// Pausa tras un error de `accept` (p. ej. sin descriptores libres) para no girar en vacío.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);
/// Cada cuánto, como mucho, se registra un error de `accept` repetido (con el conteo acumulado).
const ACCEPT_LOG_EVERY: Duration = Duration::from_secs(5);
/// Plazo por defecto para que terminen los requests en curso al apagar.
pub const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// Tras vencer el plazo y cortar todo, cuánto más se espera a que las tareas se suelten.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// Una instancia del proxy escuchando.
#[derive(Debug)]
pub struct Proxy {
    local_addr: SocketAddr,
    events: broadcast::Sender<FlowEvent>,
    shutdown: oneshot::Sender<()>,
    accept_loop: JoinHandle<()>,
    lifecycle: Lifecycle,
    ctx: Arc<Context>,
}

impl Proxy {
    /// Abre el listener y empieza a aceptar conexiones en segundo plano.
    ///
    /// # Errors
    ///
    /// Si no se puede escuchar en `config.listen` (puerto ocupado, sin permisos…).
    pub async fn start(config: ProxyConfig) -> io::Result<Self> {
        let listener = TcpListener::bind(config.listen).await?;
        let local_addr = listener.local_addr()?;
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let mut lifecycle = Lifecycle::new();
        let ctx = Arc::new(Context::new(
            &config,
            local_addr,
            events.clone(),
            lifecycle.tracker(),
        )?);
        let (shutdown, stop) = oneshot::channel();
        let accept_loop = tokio::spawn(accept_loop(listener, Arc::clone(&ctx), stop));
        tracing::debug!(%local_addr, mitm = config.mitm.is_some(), "proxy escuchando");
        Ok(Self {
            local_addr,
            events,
            shutdown,
            accept_loop,
            lifecycle,
            ctx,
        })
    }

    /// Manda un request armado a mano (Repeat, Compose) por el pipeline del proxy: se le aplican las
    /// reglas y se captura como cualquier flujo, también si es HTTPS. Devuelve el id del flujo, cuyo
    /// evento llega por [`Proxy::subscribe`].
    ///
    /// # Errors
    ///
    /// Método o URL inválidos, o una URL que apunta al propio proxy.
    pub fn replay(&self, request: Replay) -> Result<u64, String> {
        handler::replay(&self.ctx, request)
    }

    /// Dirección efectiva de escucha (resuelve el puerto real si se pidió el `0`).
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Suscribe a los eventos de flujo. Solo recibe los eventos posteriores a la suscripción.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<FlowEvent> {
        self.events.subscribe()
    }

    /// Apaga con el plazo por defecto ([`DEFAULT_SHUTDOWN_GRACE`]). Ver [`Proxy::shutdown_within`].
    pub async fn shutdown(self) {
        self.shutdown_within(DEFAULT_SHUTDOWN_GRACE).await;
    }

    /// Apagado ordenado: deja de aceptar y libera el puerto, cierra los túneles opacos y las
    /// conexiones keep-alive ociosas, y espera hasta `grace` a que terminen los requests en curso. Lo
    /// que siga abierto pasado el plazo se corta. Al volver ya no queda ninguna conexión del proxy.
    pub async fn shutdown_within(self, grace: Duration) {
        let Self {
            shutdown,
            accept_loop,
            mut lifecycle,
            ctx,
            ..
        } = self;
        let _ = shutdown.send(());
        let _ = accept_loop.await;
        // El contexto lleva un tracker de conexiones: soltarlo para que `idle` cuente solo las que siguen.
        drop(ctx);
        lifecycle.set(Phase::Draining);
        if tokio::time::timeout(grace, lifecycle.idle()).await.is_err() {
            tracing::debug!(
                ?grace,
                "se venció el plazo de apagado: se cortan las conexiones"
            );
            lifecycle.set(Phase::Closing);
            let _ = tokio::time::timeout(CLOSE_WAIT, lifecycle.idle()).await;
        }
    }
}

async fn accept_loop(listener: TcpListener, ctx: Arc<Context>, mut stop: oneshot::Receiver<()>) {
    let mut failures = AcceptFailures::default();
    loop {
        tokio::select! {
            _ = &mut stop => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _peer)) => {
                    failures.recovered();
                    tokio::spawn(serve_connection(stream, Arc::clone(&ctx)));
                }
                // Transitorio (p. ej. sin descriptores libres): pausa y reintento, registrado sin inundar.
                Err(error) => {
                    failures.failed(&error);
                    tokio::time::sleep(ACCEPT_BACKOFF).await;
                }
            },
        }
    }
}

/// Registro de errores de `accept` con conteo: el primero enseguida, los siguientes agrupados.
#[derive(Debug, Default)]
struct AcceptFailures {
    pending: u64,
    last_log: Option<Instant>,
}

impl AcceptFailures {
    fn failed(&mut self, error: &io::Error) {
        self.pending += 1;
        if self
            .last_log
            .is_none_or(|t| t.elapsed() >= ACCEPT_LOG_EVERY)
        {
            tracing::warn!(
                %error,
                count = self.pending,
                "el proxy no puede aceptar conexiones; se reintenta"
            );
            self.pending = 0;
            self.last_log = Some(Instant::now());
        }
    }

    fn recovered(&mut self) {
        if self.last_log.take().is_some() && self.pending > 0 {
            tracing::warn!(
                count = self.pending,
                "el proxy volvió a aceptar conexiones tras errores"
            );
        }
        self.pending = 0;
    }
}

async fn serve_connection(stream: TcpStream, ctx: Arc<Context>) {
    let _ = stream.set_nodelay(true);
    let tracker = ctx.tracker().clone();
    let service_ctx = Arc::clone(&ctx);
    let service = service_fn(move |req| {
        let ctx = Arc::clone(&service_ctx);
        async move { handler::handle(req, &ctx).await }
    });
    let conn = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    tokio::pin!(conn);
    // Los errores de una conexión (cliente que corta, request mal formado) no afectan al resto.
    tokio::select! {
        result = conn.as_mut() => return log_connection_error(result),
        () = tracker.reached(Phase::Draining) => conn.as_mut().graceful_shutdown(),
    }
    tokio::select! {
        result = conn => log_connection_error(result),
        () = tracker.reached(Phase::Closing) => {}
    }
}

fn log_connection_error(result: Result<(), hyper::Error>) {
    if let Err(error) = result {
        tracing::debug!(%error, "conexión del cliente terminada con error");
    }
}
