//! Herramientas de depuración en la app (spec 0011): borradores de reglas desde un flujo, breakpoints
//! para la vista, Compose y Copy as cURL. Funciones puras, probadas sin ventana.

use std::fmt::Write as _;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use proxyrr_core::{Headers, Paused, Replay, Stage};
use proxyrr_rules::{Action, MapLocal, MapRemote, PausedFlow, Rule};
use proxyrr_store::{Decoded, HttpRecord, decode_body};
use serde::{Deserialize, Serialize};

/// Headers que no tiene sentido copiar a un mock ni a un request nuevo: los recalcula el proxy o son
/// de la conexión.
const CONNECTION_HEADERS: [&str; 9] = [
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authorization",
    "te",
    "trailer",
    "upgrade",
];

fn is_connection_header(name: &str) -> bool {
    CONNECTION_HEADERS
        .iter()
        .any(|h| h.eq_ignore_ascii_case(name))
}

/// Header que se copia a un mock o a Compose. Si el body se decodificó, su `Content-Encoding` ya no vale.
fn keep_header(name: &str, decoded: bool) -> bool {
    !(is_connection_header(name) || decoded && name.eq_ignore_ascii_case("content-encoding"))
}

fn header<'a>(headers: &'a Headers, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Body como texto editable, decodificado si venía comprimido. `None` si es binario.
fn editable_text(headers: &Headers, data: &[u8]) -> (Option<String>, bool) {
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

/// URL sin query.
fn base_url(url: &str) -> &str {
    url.split_once('?').map_or(url, |(base, _)| base)
}

/// `esquema://host[:puerto]` de una URL.
fn origin(url: &str) -> &str {
    let after = url.find("://").map_or(0, |i| i + 3);
    let end = url[after..]
        .find(['/', '?'])
        .map_or(url.len(), |i| after + i);
    &url[..end]
}

/// Path de una URL, para el nombre de la regla.
fn path(url: &str) -> &str {
    let rest = &base_url(url)[origin(url).len()..];
    if rest.is_empty() { "/" } else { rest }
}

/// Tipo de regla a crear desde un flujo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftKind {
    /// Map Local con la respuesta del flujo.
    MapLocal,
    /// Map Remote del host del flujo.
    MapRemote,
    /// Breakpoint de request y respuesta.
    Breakpoint,
    /// Block del host.
    Block,
    /// No Caching del host.
    NoCache,
}

/// Regla nueva (id 0) completada con los datos de un flujo, para que la UI la edite antes de guardar.
#[must_use]
pub fn draft_rule(record: &HttpRecord, kind: DraftKind) -> Rule {
    let head = &record.head;
    let host_pattern = format!("{}/*", origin(&head.url));
    let (name, method, url, action) = match kind {
        DraftKind::MapLocal => {
            let response = record.bodies.as_ref().map(|b| &b.response);
            let data = response.map_or(&[][..], |r| &r.data[..]);
            let (text, decoded) = editable_text(&head.response_headers, data);
            let headers = head
                .response_headers
                .iter()
                .filter(|(n, _)| keep_header(n, decoded))
                .cloned()
                .collect();
            (
                format!("Map Local {} {}", head.method, path(&head.url)),
                Some(head.method.clone()),
                base_url(&head.url).to_owned(),
                Action::MapLocal(MapLocal {
                    status: head.status,
                    headers,
                    body: text.unwrap_or_default(),
                    file: None,
                }),
            )
        }
        DraftKind::MapRemote => (
            format!("Map Remote {}", origin(&head.url)),
            None,
            host_pattern,
            Action::MapRemote(MapRemote {
                scheme: Some("http".into()),
                host: Some("localhost".into()),
                port: Some(3000),
                ..MapRemote::default()
            }),
        ),
        DraftKind::Breakpoint => (
            format!("Breakpoint {} {}", head.method, path(&head.url)),
            Some(head.method.clone()),
            base_url(&head.url).to_owned(),
            Action::Breakpoint {
                request: true,
                response: true,
            },
        ),
        DraftKind::Block => (
            format!("Block {}", origin(&head.url)),
            None,
            host_pattern,
            Action::Block { status: 403 },
        ),
        DraftKind::NoCache => (
            format!("No Caching {}", origin(&head.url)),
            None,
            host_pattern,
            Action::NoCache,
        ),
    };
    Rule {
        id: 0,
        name,
        enabled: true,
        method,
        url,
        regex: false,
        action,
    }
}

/// Lado en pausa, para la vista.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StageDto {
    /// Request.
    Request,
    /// Respuesta.
    Response,
}

/// Flujo en pausa para la vista. Si el body venía comprimido se muestra decodificado y sin
/// `Content-Encoding`: si se edita, sale sin comprimir.
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

/// Mensaje editado en la vista.
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

/// Aplica la edición de la vista sobre el mensaje original.
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

/// Request para Compose / Editar y repetir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposeDto {
    /// Método.
    pub method: String,
    /// URL.
    pub url: String,
    /// Headers.
    pub headers: Headers,
    /// Body en texto.
    pub body: String,
    /// Aviso (body binario que no se puede editar).
    #[serde(default)]
    pub note: Option<String>,
}

impl ComposeDto {
    /// Desde un request guardado.
    #[must_use]
    pub fn from_replay(replay: &Replay) -> Self {
        let (text, decoded) = editable_text(&replay.headers, &replay.body);
        let headers = replay
            .headers
            .iter()
            .filter(|(n, _)| keep_header(n, decoded))
            .cloned()
            .collect();
        Self {
            method: replay.method.clone(),
            url: replay.url.clone(),
            headers,
            note: (text.is_none() && !replay.body.is_empty()).then(|| {
                format!(
                    "El body original es binario ({} bytes) y no se puede editar acá.",
                    replay.body.len()
                )
            }),
            body: text.unwrap_or_default(),
        }
    }

    /// A un request para el proxy.
    #[must_use]
    pub fn into_replay(self) -> Replay {
        Replay {
            method: self.method,
            url: self.url,
            headers: self.headers,
            body: Bytes::from(self.body),
        }
    }
}

/// Comillas simples de shell POSIX: `'` se escribe `'\''`.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Shell para el que se arma el comando `curl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurlShell {
    /// bash / zsh / Git Bash: comillas simples POSIX y `\` para seguir la línea.
    Posix,
    /// PowerShell (5 y 7): `curl.exe` (no el alias de `Invoke-WebRequest`), comillas simples
    /// con `''` como escape y `` ` `` para seguir la línea.
    PowerShell,
}

impl CurlShell {
    fn quote(self, text: &str) -> String {
        match self {
            Self::Posix => quote(text),
            Self::PowerShell => format!("'{}'", text.replace('\'', "''")),
        }
    }

    fn program(self) -> &'static str {
        match self {
            Self::Posix => "curl",
            Self::PowerShell => "curl.exe",
        }
    }

    fn next_line(self) -> &'static str {
        match self {
            Self::Posix => " \\\n  ",
            Self::PowerShell => " `\n  ",
        }
    }
}

/// Comando `curl` (bash/zsh) que reproduce un request.
#[cfg(test)]
#[must_use]
pub fn curl_command(request: &Replay) -> String {
    curl_command_for(request, CurlShell::Posix)
}

/// Comando `curl` que reproduce un request, en la sintaxis de `shell`.
#[must_use]
pub fn curl_command_for(request: &Replay, shell: CurlShell) -> String {
    let q = |text: &str| shell.quote(text);
    let nl = shell.next_line();
    let mut out = String::from(shell.program());
    match request.method.to_ascii_uppercase().as_str() {
        "GET" => {}
        "HEAD" => out.push_str(" --head"),
        method => {
            let _ = write!(out, " -X {}", q(method));
        }
    }
    let _ = write!(out, " {}", q(&request.url));
    let url_host = origin(&request.url)
        .split_once("://")
        .map_or("", |(_, h)| h);
    let mut compressed = false;
    for (name, value) in &request.headers {
        if is_connection_header(name)
            || (name.eq_ignore_ascii_case("host") && value.eq_ignore_ascii_case(url_host))
        {
            continue;
        }
        if name.eq_ignore_ascii_case("accept-encoding") {
            compressed = true;
        }
        let _ = write!(out, "{nl}-H {}", q(&format!("{name}: {value}")));
    }
    if compressed {
        let _ = write!(out, "{nl}--compressed");
    }
    if !request.body.is_empty() {
        match std::str::from_utf8(&request.body) {
            Ok(text) => {
                let _ = write!(out, "{nl}--data-binary {}", q(text));
            }
            Err(_) => {
                let _ = write!(
                    out,
                    "{nl}--data-binary '@body.bin'  # body binario de {} bytes: base64 {}",
                    request.body.len(),
                    STANDARD.encode(&request.body)
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::time::{Duration, SystemTime};

    use proxyrr_core::{CapturedBody, HttpBodies, HttpFlow};

    use super::*;

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    fn record(response_headers: Headers, body: Vec<u8>) -> HttpRecord {
        HttpRecord {
            started: SystemTime::now(),
            head: HttpFlow {
                id: 3,
                method: "GET".into(),
                url: "https://api.x.com:8443/v1/users?page=2".into(),
                request_headers: vec![],
                status: 200,
                response_headers,
                rules: vec![],
                error: None,
                elapsed: Duration::from_millis(1),
                content_length: None,
            },
            bodies: Some(HttpBodies {
                id: 3,
                request: CapturedBody::default(),
                response: CapturedBody {
                    size: body.len() as u64,
                    data: Bytes::from(body),
                    truncated: false,
                    complete: true,
                },
                duration: Duration::from_millis(2),
            }),
        }
    }

    #[test]
    fn map_local_draft_uses_the_decoded_response() {
        let rec = record(
            vec![
                ("content-type".into(), "application/json".into()),
                ("content-encoding".into(), "gzip".into()),
                ("content-length".into(), "40".into()),
                ("x-trace".into(), "1".into()),
            ],
            gzip(br#"{"users":[]}"#),
        );
        let rule = draft_rule(&rec, DraftKind::MapLocal);
        assert_eq!(rule.url, "https://api.x.com:8443/v1/users", "sin query");
        assert_eq!(rule.method.as_deref(), Some("GET"));
        assert_eq!(rule.name, "Map Local GET /v1/users");
        let Action::MapLocal(local) = rule.action else {
            panic!("debía ser map_local");
        };
        assert_eq!(local.body, r#"{"users":[]}"#);
        let names: Vec<&str> = local.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["content-type", "x-trace"]);
    }

    #[test]
    fn host_drafts() {
        let rec = record(vec![], vec![]);
        let block = draft_rule(&rec, DraftKind::Block);
        assert_eq!(block.url, "https://api.x.com:8443/*");
        assert_eq!(block.method, None);
        assert_eq!(block.action, Action::Block { status: 403 });
        assert!(matches!(
            draft_rule(&rec, DraftKind::MapRemote).action,
            Action::MapRemote(_)
        ));
        assert_eq!(draft_rule(&rec, DraftKind::NoCache).action, Action::NoCache);
        assert_eq!(
            draft_rule(&rec, DraftKind::Breakpoint).action,
            Action::Breakpoint {
                request: true,
                response: true
            }
        );
    }

    fn paused(headers: Headers, body: Vec<u8>) -> PausedFlow {
        PausedFlow {
            key: 5,
            stage: Stage::Response,
            message: Paused {
                id: 9,
                method: "GET".into(),
                url: "https://x.com/".into(),
                status: Some(200),
                headers,
                body: Bytes::from(body),
            },
        }
    }

    #[test]
    fn paused_bodies_are_decoded_for_editing() {
        let flow = paused(
            vec![("content-encoding".into(), "gzip".into())],
            gzip(b"hola"),
        );
        let dto = PausedDto::from(&flow);
        assert_eq!(dto.body.as_deref(), Some("hola"));
        assert!(dto.headers.is_empty(), "sin content-encoding");
        assert!(dto.note.unwrap().contains("gzip"));

        let edited = apply_edit(
            &flow.message,
            EditedDto {
                method: None,
                url: None,
                status: Some(500),
                headers: dto.headers.clone(),
                body: Some("chau".into()),
            },
        );
        assert_eq!(edited.status, Some(500));
        assert_eq!(edited.body, "chau");
        assert!(edited.headers.is_empty(), "sale sin comprimir");

        let kept = apply_edit(
            &flow.message,
            EditedDto {
                method: None,
                url: None,
                status: None,
                headers: vec![],
                body: None,
            },
        );
        assert_eq!(kept.body, flow.message.body, "el original");
        assert_eq!(
            kept.headers,
            [("content-encoding".to_owned(), "gzip".to_owned())]
        );
    }

    #[test]
    fn binary_paused_body_is_not_editable() {
        let dto = PausedDto::from(&paused(vec![], vec![0, 1, 2, 255]));
        assert!(dto.body.is_none());
        assert!(dto.note.unwrap().contains("binario"));
    }

    #[test]
    fn compose_round_trip() {
        let replay = Replay {
            method: "POST".into(),
            url: "https://x.com/a".into(),
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("content-length".into(), "2".into()),
            ],
            body: Bytes::from_static(b"{}"),
        };
        let dto = ComposeDto::from_replay(&replay);
        assert_eq!(dto.headers.len(), 1, "sin content-length");
        assert_eq!(dto.body, "{}");
        assert!(dto.note.is_none());
        let back = dto.into_replay();
        assert_eq!(back.body, "{}");
        assert_eq!(back.method, "POST");
    }

    #[test]
    fn curl_reproduces_the_request() {
        let get = Replay {
            method: "GET".into(),
            url: "https://x.com/a?b=1".into(),
            headers: vec![
                ("host".into(), "x.com".into()),
                ("accept-encoding".into(), "gzip".into()),
                ("connection".into(), "keep-alive".into()),
            ],
            body: Bytes::new(),
        };
        assert_eq!(
            curl_command(&get),
            "curl 'https://x.com/a?b=1' \\\n  -H 'accept-encoding: gzip' \\\n  --compressed"
        );
        let post = Replay {
            method: "POST".into(),
            url: "http://x.com/".into(),
            headers: vec![
                ("host".into(), "otro.com".into()),
                ("content-length".into(), "13".into()),
            ],
            body: Bytes::from_static(b"it's {\"a\":1}"),
        };
        assert_eq!(
            curl_command(&post),
            "curl -X 'POST' 'http://x.com/' \\\n  -H 'host: otro.com' \\\n  --data-binary 'it'\\''s {\"a\":1}'"
        );
        let head = Replay {
            method: "HEAD".into(),
            body: Bytes::from_static(&[0xff, 0x00]),
            ..get
        };
        let cmd = curl_command(&head);
        assert!(cmd.starts_with("curl --head"), "{cmd}");
        assert!(cmd.contains("binario de 2 bytes"), "{cmd}");
    }

    #[test]
    fn curl_for_powershell_uses_curl_exe_and_its_quoting() {
        let post = Replay {
            method: "POST".into(),
            url: "https://x.com/login".into(),
            headers: vec![("content-type".into(), "application/json".into())],
            body: Bytes::from_static(b"{\"user\":\"o'neil\"}"),
        };
        assert_eq!(
            curl_command_for(&post, CurlShell::PowerShell),
            "curl.exe -X 'POST' 'https://x.com/login' `\n  -H 'content-type: application/json' `\n  --data-binary '{\"user\":\"o''neil\"}'"
        );
    }
}
