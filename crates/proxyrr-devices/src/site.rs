//! Página `http://proxyrr.cert/` (y `/cert` directo al proxy): descargas de la CA e instrucciones
//! según el dispositivo (spec 0006, CA 11 y 13).

use bytes::Bytes;
use proxyrr_core::{LocalRequest, LocalResponse, LocalSite};

use crate::files::{CaFiles, xml_escape};

/// Plataforma deducida del `User-Agent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// iPhone / iPad / iPod.
    Ios,
    /// Android.
    Android,
    /// Cualquier otra cosa.
    Desktop,
}

impl Platform {
    /// Deduce la plataforma. Un iPad en modo escritorio se anuncia como Mac: por eso la página
    /// muestra siempre las otras plataformas debajo.
    #[must_use]
    pub fn from_user_agent(user_agent: Option<&str>) -> Self {
        let ua = user_agent.unwrap_or_default();
        if ["iPhone", "iPad", "iPod"].iter().any(|d| ua.contains(d)) {
            Self::Ios
        } else if ua.contains("Android") {
            Self::Android
        } else {
            Self::Desktop
        }
    }
}

/// Sitio local que sirve la CA.
#[derive(Debug, Clone)]
pub struct CertSite {
    files: CaFiles,
    mobileconfig: Bytes,
    pem: Bytes,
    der: Bytes,
}

impl CertSite {
    /// Sitio para `files`.
    #[must_use]
    pub fn new(files: CaFiles) -> Self {
        Self {
            mobileconfig: Bytes::from(files.mobileconfig()),
            pem: Bytes::from(files.pem.clone()),
            der: Bytes::from(files.der.clone()),
            files,
        }
    }

    fn page(&self, base: &str, platform: Platform) -> String {
        let ios = format!(
            r#"<section><h2>iPhone / iPad</h2><ol>
<li>Tocá <a href="{base}/ca.mobileconfig">Descargar perfil</a> y aceptá.</li>
<li>Ajustes → <b>Perfil descargado</b> → Instalar.</li>
<li><b>Paso que todos olvidan:</b> Ajustes → General → Información → <b>Ajustes de confianza de certificados</b> → activá <i>{name}</i>.</li>
</ol></section>"#,
            name = xml_escape(&self.files.name)
        );
        let android = format!(
            r#"<section><h2>Android</h2><ol>
<li>Tocá <a href="{base}/ca.crt">Descargar certificado</a>.</li>
<li>Ajustes → Seguridad → Cifrado y credenciales → <b>Instalar certificado</b> → Certificado de CA → elegí <code>proxyrr-ca.crt</code>.</li>
<li>Desde Android 7 las apps <b>no</b> confían en certificados de usuario: tu app necesita <code>network_security_config</code> en builds debug (lo imprime <code>proxyrr setup android-device</code>). Chrome sí confía.</li>
</ol></section>"#
        );
        let desktop = format!(
            r#"<section><h2>Windows / macOS / Linux</h2><p>En la máquina donde corre ProxyRR: <code>proxyrr ca install</code>.
En otra máquina: descargá <a href="{base}/ca.pem">ca.pem</a> o <a href="{base}/ca.crt">ca.crt</a> e instalalo como raíz de confianza.</p></section>"#
        );
        let (first, rest) = match platform {
            Platform::Ios => (ios, [android, desktop]),
            Platform::Android => (android, [ios, desktop]),
            Platform::Desktop => (desktop, [ios, android]),
        };
        format!(
            r#"<!doctype html><html lang="es"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>ProxyRR — certificado</title>
<style>body{{font-family:system-ui,sans-serif;max-width:40rem;margin:1rem auto;padding:0 1rem;line-height:1.5}}
code{{background:#8882;padding:0 .2rem}}.warn{{border-left:4px solid #c80;padding-left:.6rem}}</style></head>
<body><h1>Certificado de ProxyRR</h1>
<p><b>{name}</b><br><small>SHA-256 {sha}</small></p>
<p class="warn">Quien tenga la clave de esta CA puede leer tu HTTPS. Quitala cuando no la uses.</p>
{first}
<details><summary>Otros dispositivos</summary>{rest}</details>
<p><small>Descargas: <a href="{base}/ca.pem">PEM</a> · <a href="{base}/ca.crt">DER (.crt)</a> · <a href="{base}/ca.mobileconfig">perfil iOS</a></small></p>
</body></html>
"#,
            name = xml_escape(&self.files.name),
            sha = self.files.sha256,
            rest = rest.join("\n"),
        )
    }
}

impl LocalSite for CertSite {
    fn respond(&self, request: &LocalRequest<'_>) -> LocalResponse {
        let base = if request.direct {
            proxyrr_core::DIRECT_PREFIX
        } else {
            ""
        };
        let (content_type, body, file) = match request.path {
            "/" | "/index.html" => {
                let page = self.page(base, Platform::from_user_agent(request.user_agent));
                ("text/html; charset=utf-8", Bytes::from(page), None)
            }
            "/ca.pem" => (
                "application/x-pem-file",
                self.pem.clone(),
                Some("proxyrr-ca.pem"),
            ),
            "/ca.crt" => (
                "application/x-x509-ca-cert",
                self.der.clone(),
                Some("proxyrr-ca.crt"),
            ),
            // Sin `attachment`: Safari en iOS abre el perfil directamente con este tipo.
            "/ca.mobileconfig" => (
                "application/x-apple-aspen-config",
                self.mobileconfig.clone(),
                None,
            ),
            _ => {
                return LocalResponse {
                    status: 404,
                    headers: vec![("content-type".into(), "text/plain; charset=utf-8".into())],
                    body: Bytes::from_static(
                        b"ProxyRR: no existe. Proba con la pagina principal.\n",
                    ),
                };
            }
        };
        let mut headers = vec![
            ("content-type".to_owned(), content_type.to_owned()),
            ("cache-control".to_owned(), "no-store".to_owned()),
            ("x-content-type-options".to_owned(), "nosniff".to_owned()),
            (
                "content-security-policy".to_owned(),
                "default-src 'none'; style-src 'unsafe-inline'".to_owned(),
            ),
        ];
        if let Some(file) = file {
            headers.push((
                "content-disposition".to_owned(),
                format!("attachment; filename=\"{file}\""),
            ));
        }
        LocalResponse {
            status: 200,
            headers,
            body,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::tests::files;

    fn get(site: &CertSite, direct: bool, path: &str, ua: Option<&str>) -> LocalResponse {
        site.respond(&LocalRequest {
            direct,
            path,
            user_agent: ua,
        })
    }

    fn header<'a>(r: &'a LocalResponse, name: &str) -> Option<&'a str> {
        r.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn serves_every_format() {
        let f = files();
        let site = CertSite::new(f.clone());
        let pem = get(&site, false, "/ca.pem", None);
        assert_eq!(pem.status, 200);
        assert_eq!(pem.body, f.pem.as_bytes());
        let der = get(&site, false, "/ca.crt", None);
        assert_eq!(der.body, f.der);
        assert_eq!(
            header(&der, "content-type"),
            Some("application/x-x509-ca-cert")
        );
        let profile = get(&site, false, "/ca.mobileconfig", None);
        assert_eq!(profile.body, f.mobileconfig().as_bytes());
        assert_eq!(header(&profile, "content-disposition"), None);
        assert_eq!(get(&site, false, "/ca.key", None).status, 404);
        for r in [&pem, &der, &profile] {
            assert!(!String::from_utf8_lossy(&r.body).contains("PRIVATE"));
        }
    }

    #[test]
    fn page_adapts_to_the_device_and_the_entry_point() {
        let site = CertSite::new(files());
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)";
        let page = String::from_utf8(get(&site, false, "/", Some(iphone)).body.to_vec()).unwrap();
        let ios = page.find("<h2>iPhone").unwrap();
        let android = page.find("<h2>Android").unwrap();
        assert!(ios < android, "iOS primero en un iPhone");
        assert!(page.contains("Ajustes de confianza de certificados"));
        assert!(page.contains(r#"href="/ca.mobileconfig""#));

        let direct = String::from_utf8(
            get(&site, true, "/", Some("Linux; Android 14"))
                .body
                .to_vec(),
        )
        .unwrap();
        assert!(direct.find("<h2>Android").unwrap() < direct.find("<h2>iPhone").unwrap());
        assert!(
            direct.contains(r#"href="/cert/ca.crt""#),
            "links con el prefijo directo"
        );
    }

    #[test]
    fn platforms() {
        assert_eq!(
            Platform::from_user_agent(Some("(iPad; CPU OS 17_0)")),
            Platform::Ios
        );
        assert_eq!(
            Platform::from_user_agent(Some("(Linux; Android 13)")),
            Platform::Android
        );
        assert_eq!(
            Platform::from_user_agent(Some("(Windows NT 10.0)")),
            Platform::Desktop
        );
        assert_eq!(Platform::from_user_agent(None), Platform::Desktop);
    }
}
