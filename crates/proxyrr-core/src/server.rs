//! Listener y ciclo de vida de una instancia del proxy.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;

use crate::config::ProxyConfig;
use crate::event::FlowEvent;
use crate::handler::{self, Context, HEADER_READ_TIMEOUT};

/// Capacidad del canal de eventos. Un suscriptor que se atrasa más que esto recibe `Lagged`.
const EVENT_CAPACITY: usize = 1024;
/// Pausa tras un error de `accept` (p. ej. sin descriptores libres) para no girar en vacío.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);

/// Una instancia del proxy escuchando.
#[derive(Debug)]
pub struct Proxy {
    local_addr: SocketAddr,
    events: broadcast::Sender<FlowEvent>,
    shutdown: oneshot::Sender<()>,
    accept_loop: JoinHandle<()>,
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
        let ctx = Arc::new(Context::new(&config, local_addr, events.clone())?);
        let (shutdown, stop) = oneshot::channel();
        let accept_loop = tokio::spawn(accept_loop(listener, ctx, stop));
        Ok(Self {
            local_addr,
            events,
            shutdown,
            accept_loop,
        })
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

    /// Deja de aceptar conexiones y libera el puerto. Las conexiones en curso terminan solas.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(());
        let _ = self.accept_loop.await;
    }
}

async fn accept_loop(listener: TcpListener, ctx: Arc<Context>, mut stop: oneshot::Receiver<()>) {
    loop {
        tokio::select! {
            _ = &mut stop => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _peer)) => {
                    tokio::spawn(serve_connection(stream, Arc::clone(&ctx)));
                }
                // Transitorio (p. ej. sin descriptores libres): pausa y reintento. Sin logging todavía,
                // el error no se reporta; ver TD-006 en specs/TECH-DEBT.md.
                Err(_) => tokio::time::sleep(ACCEPT_BACKOFF).await,
            },
        }
    }
}

async fn serve_connection(stream: TcpStream, ctx: Arc<Context>) {
    let _ = stream.set_nodelay(true);
    let service = service_fn(move |req| {
        let ctx = Arc::clone(&ctx);
        async move { handler::handle(req, &ctx).await }
    });
    // Los errores de una conexión (cliente que corta, request mal formado) no afectan al resto.
    let _ = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades()
        .await;
}
