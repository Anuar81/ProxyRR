//! Breakpoints vistos desde afuera del motor (TD-011): el mismo modelo lo usan la API HTTP y la
//! app de escritorio, así un flujo en pausa se ve y se edita igual por las dos vías.

use bytes::Bytes;
use proxyrr_core::{Headers, Paused, Stage, Verdict};
use proxyrr_rules::{Breakpoints, PausedFlow};
use proxyrr_store::{Decoded, decode_body};
use serde::{Deserialize, Serialize};

/// Primer header `name` (sin distinguir mayúsculas).
#[must_use]
pub fn header<'a>(headers: &'a Headers, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Body como texto editable, decodificado si venía comprimido. `None` si es binario. El `bool`
/// dice si se decodificó (y entonces su `Content-Encoding` ya no vale).
#[must_use]
pub fn editable_text(headers: &Headers, data: &[u8]) -> (Option<String>, bool) {
    let (bytes, decoded): (Vec<u8>, bool) =
        match decode_body(header(headers, "content-encoding"), data) {
            Decoded::Identity => (data.to_vec(), false),
            Decoded::Decoded {
                data,
                complete: true,
            } => (data, true),
            Decoded::Decoded { .. } | Decoded::Unsupported(_) => return (None, false),
        };
    match String::from_utf8(bytes) {
        Ok(text) if !text.chars().any(|c| c.is_control() && !c.is_whitespace()) => {
            (Some(text), decoded)
        }
        _ => (None, false),
    }
}

/// Lado en pausa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StageDto {
    /// Request.
    Request,
    /// Respuesta.
    Response,
}

/// Flujo en pausa. Si el body venía comprimido se muestra decodificado y sin `Content-Encoding`:
/// si se edita, sale sin comprimir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PausedDto {
    /// Clave para resolverlo.
    pub key: u64,
    /// Lado.
    pub stage: StageDto,
    /// Id del flujo.
    pub id: u64,
    /// Método.
    pub method: String,
    /// URL.
    pub url: String,
    /// Status (respuesta).
    pub status: Option<u16>,
    /// Headers.
    pub headers: Headers,
    /// Body editable; `None` si es binario (se manda el original).
    pub body: Option<String>,
    /// Tamaño del body original.
    pub size: usize,
    /// Aviso para la vista.
    pub note: Option<String>,
}

impl From<&PausedFlow> for PausedDto {
    fn from(flow: &PausedFlow) -> Self {
        let message = &flow.message;
        let (body, decoded) = editable_text(&message.headers, &message.body);
        let headers = message
            .headers
            .iter()
            .filter(|(n, _)| !(decoded && n.eq_ignore_ascii_case("content-encoding")))
            .cloned()
            .collect();
        let note = if body.is_none() && !message.body.is_empty() {
            Some(format!(
                "Body binario de {} bytes: se manda el original.",
                message.body.len()
            ))
        } else if decoded {
            header(&message.headers, "content-encoding").map(|enc| {
                format!(
                    "El body venía con {enc}: se muestra decodificado y se manda sin comprimir."
                )
            })
        } else {
            None
        };
        Self {
            key: flow.key,
            stage: match flow.stage {
                Stage::Request => StageDto::Request,
                Stage::Response => StageDto::Response,
            },
            id: message.id,
            method: message.method.clone(),
            url: message.url.clone(),
            status: message.status,
            headers,
            body,
            size: message.body.len(),
            note,
        }
    }
}

/// Mensaje editado.
#[derive(Debug, Clone, Deserialize)]
pub struct EditedDto {
    /// Método (request).
    #[serde(default)]
    pub method: Option<String>,
    /// URL (request).
    #[serde(default)]
    pub url: Option<String>,
    /// Status (respuesta).
    #[serde(default)]
    pub status: Option<u16>,
    /// Headers.
    pub headers: Headers,
    /// Body; `None`: el original (con su `Content-Encoding`).
    #[serde(default)]
    pub body: Option<String>,
}

/// Aplica una edición sobre el mensaje original.
#[must_use]
pub fn apply_edit(original: &Paused, edited: EditedDto) -> Paused {
    let mut headers = edited.headers;
    let body = if let Some(text) = edited.body {
        Bytes::from(text)
    } else {
        // Body original: vuelve con su encoding (la vista lo había ocultado al decodificar).
        if let Some(enc) = header(&original.headers, "content-encoding")
            && header(&headers, "content-encoding").is_none()
        {
            headers.push(("content-encoding".into(), enc.to_owned()));
        }
        original.body.clone()
    };
    Paused {
        id: original.id,
        method: edited.method.unwrap_or_else(|| original.method.clone()),
        url: edited.url.unwrap_or_else(|| original.url.clone()),
        status: edited.status.or(original.status),
        headers,
        body,
    }
}

/// Decisión sobre un flujo en pausa.
#[derive(Debug, Clone, Deserialize)]
pub struct ResolveDto {
    /// `execute` (con `edited`), `continue` (sin cambios) o `abort` (503).
    pub action: String,
    /// Mensaje editado, para `execute`.
    #[serde(default)]
    pub edited: Option<EditedDto>,
}

/// Mensaje cuando la clave ya no está en pausa.
pub const NOT_PAUSED: &str = "ese flujo ya no está en pausa (venció o el cliente cortó)";

/// Resuelve el flujo `key`.
///
/// # Errors
/// Acción desconocida, `execute` sin `edited`, o flujo que ya no está en pausa ([`NOT_PAUSED`]).
pub fn resolve(breakpoints: &Breakpoints, key: u64, decision: ResolveDto) -> Result<(), String> {
    let original = breakpoints
        .pending()
        .into_iter()
        .find(|p| p.key == key)
        .ok_or(NOT_PAUSED)?
        .message;
    let verdict = match decision.action.as_str() {
        "continue" => Verdict::Continue(original),
        "abort" => Verdict::Abort,
        "execute" => Verdict::Continue(apply_edit(
            &original,
            decision.edited.ok_or("falta el mensaje editado")?,
        )),
        other => return Err(format!("acción desconocida: {other}")),
    };
    if breakpoints.resolve(key, verdict) {
        Ok(())
    } else {
        Err(NOT_PAUSED.into())
    }
}
