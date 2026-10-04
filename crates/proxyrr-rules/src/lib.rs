//! Reglas de ProxyRR (spec 0011).
//!
//! Patrones por wildcard/regex y acciones sobre los flujos: Map Local, Map Remote, Block, No Caching y
//! Breakpoint. [`Rules`] implementa el [`FlowHook`] del motor; los cambios aplican al próximo request.

mod breakpoints;
mod rule;

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock};

use bytes::Bytes;
use proxyrr_core::{
    BoxFuture, FlowHook, HeaderEdits, LocalResponse, Paused, Plan, RequestHead, Stage, Verdict,
};
use serde::{Deserialize, Serialize};

pub use breakpoints::{BreakpointEvent, Breakpoints, DEFAULT_PAUSE_TIMEOUT, PausedFlow};
pub use rule::{Action, MapLocal, MapRemote, Rule};

use rule::Matcher;

/// Nombre del archivo de reglas dentro del directorio de datos.
pub const RULES_FILE: &str = "rules.json";

/// Formato del archivo.
#[derive(Debug, Serialize, Deserialize)]
struct RulesFile {
    version: u32,
    rules: Vec<Rule>,
}

#[derive(Debug, Default)]
struct Compiled {
    rules: Vec<Rule>,
    matchers: Vec<Matcher>,
}

impl Compiled {
    fn new(rules: Vec<Rule>) -> Result<Self, String> {
        let matchers = rules
            .iter()
            .map(Matcher::compile)
            .collect::<Result<_, _>>()?;
        Ok(Self { rules, matchers })
    }
}

/// Conjunto de reglas vivo, opcionalmente guardado en un archivo.
#[derive(Debug, Default)]
pub struct Rules {
    compiled: RwLock<Compiled>,
    path: Option<PathBuf>,
    breakpoints: Breakpoints,
}

impl Rules {
    /// Reglas en memoria, sin archivo.
    #[must_use]
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Carga las reglas de `path` (sin archivo: ninguna) y guarda ahí cada cambio.
    ///
    /// # Errors
    /// Si el archivo existe pero no se puede leer o interpretar, o tiene un patrón inválido.
    pub fn load(path: &Path) -> Result<Self, String> {
        let rules = match std::fs::read(path) {
            Ok(bytes) => {
                serde_json::from_slice::<RulesFile>(&bytes)
                    .map_err(|e| {
                        format!("{} no es un archivo de reglas válido: {e}", path.display())
                    })?
                    .rules
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("no se pudo leer {}: {e}", path.display())),
        };
        Ok(Self {
            compiled: RwLock::new(Compiled::new(rules)?),
            path: Some(path.to_path_buf()),
            breakpoints: Breakpoints::default(),
        })
    }

    /// Cola de breakpoints (para la UI).
    #[must_use]
    pub fn breakpoints(&self) -> &Breakpoints {
        &self.breakpoints
    }

    /// Reglas actuales, en orden.
    #[must_use]
    pub fn list(&self) -> Vec<Rule> {
        self.read().rules.clone()
    }

    /// Reemplaza todas las reglas (asigna id a las que vienen en 0), las guarda y las aplica desde el
    /// próximo request. Devuelve las reglas con sus ids.
    ///
    /// # Errors
    /// Patrón inválido (no cambia nada) o archivo que no se pudo escribir.
    pub fn replace(&self, mut rules: Vec<Rule>) -> Result<Vec<Rule>, String> {
        let mut next = rules.iter().map(|r| r.id).max().unwrap_or(0);
        for rule in &mut rules {
            rule.name = rule.name.trim().to_owned();
            if rule.name.is_empty() {
                rule.name = rule.url.trim().to_owned();
            }
            if rule.id == 0 {
                next += 1;
                rule.id = next;
            }
        }
        let compiled = Compiled::new(rules.clone())?;
        if let Some(path) = &self.path {
            save(path, &rules)?;
        }
        *self
            .compiled
            .write()
            .unwrap_or_else(PoisonError::into_inner) = compiled;
        Ok(rules)
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Compiled> {
        self.compiled.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Plan de un request según las reglas activas.
    #[must_use]
    pub fn plan_for(&self, head: &RequestHead) -> Plan {
        let compiled = self.read();
        let pause_allowed = self.breakpoints.has_listener();
        let mut plan = Plan::default();
        let mut no_cache = false;
        for (rule, matcher) in compiled.rules.iter().zip(&compiled.matchers) {
            if !rule.enabled || !matcher.matches(&head.method, &head.url) {
                continue;
            }
            let applied = match &rule.action {
                Action::MapLocal(local) if plan.respond.is_none() => {
                    plan.respond = Some(map_local(local));
                    true
                }
                Action::Block { status } if plan.respond.is_none() => {
                    plan.respond = Some(text(
                        *status,
                        &format!("ProxyRR: bloqueado por la regla \"{}\".", rule.name),
                    ));
                    true
                }
                Action::MapRemote(remote) if plan.redirect.is_none() => {
                    match map_remote(&head.url, remote) {
                        Some(url) => {
                            plan.redirect = Some(url);
                            plan.preserve_host = remote.preserve_host;
                            true
                        }
                        None => false,
                    }
                }
                Action::NoCache => {
                    no_cache = true;
                    true
                }
                Action::Breakpoint { request, response }
                    if pause_allowed && (*request || *response) =>
                {
                    plan.pause_request |= request;
                    plan.pause_response |= response;
                    true
                }
                _ => false,
            };
            if applied {
                plan.rules.push(rule.name.clone());
            }
        }
        if no_cache {
            plan.request_headers = HeaderEdits {
                remove: vec!["if-none-match".into(), "if-modified-since".into()],
                set: vec![
                    ("cache-control".into(), "no-cache".into()),
                    ("pragma".into(), "no-cache".into()),
                ],
            };
            plan.response_headers = HeaderEdits {
                remove: vec!["etag".into(), "last-modified".into(), "expires".into()],
                set: vec![
                    (
                        "cache-control".into(),
                        "no-store, no-cache, must-revalidate".into(),
                    ),
                    ("pragma".into(), "no-cache".into()),
                ],
            };
        }
        // Una respuesta local no va al origen: no hay request que pausar ni que redirigir.
        if plan.respond.is_some() {
            plan.redirect = None;
            plan.pause_request = false;
            plan.pause_response = false;
        }
        plan
    }
}

impl FlowHook for Rules {
    fn plan(&self, head: &RequestHead) -> Plan {
        self.plan_for(head)
    }

    fn pause(&self, stage: Stage, message: Paused) -> BoxFuture<Verdict> {
        self.breakpoints.pause(stage, message)
    }
}

/// Escribe el archivo de reglas de forma atómica (temporal + renombre).
fn save(path: &Path, rules: &[Rule]) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(&RulesFile {
        version: 1,
        rules: rules.to_vec(),
    })
    .map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("no se pudo crear {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| format!("no se pudo guardar {}: {e}", path.display()))
}

fn text(status: u16, message: &str) -> LocalResponse {
    LocalResponse {
        status,
        headers: vec![("content-type".into(), "text/plain; charset=utf-8".into())],
        body: Bytes::from(format!("{message}\n")),
    }
}

fn map_local(local: &MapLocal) -> LocalResponse {
    let body = match local
        .file
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
    {
        Some(file) => match std::fs::read(file) {
            Ok(data) => Bytes::from(data),
            Err(e) => return text(500, &format!("ProxyRR: Map Local no pudo leer {file}: {e}")),
        },
        None => Bytes::from(local.body.clone()),
    };
    LocalResponse {
        status: local.status,
        headers: local.headers.clone(),
        body,
    }
}

/// Partes de una URL absoluta.
#[derive(Debug, PartialEq, Eq)]
struct UrlParts<'a> {
    scheme: &'a str,
    host: &'a str,
    port: Option<u16>,
    path: &'a str,
    query: Option<&'a str>,
}

fn split_url(url: &str) -> Option<UrlParts<'_>> {
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, path_query) = rest.split_at(end);
    let (path, query) = match path_query.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path_query, None),
    };
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (inside, after) = v6.split_once(']')?;
        let port = after.strip_prefix(':').map(str::parse).transpose().ok()?;
        (&authority[..inside.len() + 2], port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port.parse().ok()?)),
            None => (authority, None),
        }
    };
    Some(UrlParts {
        scheme,
        host,
        port,
        path: if path.is_empty() { "/" } else { path },
        query,
    })
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme.to_ascii_lowercase().as_str() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

fn non_empty(value: Option<&String>) -> Option<&str> {
    value.map(|v| v.trim()).filter(|v| !v.is_empty())
}

/// URL nueva de Map Remote: los campos con valor reemplazan los de `url`, los vacíos no cambian.
fn map_remote(url: &str, remote: &MapRemote) -> Option<String> {
    let parts = split_url(url)?;
    let scheme = non_empty(remote.scheme.as_ref()).unwrap_or(parts.scheme);
    let host = non_empty(remote.host.as_ref()).unwrap_or(parts.host);
    let port = remote.port.or_else(|| {
        // Al cambiar de esquema, el puerto por defecto del viejo no se arrastra.
        parts
            .port
            .filter(|p| Some(*p) != default_port(parts.scheme))
    });
    let path = non_empty(remote.path.as_ref()).unwrap_or(parts.path);
    let query = non_empty(remote.query.as_ref()).or(parts.query);
    let mut out = format!("{scheme}://{host}");
    if let Some(port) = port.filter(|p| Some(*p) != default_port(scheme)) {
        let _ = write!(out, ":{port}");
    }
    if !path.starts_with('/') {
        out.push('/');
    }
    out.push_str(path);
    if let Some(query) = query {
        out.push('?');
        out.push_str(query.trim_start_matches('?'));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(method: &str, url: &str) -> RequestHead {
        RequestHead {
            id: 1,
            method: method.into(),
            url: url.into(),
            headers: vec![],
        }
    }

    fn rule(name: &str, url: &str, action: Action) -> Rule {
        Rule {
            id: 0,
            name: name.into(),
            enabled: true,
            method: None,
            url: url.into(),
            regex: false,
            action,
        }
    }

    #[test]
    fn plan_map_local_and_block() {
        let rules = Rules::in_memory();
        rules
            .replace(vec![
                rule(
                    "mock",
                    "https://api.x.com/users*",
                    Action::MapLocal(MapLocal {
                        status: 201,
                        headers: vec![("content-type".into(), "application/json".into())],
                        body: "[]".into(),
                        file: None,
                    }),
                ),
                rule("ads", "*.ads.com/*", Action::Block { status: 403 }),
            ])
            .unwrap();
        let plan = rules.plan_for(&head("GET", "https://api.x.com/users?page=1"));
        let local = plan.respond.unwrap();
        assert_eq!((local.status, local.body.as_ref()), (201, b"[]".as_ref()));
        assert_eq!(plan.rules, ["mock"]);

        let blocked = rules
            .plan_for(&head("GET", "http://cdn.ads.com/a.js"))
            .respond
            .unwrap();
        assert_eq!(blocked.status, 403);
        assert_eq!(
            rules.plan_for(&head("GET", "https://ok.com/")),
            Plan::default()
        );
    }

    #[test]
    fn plan_first_responder_wins_and_disabled_rules_skip() {
        let rules = Rules::in_memory();
        let mut off = rule("off", "*", Action::Block { status: 418 });
        off.enabled = false;
        rules
            .replace(vec![
                off,
                rule("first", "*", Action::Block { status: 403 }),
                rule("second", "*", Action::Block { status: 404 }),
                rule("cache", "*", Action::NoCache),
            ])
            .unwrap();
        let plan = rules.plan_for(&head("GET", "https://a.com/"));
        assert_eq!(plan.respond.unwrap().status, 403);
        assert_eq!(plan.rules, ["first", "cache"]);
        assert!(
            plan.response_headers
                .set
                .iter()
                .any(|(n, v)| n == "cache-control" && v.contains("no-store"))
        );
        assert!(
            plan.request_headers
                .remove
                .contains(&"if-none-match".to_owned())
        );
    }

    #[test]
    fn plan_map_remote() {
        let rules = Rules::in_memory();
        rules
            .replace(vec![rule(
                "local",
                "https://api.prod.com/*",
                Action::MapRemote(MapRemote {
                    scheme: Some("http".into()),
                    host: Some("localhost".into()),
                    port: Some(3000),
                    ..MapRemote::default()
                }),
            )])
            .unwrap();
        let plan = rules.plan_for(&head("POST", "https://api.prod.com/v1/login?x=1"));
        assert_eq!(
            plan.redirect.as_deref(),
            Some("http://localhost:3000/v1/login?x=1")
        );
        assert!(!plan.preserve_host);
    }

    #[test]
    fn map_remote_composition() {
        let only_host = MapRemote {
            host: Some("staging.x.com".into()),
            ..MapRemote::default()
        };
        assert_eq!(
            map_remote("https://x.com:8443/a?b=1", &only_host).unwrap(),
            "https://staging.x.com:8443/a?b=1"
        );
        let to_https = MapRemote {
            scheme: Some("https".into()),
            path: Some("/v2".into()),
            query: Some("new=1".into()),
            ..MapRemote::default()
        };
        assert_eq!(
            map_remote("http://x.com/v1", &to_https).unwrap(),
            "https://x.com/v2?new=1"
        );
        assert_eq!(
            map_remote("http://[::1]:8080", &MapRemote::default()).unwrap(),
            "http://[::1]:8080/"
        );
        assert!(map_remote("no-es-url", &MapRemote::default()).is_none());
    }

    #[test]
    fn plan_breakpoint_needs_a_listener() {
        let rules = Rules::in_memory();
        rules
            .replace(vec![rule(
                "bp",
                "*",
                Action::Breakpoint {
                    request: true,
                    response: true,
                },
            )])
            .unwrap();
        assert!(!rules.plan_for(&head("GET", "https://a.com/")).pause_request);
        let _ui = rules.breakpoints().subscribe();
        let plan = rules.plan_for(&head("GET", "https://a.com/"));
        assert!(plan.pause_request && plan.pause_response);
        assert_eq!(plan.rules, ["bp"]);
    }

    #[test]
    fn map_local_reads_the_file_each_time() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mock.json");
        std::fs::write(&file, "{\"v\":1}").unwrap();
        let local = MapLocal {
            status: 200,
            headers: vec![],
            body: "ignorado".into(),
            file: Some(file.display().to_string()),
        };
        assert_eq!(map_local(&local).body.as_ref(), b"{\"v\":1}");
        std::fs::write(&file, "{\"v\":2}").unwrap();
        assert_eq!(map_local(&local).body.as_ref(), b"{\"v\":2}");
        std::fs::remove_file(&file).unwrap();
        assert_eq!(map_local(&local).status, 500);
    }

    #[test]
    fn rules_persist_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(RULES_FILE);
        let rules = Rules::load(&path).unwrap();
        assert!(rules.list().is_empty(), "sin archivo, ninguna regla");
        let saved = rules
            .replace(vec![
                rule("  ", "*.a.com/*", Action::NoCache),
                rule("b", "*", Action::Block { status: 403 }),
            ])
            .unwrap();
        assert_eq!(saved.iter().map(|r| r.id).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(saved[0].name, "*.a.com/*", "sin nombre usa el patrón");
        let again = Rules::load(&path).unwrap();
        assert_eq!(again.list(), saved);
        assert!(!dir.path().join("rules.json.tmp").exists());
    }

    #[test]
    fn invalid_rules_change_nothing() {
        let rules = Rules::in_memory();
        rules
            .replace(vec![rule("ok", "*", Action::NoCache)])
            .unwrap();
        let mut bad = rule("mala", "(", Action::NoCache);
        bad.regex = true;
        let err = rules.replace(vec![bad]).unwrap_err();
        assert!(err.contains("mala"), "{err}");
        assert_eq!(rules.list().len(), 1);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(RULES_FILE);
        std::fs::write(&path, "no es json").unwrap();
        assert!(Rules::load(&path).is_err());
    }
}
