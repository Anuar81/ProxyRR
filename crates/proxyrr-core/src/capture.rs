//! Captura de bodies en streaming (spec 0007).
//!
//! `Recorder` envuelve un body: reenvía cada frame sin tocarlo y copia sus datos hasta un límite.
//! `Tap` junta el resultado del body de request y el de response de un flujo y emite un único
//! `FlowEvent::HttpBodies` cuando terminaron los dos.

use std::fmt;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Instant;

use bytes::{Bytes, BytesMut};
use hyper::HeaderMap;
use hyper::body::{Body, Frame, SizeHint};
use tokio::sync::broadcast;

use crate::event::{CapturedBody, FlowEvent, Headers, HttpBodies};

/// Límite de captura por body por defecto.
pub const DEFAULT_MAX_BODY_CAPTURE: usize = 10 * 1024 * 1024;

/// Lado de un flujo al que pertenece un body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Request,
    Response,
}

/// Convierte headers a la forma del evento, en orden y con repetidos.
pub(crate) fn headers_vec(headers: &HeaderMap) -> Headers {
    headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect()
}

#[derive(Debug, Default)]
struct Slots {
    request: Option<CapturedBody>,
    response: Option<CapturedBody>,
    emitted: bool,
}

/// Punto de encuentro de los dos bodies de un flujo.
pub(crate) struct Tap {
    id: u64,
    started: Instant,
    limit: usize,
    events: broadcast::Sender<FlowEvent>,
    slots: Mutex<Slots>,
}

impl fmt::Debug for Tap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tap")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Tap {
    pub(crate) fn new(
        id: u64,
        started: Instant,
        limit: usize,
        events: broadcast::Sender<FlowEvent>,
    ) -> Arc<Self> {
        Arc::new(Self {
            id,
            started,
            limit,
            events,
            slots: Mutex::new(Slots::default()),
        })
    }

    /// Id del flujo.
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// Registra un body que el proxy ya tiene entero (uno editado en un breakpoint).
    pub(crate) fn record_bytes(&self, side: Side, data: &Bytes, complete: bool) {
        let take = self.limit.min(data.len());
        self.finish(
            side,
            CapturedBody {
                data: data.slice(..take),
                size: data.len() as u64,
                truncated: take < data.len(),
                complete,
            },
        );
    }

    /// Registra el final de un lado. Solo cuenta el primero; al tener los dos, emite una vez.
    fn finish(&self, side: Side, body: CapturedBody) {
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        let slot = match side {
            Side::Request => &mut slots.request,
            Side::Response => &mut slots.response,
        };
        if slot.is_none() {
            *slot = Some(body);
        }
        if slots.emitted || slots.request.is_none() || slots.response.is_none() {
            return;
        }
        slots.emitted = true;
        let (Some(request), Some(response)) = (slots.request.take(), slots.response.take()) else {
            return;
        };
        drop(slots);
        // Sin suscriptores `send` falla; no es un error del proxy.
        let _ = self.events.send(FlowEvent::HttpBodies(HttpBodies {
            id: self.id,
            request,
            response,
            duration: self.started.elapsed(),
        }));
    }
}

/// Estado de captura de un body en curso.
#[derive(Debug)]
struct Capture {
    tap: Arc<Tap>,
    side: Side,
    buf: BytesMut,
    size: u64,
    truncated: bool,
}

/// Body que reenvía `inner` tal cual y copia sus datos para el `Tap`.
#[derive(Debug)]
pub(crate) struct Recorder<B: Body> {
    inner: B,
    capture: Option<Capture>,
}

impl<B: Body> Recorder<B> {
    pub(crate) fn new(inner: B, tap: Arc<Tap>, side: Side) -> Self {
        Self {
            inner,
            capture: Some(Capture {
                tap,
                side,
                buf: BytesMut::new(),
                size: 0,
                truncated: false,
            }),
        }
    }

    /// Reenvía `inner` sin capturar (su body ya se registró con [`Tap::record_bytes`]).
    pub(crate) fn plain(inner: B) -> Self {
        Self {
            inner,
            capture: None,
        }
    }

    fn record(&mut self, data: &Bytes) {
        let Some(capture) = self.capture.as_mut() else {
            return;
        };
        capture.size += data.len() as u64;
        let room = capture.tap.limit.saturating_sub(capture.buf.len());
        let take = room.min(data.len());
        capture.buf.extend_from_slice(&data[..take]);
        if take < data.len() {
            capture.truncated = true;
        }
    }

    fn finish(&mut self, complete: bool) {
        if let Some(capture) = self.capture.take() {
            capture.tap.finish(
                capture.side,
                CapturedBody {
                    data: capture.buf.freeze(),
                    size: capture.size,
                    truncated: capture.truncated,
                    complete,
                },
            );
        }
    }
}

impl<B> Body for Recorder<B>
where
    B: Body<Data = Bytes> + Unpin,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, B::Error>>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.inner).poll_frame(cx);
        match &poll {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.record(data);
                }
                // Con tamaño exacto hyper puede no volver a llamar después del último frame.
                if this.inner.is_end_stream() {
                    this.finish(true);
                }
            }
            Poll::Ready(None) => this.finish(true),
            Poll::Ready(Some(Err(_))) => this.finish(false),
            Poll::Pending => {}
        }
        poll
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl<B: Body> Drop for Recorder<B> {
    fn drop(&mut self) {
        // Destruido sin ver el final: completo solo si el body ya no tenía nada más (p. ej. uno
        // vacío que nadie leyó); si no, fue un corte del cliente u origen.
        let complete = self.inner.is_end_stream();
        self.finish(complete);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::convert::Infallible;

    use http_body_util::{BodyExt, Empty, Full};

    use super::*;

    /// Body en varios frames, sin tamaño conocido (como uno chunked).
    struct Chunks(VecDeque<Bytes>);

    impl Body for Chunks {
        type Data = Bytes;
        type Error = Infallible;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
            Poll::Ready(self.get_mut().0.pop_front().map(|b| Ok(Frame::data(b))))
        }
    }

    fn chunks(parts: &[&'static str]) -> Chunks {
        Chunks(
            parts
                .iter()
                .map(|p| Bytes::from_static(p.as_bytes()))
                .collect(),
        )
    }

    fn tap(limit: usize) -> (Arc<Tap>, broadcast::Receiver<FlowEvent>) {
        let (tx, rx) = broadcast::channel(8);
        (Tap::new(7, Instant::now(), limit, tx), rx)
    }

    fn bodies(rx: &mut broadcast::Receiver<FlowEvent>) -> HttpBodies {
        match rx.try_recv().expect("debió emitirse HttpBodies") {
            FlowEvent::HttpBodies(bodies) => bodies,
            other => panic!("evento inesperado: {other:?}"),
        }
    }

    #[tokio::test]
    async fn forwards_frames_unchanged_and_captures() {
        let (tap, mut rx) = tap(1024);
        let request = Recorder::new(Empty::<Bytes>::new(), Arc::clone(&tap), Side::Request);
        let response = Recorder::new(chunks(&["hola", " ", "mundo"]), tap, Side::Response);
        drop(request); // un GET sin body: nadie lo lee
        let forwarded = response.collect().await.unwrap().to_bytes();
        assert_eq!(forwarded, "hola mundo");
        let got = bodies(&mut rx);
        assert_eq!(got.id, 7);
        assert_eq!(got.response.data, "hola mundo");
        assert_eq!(got.response.size, 10);
        assert!(got.response.complete);
        assert!(!got.response.truncated);
        assert!(
            got.request.complete,
            "body vacío sin leer cuenta como completo"
        );
        assert_eq!(got.request.size, 0);
    }

    #[tokio::test]
    async fn truncates_at_limit_but_forwards_everything() {
        let (tap, mut rx) = tap(4);
        let request = Recorder::new(
            Full::new(Bytes::from("0123456789")),
            Arc::clone(&tap),
            Side::Request,
        );
        let forwarded = request.collect().await.unwrap().to_bytes();
        assert_eq!(forwarded, "0123456789");
        drop(Recorder::new(Empty::<Bytes>::new(), tap, Side::Response));
        let got = bodies(&mut rx);
        assert_eq!(got.request.data, "0123");
        assert_eq!(got.request.size, 10);
        assert!(got.request.truncated);
        assert!(got.request.complete);
    }

    #[tokio::test]
    async fn exact_size_body_finishes_without_extra_poll() {
        let (tap, mut rx) = tap(1024);
        let mut response = Recorder::new(
            Full::new(Bytes::from("abc")),
            Arc::clone(&tap),
            Side::Response,
        );
        // Un solo frame y nada más, como hace hyper con un body de tamaño exacto.
        let frame = response.frame().await.unwrap().unwrap();
        assert_eq!(frame.into_data().unwrap(), "abc");
        drop(Recorder::new(Empty::<Bytes>::new(), tap, Side::Request));
        assert!(bodies(&mut rx).response.complete);
        drop(response);
        assert!(rx.try_recv().is_err(), "debe emitirse una sola vez");
    }

    #[tokio::test]
    async fn dropped_mid_body_is_incomplete() {
        let (tap, mut rx) = tap(1024);
        let mut response = Recorder::new(
            chunks(&["parte1", "parte2"]),
            Arc::clone(&tap),
            Side::Response,
        );
        response.frame().await.unwrap().unwrap();
        drop(response); // el cliente cortó
        drop(Recorder::new(Empty::<Bytes>::new(), tap, Side::Request));
        let got = bodies(&mut rx);
        assert_eq!(got.response.data, "parte1");
        assert!(!got.response.complete);
    }

    #[test]
    fn emits_only_when_both_sides_finish() {
        let (tap, mut rx) = tap(16);
        drop(Recorder::new(
            Empty::<Bytes>::new(),
            Arc::clone(&tap),
            Side::Request,
        ));
        assert!(rx.try_recv().is_err());
        drop(Recorder::new(Empty::<Bytes>::new(), tap, Side::Response));
        bodies(&mut rx);
    }

    #[test]
    fn headers_keep_order_and_duplicates() {
        let mut map = HeaderMap::new();
        map.append("set-cookie", "a=1".parse().unwrap());
        map.append("x-one", "1".parse().unwrap());
        map.append("set-cookie", "b=2".parse().unwrap());
        let list = headers_vec(&map);
        let cookies: Vec<_> = list.iter().filter(|(n, _)| n == "set-cookie").collect();
        assert_eq!(cookies.len(), 2);
        assert_eq!(list.len(), 3);
    }
}
