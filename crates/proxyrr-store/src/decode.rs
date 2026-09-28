//! Decodificar bodies para mostrarlos (`Content-Encoding` gzip, deflate, br). El store guarda los bytes
//! tal como viajaron; esto solo arma una vista legible.

use std::io::Read;

/// Tope del body decodificado: un body chico que se infla a gigas (bomba de compresión) se corta acá.
pub const MAX_DECODED: usize = 64 * 1024 * 1024;

/// Resultado de decodificar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    /// Sin `Content-Encoding` (o `identity`): los bytes son el contenido.
    Identity,
    /// Decodificado. `complete` es `false` si se cortó en [`MAX_DECODED`] o el stream estaba truncado.
    Decoded {
        /// Contenido.
        data: Vec<u8>,
        /// `true` si llegó entero.
        complete: bool,
    },
    /// Encoding desconocido (p. ej. `zstd`) o datos inválidos: se muestran los bytes crudos.
    Unsupported(String),
}

/// Decodifica `data` según el valor del header `Content-Encoding`. Encodings encadenados
/// (`gzip, br`) se deshacen en orden inverso.
#[must_use]
pub fn decode_body(content_encoding: Option<&str>, data: &[u8]) -> Decoded {
    let codings: Vec<String> = content_encoding
        .unwrap_or_default()
        .split(',')
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| !c.is_empty() && c != "identity")
        .collect();
    if codings.is_empty() {
        return Decoded::Identity;
    }
    let mut current = data.to_vec();
    let mut complete = true;
    for coding in codings.iter().rev() {
        let (out, whole) = match coding.as_str() {
            "gzip" | "x-gzip" => {
                read_limited(flate2::read::MultiGzDecoder::new(current.as_slice()))
            }
            // `deflate` en HTTP es zlib (RFC 9110), aunque hay servidores que mandan deflate crudo.
            "deflate" => match read_limited(flate2::read::ZlibDecoder::new(current.as_slice())) {
                Ok(ok) => Ok(ok),
                Err(_) => read_limited(flate2::read::DeflateDecoder::new(current.as_slice())),
            },
            "br" => read_limited(brotli_decompressor::Decompressor::new(
                current.as_slice(),
                8192,
            )),
            other => return Decoded::Unsupported(format!("encoding no soportado: {other}")),
        }
        .map_or_else(|e| (Err(e), false), |(out, whole)| (Ok(out), whole));
        match out {
            Ok(out) => {
                current = out;
                complete &= whole;
            }
            Err(e) => return Decoded::Unsupported(format!("no se pudo decodificar {coding}: {e}")),
        }
    }
    Decoded::Decoded {
        data: current,
        complete,
    }
}

/// Lee hasta [`MAX_DECODED`]. Un stream cortado (body truncado por el límite de captura) devuelve lo
/// que se alcanzó a leer como incompleto, no un error.
fn read_limited(reader: impl Read) -> std::io::Result<(Vec<u8>, bool)> {
    let mut out = Vec::new();
    let mut limited = reader.take(MAX_DECODED as u64 + 1);
    match limited.read_to_end(&mut out) {
        Ok(_) if out.len() > MAX_DECODED => {
            out.truncate(MAX_DECODED);
            Ok((out, false))
        }
        Ok(_) => Ok((out, true)),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && !out.is_empty() => {
            Ok((out, false))
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    fn raw_deflate(data: &[u8]) -> Vec<u8> {
        let mut enc =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    /// "hola" comprimido con brotli (generado con `zlib.brotliCompressSync` de Node, que usa la
    /// librería de referencia; el crate que usamos solo decodifica).
    const BR_HOLA: &[u8] = &[0x8b, 0x01, 0x80, 0x68, 0x6f, 0x6c, 0x61, 0x03];

    fn decoded(result: Decoded) -> (Vec<u8>, bool) {
        match result {
            Decoded::Decoded { data, complete } => (data, complete),
            other => panic!("esperaba decodificado: {other:?}"),
        }
    }

    #[test]
    fn identity_and_missing_encoding() {
        assert_eq!(decode_body(None, b"x"), Decoded::Identity);
        assert_eq!(decode_body(Some("identity"), b"x"), Decoded::Identity);
    }

    #[test]
    fn gzip_deflate_and_brotli() {
        assert_eq!(
            decoded(decode_body(Some("gzip"), &gzip(b"hola"))),
            (b"hola".to_vec(), true)
        );
        assert_eq!(
            decoded(decode_body(Some("GZIP"), &gzip(b"hola"))).0,
            b"hola"
        );
        assert_eq!(
            decoded(decode_body(Some("deflate"), &zlib(b"hola"))).0,
            b"hola"
        );
        assert_eq!(
            decoded(decode_body(Some("deflate"), &raw_deflate(b"hola"))).0,
            b"hola",
            "deflate crudo"
        );
        assert_eq!(
            decoded(decode_body(Some("br"), BR_HOLA)),
            (b"hola".to_vec(), true)
        );
    }

    #[test]
    fn chained_encodings_are_undone_in_reverse() {
        let twice = gzip(&zlib(b"doble"));
        assert_eq!(
            decoded(decode_body(Some("deflate, gzip"), &twice)).0,
            b"doble"
        );
    }

    #[test]
    fn truncated_stream_is_partial_not_an_error() {
        let big: Vec<u8> = (0..200_000u32).flat_map(u32::to_le_bytes).collect();
        let compressed = gzip(&big);
        let (data, complete) = decoded(decode_body(
            Some("gzip"),
            &compressed[..compressed.len() / 2],
        ));
        assert!(!complete);
        assert!(!data.is_empty() && big.starts_with(&data));
    }

    #[test]
    fn bombs_are_capped() {
        let bomb = gzip(&vec![0u8; MAX_DECODED + 10]);
        let (data, complete) = decoded(decode_body(Some("gzip"), &bomb));
        assert_eq!(data.len(), MAX_DECODED);
        assert!(!complete);
    }

    #[test]
    fn unknown_or_garbage_falls_back_to_raw() {
        assert!(matches!(
            decode_body(Some("zstd"), b"x"),
            Decoded::Unsupported(_)
        ));
        assert!(matches!(
            decode_body(Some("gzip"), b"no es gzip"),
            Decoded::Unsupported(_)
        ));
    }
}
