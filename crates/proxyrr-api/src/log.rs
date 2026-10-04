//! Reenvío de logs del motor a las UIs (TD-006): los `warn`/`error` de `proxyrr*` se publican como
//! [`Notice::Log`] por el mismo canal que los flujos.

use std::fmt::{self, Write as _};

use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use crate::engine::Notice;

/// Nivel de un aviso de log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// Advertencia.
    Warn,
    /// Error.
    Error,
}

/// Capa de `tracing` que manda los avisos del motor a las UIs. Se arma con `Engine::log_layer`.
#[derive(Debug, Clone)]
pub struct LogLayer {
    notices: broadcast::Sender<Notice>,
}

impl LogLayer {
    pub(crate) fn new(notices: broadcast::Sender<Notice>) -> Self {
        Self { notices }
    }
}

impl<S: Subscriber> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let level = match *meta.level() {
            Level::ERROR => LogLevel::Error,
            Level::WARN => LogLevel::Warn,
            _ => return,
        };
        if !meta.target().starts_with("proxyrr") {
            return;
        }
        let mut text = MessageText::default();
        event.record(&mut text);
        let _ = self.notices.send(Notice::Log {
            level,
            message: text.finish(),
        });
    }
}

/// Junta `message` y los demás campos como `clave=valor`.
#[derive(Debug, Default)]
struct MessageText {
    message: String,
    fields: String,
}

impl MessageText {
    fn finish(self) -> String {
        if self.fields.is_empty() {
            self.message
        } else {
            format!("{} ({})", self.message, self.fields.trim_start())
        }
    }
}

impl Visit for MessageText {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.fields, " {}={value}", field.name());
        }
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    #[test]
    fn forwards_only_proxyrr_warnings_and_errors() {
        let (tx, mut rx) = broadcast::channel(8);
        let subscriber = tracing_subscriber::registry().with(LogLayer::new(tx));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "proxyrr_core", "info no pasa");
            tracing::warn!(target: "otra_lib", "otro crate no pasa");
            tracing::warn!(target: "proxyrr_core", count = 3, "accept falló");
        });
        let Ok(Notice::Log { level, message }) = rx.try_recv() else {
            panic!("se esperaba un aviso de log");
        };
        assert_eq!(level, LogLevel::Warn);
        assert_eq!(message, "accept falló (count=3)");
        assert!(rx.try_recv().is_err());
    }
}
