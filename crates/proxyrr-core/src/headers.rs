//! Headers hop-by-hop (RFC 9110 §7.6.1): valen para un solo salto y un proxy no los reenvía.

use hyper::header::{CONNECTION, HeaderMap, HeaderName};

/// Headers hop-by-hop fijos. Además se quitan los que nombre el header `Connection`.
const HOP_BY_HOP: [&str; 9] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Quita de `headers` los hop-by-hop, incluidos los listados en `Connection`.
pub fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let listed: Vec<HeaderName> = headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|token| HeaderName::from_bytes(token.trim().as_bytes()).ok())
        .collect();
    for name in listed {
        headers.remove(name);
    }
    for name in HOP_BY_HOP {
        headers.remove(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::HeaderValue;

    fn map(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(*name, HeaderValue::from_static(value));
        }
        headers
    }

    #[test]
    fn removes_fixed_hop_by_hop_headers() {
        let mut headers = map(&[
            ("connection", "keep-alive"),
            ("keep-alive", "timeout=5"),
            ("proxy-connection", "keep-alive"),
            ("proxy-authenticate", "Basic"),
            ("proxy-authorization", "Basic abc"),
            ("te", "trailers"),
            ("trailer", "x-checksum"),
            ("transfer-encoding", "chunked"),
            ("upgrade", "websocket"),
            ("host", "example.com"),
            ("content-type", "text/plain"),
        ]);
        strip_hop_by_hop(&mut headers);
        let left: Vec<&str> = headers.keys().map(HeaderName::as_str).collect();
        assert_eq!(left, ["host", "content-type"]);
    }

    #[test]
    fn removes_headers_listed_in_connection() {
        let mut headers = map(&[
            ("connection", "close, X-Secret"),
            ("connection", " x-other "),
            ("x-secret", "1"),
            ("x-other", "2"),
            ("x-keep", "3"),
        ]);
        strip_hop_by_hop(&mut headers);
        assert!(headers.get("x-secret").is_none());
        assert!(headers.get("x-other").is_none());
        assert_eq!(headers.get("x-keep").unwrap(), "3");
    }

    #[test]
    fn ignores_invalid_connection_tokens() {
        let mut headers = map(&[("connection", "close, , bad header"), ("x-keep", "1")]);
        strip_hop_by_hop(&mut headers);
        assert_eq!(headers.len(), 1);
    }
}
