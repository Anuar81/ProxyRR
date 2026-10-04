//! Export HAR 1.2 (<http://www.softwareishard.com/blog/har-12-spec/>) de los flujos del store.
//!
//! Los túneles no son requests HTTP: no entran. Los bodies se exportan decodificados (`content.text`
//! es el contenido, no los bytes comprimidos) y en base64 si no son UTF-8.

use std::time::{Duration, SystemTime};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use proxyrr_core::{CapturedBody, Headers};
use proxyrr_store::{Decoded, HttpRecord, StoredFlow, decode_body};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Documento HAR con todos los flujos HTTP de `flows`.
#[must_use]
pub fn to_har(flows: &[StoredFlow]) -> Value {
    let entries: Vec<Value> = flows
        .iter()
        .filter_map(|flow| match flow {
            StoredFlow::Http(record) => Some(entry(record)),
            StoredFlow::Tunnel(_) => None,
        })
        .collect();
    json!({
        "log": {
            "version": "1.2",
            "creator": { "name": "ProxyRR", "version": env!("CARGO_PKG_VERSION") },
            "pages": [],
            "entries": entries,
        }
    })
}

fn millis(d: Duration) -> f64 {
    // Precisión de microsegundos; `as` está bien: una duración de flujo no se acerca a 2^52 µs.
    #[allow(clippy::cast_precision_loss)]
    let micros = d.as_micros() as f64;
    micros / 1000.0
}

fn timestamp(t: SystemTime) -> String {
    OffsetDateTime::from(t)
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

fn entry(record: &HttpRecord) -> Value {
    let head = &record.head;
    let bodies = record.bodies.as_ref();
    let wait = head.elapsed;
    let total = bodies.map_or(wait, |b| b.duration.max(wait));
    let mut entry = json!({
        "startedDateTime": timestamp(record.started),
        "time": millis(total),
        "request": request(record),
        "response": response(record),
        "cache": {},
        "timings": {
            "send": 0,
            "wait": millis(wait),
            "receive": millis(total.saturating_sub(wait)),
        },
    });
    if let Some(error) = &head.error {
        entry["comment"] = Value::String(error.clone());
    }
    entry
}

fn request(record: &HttpRecord) -> Value {
    let head = &record.head;
    let mut request = json!({
        "method": head.method,
        "url": head.url,
        "httpVersion": "HTTP/1.1",
        "cookies": [],
        "headers": headers(&head.request_headers),
        "queryString": query_string(&head.url),
        "headersSize": -1,
        "bodySize": record.bodies.as_ref().map_or(-1, |b| size(&b.request)),
    });
    if let Some(body) = record.bodies.as_ref().map(|b| &b.request)
        && !body.data.is_empty()
    {
        let mime =
            find(&head.request_headers, "content-type").unwrap_or("application/octet-stream");
        let (text, encoding) = text_of(&body.data);
        let mut post = json!({ "mimeType": mime, "text": text });
        if let Some(encoding) = encoding {
            post["encoding"] = Value::String(encoding.to_owned());
        }
        request["postData"] = post;
    }
    request
}

fn response(record: &HttpRecord) -> Value {
    let head = &record.head;
    let headers_list = &head.response_headers;
    let mime = find(headers_list, "content-type").unwrap_or("");
    let mut content = json!({ "size": 0, "mimeType": mime });
    if let Some(body) = record.bodies.as_ref().map(|b| &b.response) {
        let raw = &body.data;
        let decoded = match decode_body(find(headers_list, "content-encoding"), raw) {
            Decoded::Decoded { data, .. } => data,
            Decoded::Identity | Decoded::Unsupported(_) => raw.to_vec(),
        };
        content["size"] = json!(decoded.len());
        if decoded.len() > raw.len() {
            content["compression"] = json!(decoded.len() - raw.len());
        }
        if !decoded.is_empty() {
            let (text, encoding) = text_of(&decoded);
            content["text"] = Value::String(text);
            if let Some(encoding) = encoding {
                content["encoding"] = Value::String(encoding.to_owned());
            }
        }
        if body.truncated || !body.complete {
            content["comment"] = Value::String(
                "ProxyRR guardó el body incompleto (truncado por el límite o cortado)".to_owned(),
            );
        }
    }
    json!({
        "status": head.status,
        "statusText": hyper_reason(head.status),
        "httpVersion": "HTTP/1.1",
        "cookies": [],
        "headers": headers(headers_list),
        "content": content,
        "redirectURL": find(headers_list, "location").unwrap_or(""),
        "headersSize": -1,
        "bodySize": record.bodies.as_ref().map_or(-1, |b| size(&b.response)),
    })
}

fn size(body: &CapturedBody) -> i64 {
    i64::try_from(body.size).unwrap_or(i64::MAX)
}

fn headers(list: &Headers) -> Vec<Value> {
    list.iter()
        .map(|(name, value)| json!({ "name": name, "value": value }))
        .collect()
}

fn find<'a>(list: &'a Headers, name: &str) -> Option<&'a str> {
    list.iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// `(texto, encoding)`: UTF-8 tal cual, o base64 si no lo es.
fn text_of(data: &[u8]) -> (String, Option<&'static str>) {
    match std::str::from_utf8(data) {
        Ok(text) => (text.to_owned(), None),
        Err(_) => (STANDARD.encode(data), Some("base64")),
    }
}

/// Pares de la query, sin decodificar (`%20` queda como viene; HAR no exige decodificarlos).
fn query_string(url: &str) -> Vec<Value> {
    let Some((_, query)) = url.split_once('?') else {
        return Vec::new();
    };
    let query = query.split('#').next().unwrap_or_default();
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            json!({ "name": name, "value": value })
        })
        .collect()
}

fn hyper_reason(status: u16) -> &'static str {
    axum::http::StatusCode::from_u16(status)
        .ok()
        .and_then(|s| s.canonical_reason())
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use proxyrr_core::{HttpBodies, HttpFlow, TunnelFlow};

    use super::*;

    fn record(response: &'static [u8], encoding: Option<&str>) -> StoredFlow {
        let mut response_headers = vec![("content-type".to_owned(), "application/json".to_owned())];
        if let Some(encoding) = encoding {
            response_headers.push(("content-encoding".to_owned(), encoding.to_owned()));
        }
        StoredFlow::Http(Box::new(HttpRecord {
            head: HttpFlow {
                id: 1,
                method: "POST".into(),
                url: "https://example.com/api?a=1&b=dos".into(),
                request_headers: vec![("content-type".into(), "text/plain".into())],
                status: 201,
                response_headers,
                rules: Vec::new(),
                error: None,
                elapsed: Duration::from_millis(40),
                content_length: None,
            },
            bodies: Some(HttpBodies {
                id: 1,
                request: CapturedBody {
                    data: Bytes::from_static(b"hola"),
                    size: 4,
                    truncated: false,
                    complete: true,
                },
                response: CapturedBody {
                    data: Bytes::from_static(response),
                    size: response.len() as u64,
                    truncated: false,
                    complete: true,
                },
                duration: Duration::from_millis(100),
            }),
            started: SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000),
        }))
    }

    #[test]
    fn exports_http_flows_and_skips_tunnels() {
        let tunnel = StoredFlow::Tunnel(TunnelFlow {
            id: 2,
            authority: "example.com:443".into(),
            status: 200,
            intercepted: false,
            error: None,
            elapsed: Duration::from_millis(1),
        });
        let har = to_har(&[record(br#"{"ok":true}"#, None), tunnel]);
        let log = &har["log"];
        assert_eq!(log["version"], "1.2");
        let entries = log["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry["startedDateTime"], "2026-09-21T14:13:20Z");
        assert_eq!(entry["time"], 100.0);
        assert_eq!(entry["timings"]["wait"], 40.0);
        assert_eq!(entry["timings"]["receive"], 60.0);
        assert_eq!(entry["request"]["method"], "POST");
        assert_eq!(entry["request"]["postData"]["text"], "hola");
        assert_eq!(entry["request"]["queryString"][1]["value"], "dos");
        assert_eq!(entry["response"]["status"], 201);
        assert_eq!(entry["response"]["statusText"], "Created");
        assert_eq!(entry["response"]["content"]["text"], r#"{"ok":true}"#);
        assert_eq!(entry["response"]["content"]["mimeType"], "application/json");
    }

    #[test]
    fn decodes_compressed_and_base64s_binary_bodies() {
        // gzip de "hola mundo" generado con Node (zlib.gzipSync), no escrito a mano.
        const GZIP: &[u8] = &[
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0a, 0xcb, 0xc8, 0xcf, 0x49,
            0x54, 0xc8, 0x2d, 0xcd, 0x4b, 0xc9, 0x07, 0x00, 0x06, 0x42, 0xdf, 0xac, 0x0a, 0x00,
            0x00, 0x00,
        ];
        let har = to_har(&[record(GZIP, Some("gzip"))]);
        let content = &har["log"]["entries"][0]["response"]["content"];
        assert_eq!(content["text"], "hola mundo");
        assert_eq!(content["size"], 10);

        let har = to_har(&[record(&[0xff, 0x00, 0xfe], None)]);
        let content = &har["log"]["entries"][0]["response"]["content"];
        assert_eq!(content["encoding"], "base64");
        assert_eq!(content["text"], "/wD+");
    }
}
