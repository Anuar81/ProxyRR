//! Apagado ordenado (TD-004): dejar de aceptar, terminar lo que está en curso y, pasado un plazo,
//! cortar lo que quede.
//!
//! Cada tarea de conexión tiene una [`Tracker`]. `Proxy::shutdown` espera a que se suelten todas
//! (un canal `mpsc` sin mensajes: `recv` devuelve `None` cuando no queda ningún `Sender`).

use tokio::sync::{mpsc, watch};

/// Fase del apagado. Las conexiones la miran para decidir qué hacer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Phase {
    /// Funcionando.
    Running,
    /// Apagando: no se toman requests nuevos en conexiones keep-alive, los túneles opacos se cierran
    /// y los requests en curso terminan.
    Draining,
    /// Se venció el plazo: se corta todo.
    Closing,
}

/// Lado del proxy: cambia la fase y espera a que se suelten las tareas.
#[derive(Debug)]
pub(crate) struct Lifecycle {
    phase: watch::Sender<Phase>,
    alive: mpsc::Receiver<()>,
    tracker: Option<Tracker>,
}

impl Lifecycle {
    pub(crate) fn new() -> Self {
        let (phase, phase_rx) = watch::channel(Phase::Running);
        let (alive_tx, alive) = mpsc::channel(1);
        Self {
            phase,
            alive,
            tracker: Some(Tracker {
                phase: phase_rx,
                _alive: alive_tx,
            }),
        }
    }

    /// Tracker original, para repartir entre las tareas. Se entrega una sola vez.
    pub(crate) fn tracker(&mut self) -> Tracker {
        self.tracker
            .take()
            .expect("el tracker del lifecycle se pide una sola vez")
    }

    pub(crate) fn set(&self, phase: Phase) {
        self.phase.send_replace(phase);
    }

    /// Espera a que no quede ninguna tarea.
    pub(crate) async fn idle(&mut self) {
        self.tracker = None;
        while self.alive.recv().await.is_some() {}
    }
}

/// Lado de una tarea: mientras exista, el proxy la espera al apagarse.
#[derive(Debug, Clone)]
pub(crate) struct Tracker {
    phase: watch::Receiver<Phase>,
    _alive: mpsc::Sender<()>,
}

impl Tracker {
    /// Resuelve cuando la fase llega a `phase`. Si el proxy se soltó sin apagarlo, no resuelve nunca:
    /// las conexiones siguen hasta cerrarse solas, como antes de existir el apagado ordenado.
    pub(crate) async fn reached(&self, phase: Phase) {
        let mut rx = self.phase.clone();
        if rx.wait_for(|current| *current >= phase).await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn idle_waits_for_every_tracker() {
        let mut life = Lifecycle::new();
        let tracker = life.tracker();
        let task = tokio::spawn(async move {
            tracker.reached(Phase::Draining).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
        });
        life.set(Phase::Draining);
        tokio::time::timeout(Duration::from_secs(5), life.idle())
            .await
            .unwrap();
        assert!(task.is_finished());
    }

    #[tokio::test]
    async fn dropped_lifecycle_never_signals() {
        let mut life = Lifecycle::new();
        let tracker = life.tracker();
        drop(life);
        let waited =
            tokio::time::timeout(Duration::from_millis(50), tracker.reached(Phase::Draining)).await;
        assert!(waited.is_err());
    }
}
