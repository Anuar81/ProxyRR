//! Modelo de una regla y su patrón de URL.

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

/// Una regla: a qué flujos aplica y qué hace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// Id estable (lo asigna [`crate::Rules::replace`] si viene en 0).
    #[serde(default)]
    pub id: u64,
    /// Nombre que se muestra en el flujo.
    pub name: String,
    /// Inactiva: se guarda pero no aplica.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Método (`GET`, `POST`…). Vacío o `None`: cualquiera.
    #[serde(default)]
    pub method: Option<String>,
    /// Patrón de URL: wildcard con `*` (por defecto) o regex.
    pub url: String,
    /// `url` es una regex.
    #[serde(default)]
    pub regex: bool,
    /// Qué hace.
    pub action: Action,
}

fn yes() -> bool {
    true
}

/// Acción de una regla.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Responder sin ir al origen.
    MapLocal(MapLocal),
    /// Mandar el request a otro destino.
    MapRemote(MapRemote),
    /// Responder `status` sin ir al origen.
    Block {
        /// Status (403 por defecto).
        #[serde(default = "forbidden")]
        status: u16,
    },
    /// Quitar validadores de caché y pedir respuestas frescas.
    NoCache,
    /// Pausar para editar.
    Breakpoint {
        /// Pausar el request.
        #[serde(default)]
        request: bool,
        /// Pausar la respuesta.
        #[serde(default)]
        response: bool,
    },
}

fn forbidden() -> u16 {
    403
}

fn ok() -> u16 {
    200
}

/// Respuesta de Map Local.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapLocal {
    /// Status.
    #[serde(default = "ok")]
    pub status: u16,
    /// Headers (`Content-Length` se recalcula).
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// Body en texto.
    #[serde(default)]
    pub body: String,
    /// Archivo cuyo contenido es el body (gana sobre `body`). Se lee en cada request.
    #[serde(default)]
    pub file: Option<String>,
}

/// Destino de Map Remote. Los campos vacíos no cambian.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapRemote {
    /// `http` o `https`.
    #[serde(default)]
    pub scheme: Option<String>,
    /// Host.
    #[serde(default)]
    pub host: Option<String>,
    /// Puerto.
    #[serde(default)]
    pub port: Option<u16>,
    /// Path (empieza con `/`).
    #[serde(default)]
    pub path: Option<String>,
    /// Query sin `?`.
    #[serde(default)]
    pub query: Option<String>,
    /// Mantener el `Host` original.
    #[serde(default)]
    pub preserve_host: bool,
}

/// Patrón compilado de una regla.
#[derive(Debug, Clone)]
pub(crate) struct Matcher {
    method: Option<String>,
    re: Regex,
    /// Si es `false`, se compara contra la URL sin query.
    with_query: bool,
}

impl Matcher {
    /// Compila el patrón de `rule`.
    pub(crate) fn compile(rule: &Rule) -> Result<Self, String> {
        let pattern = rule.url.trim();
        if pattern.is_empty() {
            return Err(format!("la regla \"{}\" no tiene patrón de URL", rule.name));
        }
        let source = if rule.regex {
            pattern.to_owned()
        } else {
            wildcard_regex(pattern)
        };
        let re = RegexBuilder::new(&source)
            .case_insensitive(true)
            .size_limit(1 << 20)
            .build()
            .map_err(|e| format!("la regla \"{}\" tiene un patrón inválido: {e}", rule.name))?;
        let method = rule
            .method
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty() && !m.eq_ignore_ascii_case("any"))
            .map(str::to_ascii_uppercase);
        Ok(Self {
            method,
            re,
            with_query: rule.regex || pattern.contains('?'),
        })
    }

    /// `true` si el request coincide.
    pub(crate) fn matches(&self, method: &str, url: &str) -> bool {
        if let Some(m) = &self.method
            && !m.eq_ignore_ascii_case(method)
        {
            return false;
        }
        let url = if self.with_query {
            url
        } else {
            url.split_once('?').map_or(url, |(base, _)| base)
        };
        self.re.is_match(url)
    }
}

/// Wildcard a regex anclada: `*` es cualquier cosa y el resto es literal. Sin esquema, vale cualquiera.
fn wildcard_regex(pattern: &str) -> String {
    let pattern = if pattern.contains("://") {
        pattern.to_owned()
    } else {
        format!("*://{pattern}")
    };
    let body: Vec<String> = pattern.split('*').map(regex::escape).collect();
    format!("^{}$", body.join(".*"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(url: &str, regex: bool, method: Option<&str>) -> Rule {
        Rule {
            id: 1,
            name: "r".into(),
            enabled: true,
            method: method.map(str::to_owned),
            url: url.into(),
            regex,
            action: Action::NoCache,
        }
    }

    fn m(url: &str) -> Matcher {
        Matcher::compile(&rule(url, false, None)).unwrap()
    }

    #[test]
    fn matcher_wildcard() {
        let any_path = m("https://api.example.com/v1/*");
        assert!(any_path.matches("GET", "https://api.example.com/v1/users"));
        assert!(any_path.matches("GET", "https://API.example.com/v1/users?id=2"));
        assert!(!any_path.matches("GET", "https://api.example.com/v2/users"));
        assert!(!any_path.matches("GET", "http://evil.com/?https://api.example.com/v1/x"));

        let exact = m("https://api.example.com/v1/users");
        assert!(
            exact.matches("GET", "https://api.example.com/v1/users?page=2"),
            "sin `?` en el patrón, la query no cuenta"
        );
        assert!(!exact.matches("GET", "https://api.example.com/v1/users/7"));

        let with_query = m("https://api.example.com/search?q=*");
        assert!(with_query.matches("GET", "https://api.example.com/search?q=hola"));
        assert!(!with_query.matches("GET", "https://api.example.com/search"));

        let no_scheme = m("*.example.com/*");
        assert!(no_scheme.matches("GET", "http://cdn.example.com/a.js"));
        assert!(no_scheme.matches("GET", "https://img.example.com/b.png"));
        assert!(!no_scheme.matches("GET", "https://example.org/"));

        let dots = m("https://a.b/x.json");
        assert!(
            !dots.matches("GET", "https://aXb/xXjson"),
            "el punto es literal"
        );
    }

    #[test]
    fn matcher_regex_and_method() {
        let re = Matcher::compile(&rule(r"/users/\d+$", true, Some("post"))).unwrap();
        assert!(re.matches("POST", "https://x.com/users/12"));
        assert!(!re.matches("GET", "https://x.com/users/12"));
        assert!(!re.matches("POST", "https://x.com/users/me"));
        let any = Matcher::compile(&rule("*", false, Some("ANY"))).unwrap();
        assert!(any.matches("DELETE", "https://x.com/"));
    }

    #[test]
    fn invalid_regex_is_rejected() {
        let err = Matcher::compile(&rule("(", true, None)).unwrap_err();
        assert!(err.contains("\"r\""), "{err}");
        assert!(Matcher::compile(&rule("  ", false, None)).is_err());
    }

    #[test]
    fn actions_serialize_with_a_type_tag() {
        let json = serde_json::to_value(Action::Block { status: 403 }).unwrap();
        assert_eq!(json, serde_json::json!({"type": "block", "status": 403}));
        let parsed: Rule = serde_json::from_str(
            r#"{"name":"mock","url":"*/a","action":{"type":"map_local","body":"{}"}}"#,
        )
        .unwrap();
        assert!(parsed.enabled);
        let Action::MapLocal(local) = parsed.action else {
            panic!("debía ser map_local");
        };
        assert_eq!(local.status, 200);
    }
}
