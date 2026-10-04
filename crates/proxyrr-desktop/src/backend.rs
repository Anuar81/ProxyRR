//! Lógica de la app, sin tipos de Tauri: los comandos de `main.rs` son envoltorios finos de esto, así
//! se prueba con `cargo test` sin abrir ventanas.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use proxyrr_api::{
    Engine, EngineOptions, FlowDto, FlowSummaryDto, Notice, ProxySettings, ProxyStatusDto,
    StatusDto,
};
use proxyrr_cert::{CA_CERT_FILE, CertificateAuthority};
use proxyrr_core::{Headers, LocalSite, ProxyConfig, Verdict};
use proxyrr_devices::guide::{self, GuideContext, Target};
use proxyrr_devices::trust::{self, LinuxTools, Os, Runner, SystemRunner};
use proxyrr_devices::{CaFiles, CertSite, lan_ip, qr_svg};
use proxyrr_rules::{BreakpointEvent, RULES_FILE, Rule, Rules};
use proxyrr_store::{Decoded, StoredFlow, decode_body};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::android::{Android, ConfiguredDto, DevicesDto};
use crate::tools::{self, ComposeDto, DraftKind, EditedDto, PausedDto};

/// Carpeta de descargas del usuario, si existe.
fn downloads_dir() -> Option<PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
    let dir = PathBuf::from(home).join("Downloads");
    dir.is_dir().then_some(dir)
}

/// Texto máximo que se manda a la vista (el resto se ofrece como "exportar").
pub const MAX_TEXT_VIEW: usize = 2 * 1024 * 1024;
/// Bytes máximos para la vista hexadecimal.
pub const MAX_HEX_VIEW: usize = 64 * 1024;

/// Estado de la app.
pub struct Backend {
    engine: Arc<Engine>,
    ca: Arc<CertificateAuthority>,
    files: CaFiles,
    pem: PathBuf,
    data_dir: PathBuf,
    os: Os,
    linux: LinuxTools,
    runner: Box<dyn Runner + Send + Sync>,
    android: Android,
}

impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Backend")
            .field("ca", &self.files.name)
            .finish_non_exhaustive()
    }
}

/// Pedido de `start_proxy` desde la UI.
#[derive(Debug, Clone, Deserialize)]
pub struct StartRequest {
    /// `IP:PUERTO`.
    pub listen: String,
    /// Descifrar HTTPS.
    pub mitm: bool,
    /// Hosts sin descifrar.
    #[serde(default)]
    pub bypass: Vec<String>,
}

/// Lado de un flujo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// Request.
    Request,
    /// Response.
    Response,
}

/// Cómo mostrar un body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    /// Vacío.
    Empty,
    /// Texto (UTF-8 o casi).
    Text,
    /// JSON válido: la vista lo formatea.
    Json,
    /// Imagen: `base64` para un `data:` URL.
    Image,
    /// Binario: `base64` de los primeros [`MAX_HEX_VIEW`] bytes, para la vista hex.
    Binary,
}

/// Body listo para la vista.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BodyView {
    /// Tipo de vista.
    pub kind: BodyKind,
    /// `Content-Type` declarado.
    pub content_type: Option<String>,
    /// `Content-Encoding` declarado.
    pub content_encoding: Option<String>,
    /// Tamaño real que pasó por el proxy.
    pub size: u64,
    /// Bytes guardados (puede ser menos que `size`).
    pub captured: usize,
    /// El proxy guardó menos de lo que pasó.
    pub truncated: bool,
    /// El body se cortó (cliente u origen cerraron).
    pub complete: bool,
    /// Aviso para mostrar arriba del body (decodificación parcial, texto recortado…).
    pub note: Option<String>,
    /// Texto (kind `text`/`json`).
    pub text: Option<String>,
    /// Bytes en base64 (kind `image`/`binary`).
    pub base64: Option<String>,
}

/// Estado de la CA en este equipo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CaOverview {
    /// Nombre (CN).
    pub name: String,
    /// Huella SHA-256.
    pub sha256: String,
    /// Almacenes revisados.
    pub stores: Vec<StoreDto>,
    /// `true` si está instalada en al menos un almacén.
    pub installed: bool,
    /// Aviso sobre Firefox.
    pub firefox_note: Option<String>,
    /// Advertencia previa a instalar.
    pub warning: &'static str,
}

/// Un almacén.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoreDto {
    /// Nombre.
    pub store: String,
    /// `Some(true)` instalada, `Some(false)` no, `None` desconocido.
    pub installed: Option<bool>,
    /// Detalle si no se pudo verificar.
    pub detail: Option<String>,
}

/// Guía lista para la vista.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GuideDto {
    /// Título.
    pub title: String,
    /// Pasos.
    pub steps: Vec<String>,
    /// Bloques para copiar: `[título, contenido]`.
    pub snippets: Vec<(String, String)>,
    /// URL del QR.
    pub qr_url: Option<String>,
    /// QR en SVG (generado acá, sin datos de flujos: seguro de insertar).
    pub qr_svg: Option<String>,
    /// Avisos.
    pub notes: Vec<String>,
    /// `true` si el destino admite instalación automática desde la app.
    pub can_install: bool,
}

impl Backend {
    /// Carga (o crea) la CA de `data_dir` y arma el engine con la página `proxyrr.cert`.
    ///
    /// # Errors
    /// Si la CA no se puede cargar o crear.
    pub fn new(data_dir: &std::path::Path) -> Result<Self, String> {
        Self::with_runner(data_dir, Os::current(), Box::new(SystemRunner::graphical()))
    }

    /// Como [`Backend::new`], con el SO y el runner fijados (tests).
    ///
    /// # Errors
    /// Si la CA no se puede cargar o crear.
    pub fn with_runner(
        data_dir: &std::path::Path,
        os: Os,
        runner: Box<dyn Runner + Send + Sync>,
    ) -> Result<Self, String> {
        let ca =
            Arc::new(CertificateAuthority::load_or_create(data_dir).map_err(|e| e.to_string())?);
        let files = CaFiles::from_ca(&ca).map_err(|e| e.to_string())?;
        let site: Arc<dyn LocalSite> = Arc::new(CertSite::new(files.clone()));
        let rules = Arc::new(Rules::load(&data_dir.join(RULES_FILE))?);
        let engine = Engine::new(EngineOptions {
            ca: Some(Arc::clone(&ca)),
            proxy: ProxyConfig {
                local_site: Some(site),
                local_site_ca: Some(Arc::clone(&ca)),
                ..ProxyConfig::default()
            },
            rules: Some(rules),
            ..EngineOptions::default()
        });
        let linux = if os == Os::Linux {
            LinuxTools::detect()
        } else {
            LinuxTools::default()
        };
        Ok(Self {
            engine: Arc::new(engine),
            ca,
            files,
            pem: data_dir.join(CA_CERT_FILE),
            data_dir: data_dir.to_path_buf(),
            os,
            linux,
            runner,
            android: Android::new(data_dir),
        })
    }

    /// Capa de `tracing` que manda los avisos del motor a la ventana.
    #[must_use]
    pub fn log_layer(&self) -> proxyrr_api::LogLayer {
        self.engine.log_layer()
    }

    /// Guarda todos los flujos HTTP como HAR en la carpeta de descargas (o en el directorio de
    /// datos si no hay) y devuelve la ruta.
    ///
    /// # Errors
    /// Si no se pudo escribir el archivo.
    pub fn export_har(&self) -> Result<String, String> {
        let dir = downloads_dir().unwrap_or_else(|| self.data_dir.clone());
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let path = dir.join(format!("proxyrr-{stamp}.har"));
        let json = serde_json::to_vec_pretty(&self.engine.har()).map_err(|e| e.to_string())?;
        std::fs::write(&path, json)
            .map_err(|e| format!("no se pudo escribir {}: {e}", path.display()))?;
        Ok(path.display().to_string())
    }

    /// Emuladores y dispositivos Android.
    #[must_use]
    pub fn android_devices(&self) -> DevicesDto {
        self.android.devices()
    }

    /// Fija la ruta de `adb` (vacía: buscarla sola) y vuelve a listar.
    ///
    /// # Errors
    /// Ruta inválida.
    pub fn set_adb_path(&self, path: &str) -> Result<DevicesDto, String> {
        self.android.set_adb_path(path)?;
        Ok(self.android.devices())
    }

    /// Configura un dispositivo para el proxy en `port`.
    ///
    /// # Errors
    /// Si `adb` falla.
    pub fn android_configure(&self, serial: &str, port: u16) -> Result<ConfiguredDto, String> {
        self.android.configure(&self.ca, serial, port)
    }

    /// Quita el proxy de un dispositivo.
    ///
    /// # Errors
    /// Si `adb` falla.
    pub fn android_revert(&self, serial: &str) -> Result<(), String> {
        self.android.revert(serial)
    }

    /// Revierte todos los dispositivos configurados (al cerrar).
    pub fn revert_android(&self) {
        self.android.revert_all();
    }

    /// Avisos del engine (para reenviarlos a la ventana).
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Notice> {
        self.engine.subscribe()
    }

    /// Avisos de breakpoints. Mientras la ventana esté suscrita, los breakpoints pausan.
    #[must_use]
    pub fn subscribe_breakpoints(&self) -> broadcast::Receiver<BreakpointEvent> {
        self.engine.rules().breakpoints().subscribe()
    }

    /// Reglas en orden.
    #[must_use]
    pub fn rules(&self) -> Vec<Rule> {
        self.engine.rules().list()
    }

    /// Reemplaza las reglas (se guardan en `rules.json` y aplican desde el próximo request).
    ///
    /// # Errors
    /// Patrón inválido o archivo que no se pudo escribir.
    pub fn save_rules(&self, rules: Vec<Rule>) -> Result<Vec<Rule>, String> {
        self.engine.rules().replace(rules)
    }

    /// Regla nueva completada con los datos de un flujo (sin guardarla).
    ///
    /// # Errors
    /// Si el flujo ya no está o es un túnel.
    pub fn draft_rule(&self, id: u64, kind: DraftKind) -> Result<Rule, String> {
        match self.engine.store().get(id) {
            Some(StoredFlow::Http(record)) => Ok(tools::draft_rule(&record, kind)),
            Some(StoredFlow::Tunnel(_)) => Err("un túnel no tiene request para la regla".into()),
            None => Err(format!("el flujo {id} ya no está")),
        }
    }

    /// Flujos en pausa.
    #[must_use]
    pub fn paused(&self) -> Vec<PausedDto> {
        self.engine
            .rules()
            .breakpoints()
            .pending()
            .iter()
            .map(PausedDto::from)
            .collect()
    }

    /// Decide sobre un flujo en pausa: `execute` (con `edited`), `continue` (sin cambios) o `abort`.
    ///
    /// # Errors
    /// Acción desconocida, `execute` sin cambios, o flujo que ya no está en pausa.
    pub fn resolve_breakpoint(
        &self,
        key: u64,
        action: &str,
        edited: Option<EditedDto>,
    ) -> Result<(), String> {
        let breakpoints = self.engine.rules().breakpoints();
        let original = breakpoints
            .pending()
            .into_iter()
            .find(|p| p.key == key)
            .ok_or("ese flujo ya no está en pausa (venció o el cliente cortó)")?
            .message;
        let verdict = match action {
            "continue" => Verdict::Continue(original),
            "abort" => Verdict::Abort,
            "execute" => Verdict::Continue(tools::apply_edit(
                &original,
                edited.ok_or("falta el mensaje editado")?,
            )),
            other => return Err(format!("acción desconocida: {other}")),
        };
        if breakpoints.resolve(key, verdict) {
            Ok(())
        } else {
            Err("ese flujo ya no está en pausa (venció o el cliente cortó)".into())
        }
    }

    /// Repite un flujo tal cual. Devuelve el id del flujo nuevo.
    ///
    /// # Errors
    /// Proxy apagado, túnel o body recortado.
    pub async fn replay_flow(&self, id: u64) -> Result<u64, String> {
        self.engine.replay_flow(id).await
    }

    /// Request de un flujo para editarlo en Compose.
    ///
    /// # Errors
    /// Túnel, flujo que ya no está o body recortado.
    pub fn compose_from(&self, id: u64) -> Result<ComposeDto, String> {
        self.engine
            .replay_request(id)
            .map(|r| ComposeDto::from_replay(&r))
    }

    /// Manda un request de Compose. Devuelve el id del flujo nuevo.
    ///
    /// # Errors
    /// Proxy apagado, o método/URL inválidos.
    pub async fn compose_send(&self, request: ComposeDto) -> Result<u64, String> {
        self.engine.replay(request.into_replay()).await
    }

    /// Comando `curl` de un flujo, para bash (`"posix"`) o PowerShell (`"powershell"`).
    ///
    /// # Errors
    /// Túnel, flujo que ya no está o body recortado.
    pub fn curl(&self, id: u64, shell: &str) -> Result<String, String> {
        let shell = if shell.eq_ignore_ascii_case("powershell") {
            tools::CurlShell::PowerShell
        } else {
            tools::CurlShell::Posix
        };
        self.engine
            .replay_request(id)
            .map(|r| tools::curl_command_for(&r, shell))
    }

    /// Estado del proxy y del store.
    pub async fn status(&self) -> StatusDto {
        self.engine.status().await.into()
    }

    /// Prende el proxy.
    ///
    /// # Errors
    /// Dirección inválida, puerto ocupado o proxy ya prendido.
    pub async fn start_proxy(&self, request: StartRequest) -> Result<ProxyStatusDto, String> {
        let listen: SocketAddr = request.listen.trim().parse().map_err(|_| {
            format!(
                "dirección inválida: {:?} (esperado IP:PUERTO)",
                request.listen
            )
        })?;
        let bypass = request
            .bypass
            .into_iter()
            .map(|h| h.trim().to_owned())
            .filter(|h| !h.is_empty())
            .collect();
        self.engine
            .start_proxy(ProxySettings {
                listen,
                mitm: request.mitm,
                bypass,
            })
            .await
            .map(Into::into)
            .map_err(|e| e.to_string())
    }

    /// Apaga el proxy (los flujos quedan).
    pub async fn stop_proxy(&self) -> ProxyStatusDto {
        self.engine.stop_proxy().await.into()
    }

    /// Resúmenes, todos o los posteriores a `after`.
    #[must_use]
    pub fn list_flows(&self, after: Option<u64>) -> Vec<FlowSummaryDto> {
        let store = self.engine.store();
        after
            .map_or_else(|| store.list(), |after| store.list_after(after))
            .into_iter()
            .map(Into::into)
            .collect()
    }

    /// Flujo completo.
    ///
    /// # Errors
    /// Si ya no está (se vació o se descartó por los límites).
    pub fn get_flow(&self, id: u64) -> Result<FlowDto, String> {
        self.engine.store().get(id).map(Into::into).ok_or_else(|| {
            format!("el flujo {id} ya no está (se vació o se descartó por el límite)")
        })
    }

    /// Body de un lado, decodificado y clasificado para la vista.
    ///
    /// # Errors
    /// Si el flujo no está, es un túnel o su body todavía se está recibiendo.
    pub fn get_body(&self, id: u64, side: Side) -> Result<BodyView, String> {
        let Some(StoredFlow::Http(record)) = self.engine.store().get(id) else {
            return Err(format!("el flujo {id} no tiene body"));
        };
        let Some(bodies) = &record.bodies else {
            return Err("el body todavía se está recibiendo".to_owned());
        };
        let (body, headers) = match side {
            Side::Request => (&bodies.request, &record.head.request_headers),
            Side::Response => (&bodies.response, &record.head.response_headers),
        };
        Ok(body_view(
            header(headers, "content-type"),
            header(headers, "content-encoding"),
            &body.data,
            body.size,
            body.truncated,
            body.complete,
        ))
    }

    /// Vacía el store.
    pub fn clear(&self) {
        self.engine.clear();
    }

    /// Estado de la CA en este equipo (solo lee).
    #[must_use]
    pub fn ca_overview(&self) -> CaOverview {
        let stores: Vec<StoreDto> = trust::status(
            self.os,
            &self.pem,
            &self.files,
            &self.linux,
            self.runner.as_ref(),
        )
        .into_iter()
        .map(|s| StoreDto {
            store: s.store,
            installed: s.installed,
            detail: s.detail,
        })
        .collect();
        CaOverview {
            name: self.files.name.clone(),
            sha256: self.files.sha256.clone(),
            installed: stores.iter().any(|s| s.installed == Some(true)),
            stores,
            firefox_note: trust::firefox_note(self.os),
            warning: trust::INSTALL_WARNING,
        }
    }

    /// Instala la CA en este equipo (el SO puede pedir confirmación en un diálogo propio).
    ///
    /// # Errors
    /// El paso que falló.
    pub fn ca_install(&self) -> Result<CaOverview, String> {
        let plan = trust::install_plan(self.os, &self.pem, &self.files, &self.linux)?;
        trust::apply(&plan, self.runner.as_ref())?;
        Ok(self.ca_overview())
    }

    /// Quita la CA de este equipo.
    ///
    /// # Errors
    /// El paso que falló.
    pub fn ca_uninstall(&self) -> Result<CaOverview, String> {
        let plan = trust::uninstall_plan(self.os, &self.pem, &self.files, &self.linux)?;
        trust::apply(&plan, self.runner.as_ref())?;
        Ok(self.ca_overview())
    }

    /// Guía para un destino, con el puerto del proxy.
    ///
    /// # Errors
    /// Destino desconocido.
    pub fn guide(&self, target: &str, port: u16) -> Result<GuideDto, String> {
        let target = Target::ALL
            .into_iter()
            .find(|t| t.name() == target)
            .ok_or_else(|| format!("destino desconocido: {target}"))?;
        let lan_host =
            lan_ip().map_or_else(|| "<IP-de-esta-máquina>".to_owned(), |ip| ip.to_string());
        let g = guide::guide(
            target,
            &GuideContext {
                files: &self.files,
                lan_host,
                port,
            },
        );
        Ok(GuideDto {
            qr_svg: g.qr_url.as_deref().and_then(qr_svg),
            title: g.title,
            steps: g.steps,
            snippets: g.snippets,
            qr_url: g.qr_url,
            notes: g.notes,
            can_install: target == Target::IosSimulator && self.os == Os::Macos,
        })
    }

    /// Instala la CA en el simulador de iOS abierto (macOS).
    ///
    /// # Errors
    /// Si no es macOS o `xcrun` falla.
    pub fn install_ios_simulator(&self) -> Result<(), String> {
        if self.os != Os::Macos {
            return Err("el simulador de iOS solo existe en macOS".to_owned());
        }
        let step = trust::Step {
            what: "Agregar la CA al simulador de iOS abierto".to_owned(),
            program: "xcrun".to_owned(),
            args: vec![
                "simctl".into(),
                "keychain".into(),
                "booted".into(),
                "add-root-cert".into(),
                self.pem.display().to_string(),
            ],
            sudo: false,
        };
        trust::apply(&[step], self.runner.as_ref())
    }
}

fn header<'a>(headers: &'a Headers, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Clasifica y prepara un body para la vista.
#[must_use]
pub fn body_view(
    content_type: Option<&str>,
    content_encoding: Option<&str>,
    raw: &[u8],
    size: u64,
    truncated: bool,
    complete: bool,
) -> BodyView {
    let mut notes = Vec::new();
    if truncated {
        notes.push(format!(
            "El proxy guardó {} de {} bytes (límite de captura).",
            raw.len(),
            size
        ));
    }
    if !complete {
        notes.push("El body se cortó antes de terminar.".to_owned());
    }
    let decoded;
    let data: &[u8] = match decode_body(content_encoding, raw) {
        Decoded::Identity => raw,
        Decoded::Decoded { data, complete } => {
            if !complete {
                notes.push("Se decodificó solo una parte.".to_owned());
            }
            decoded = data;
            &decoded
        }
        Decoded::Unsupported(why) => {
            notes.push(format!("{why}: se muestran los bytes crudos."));
            raw
        }
    };
    let mime = content_type
        .and_then(|ct| ct.split(';').next())
        .map(|m| m.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let mut view = BodyView {
        kind: BodyKind::Empty,
        content_type: content_type.map(str::to_owned),
        content_encoding: content_encoding.map(str::to_owned),
        size,
        captured: raw.len(),
        truncated,
        complete,
        note: None,
        text: None,
        base64: None,
    };
    if data.is_empty() {
        view.note = join(&notes);
        return view;
    }
    if mime.starts_with("image/") && mime != "image/svg+xml" {
        view.kind = BodyKind::Image;
        view.base64 = Some(STANDARD.encode(data));
    } else if let Some(text) = as_text(&mime, data) {
        // JSON si parsea, lo diga o no el `Content-Type` (muchas APIs mandan `text/plain`).
        view.kind = if serde_json::from_str::<serde_json::Value>(&text).is_ok() {
            BodyKind::Json
        } else {
            BodyKind::Text
        };
        if text.len() > MAX_TEXT_VIEW {
            let mut end = MAX_TEXT_VIEW;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            notes.push(format!(
                "Se muestran los primeros {} MB.",
                MAX_TEXT_VIEW / 1024 / 1024
            ));
            view.kind = BodyKind::Text;
            view.text = Some(text[..end].to_owned());
        } else {
            view.text = Some(text);
        }
    } else {
        view.kind = BodyKind::Binary;
        if data.len() > MAX_HEX_VIEW {
            notes.push(format!(
                "Vista hex de los primeros {} KB.",
                MAX_HEX_VIEW / 1024
            ));
        }
        view.base64 = Some(STANDARD.encode(&data[..data.len().min(MAX_HEX_VIEW)]));
    }
    view.note = join(&notes);
    view
}

/// Texto si el tipo lo dice o si los bytes son UTF-8 sin controles raros.
fn as_text(mime: &str, data: &[u8]) -> Option<String> {
    let textual = mime.starts_with("text/")
        || [
            "json",
            "xml",
            "javascript",
            "x-www-form-urlencoded",
            "graphql",
            "svg",
            "yaml",
            "csv",
        ]
        .iter()
        .any(|t| mime.contains(t));
    match std::str::from_utf8(data) {
        Ok(text) if textual || !text.chars().any(|c| c.is_control() && !c.is_whitespace()) => {
            Some(text.to_owned())
        }
        Err(_) if textual => Some(String::from_utf8_lossy(data).into_owned()),
        _ => None,
    }
}

fn join(notes: &[String]) -> Option<String> {
    (!notes.is_empty()).then(|| notes.join(" "))
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write as _};
    use std::sync::Mutex;

    use proxyrr_devices::trust::{RunOutput, Step};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::*;

    /// Runner que registra y responde `ok` a todo.
    #[derive(Default)]
    struct Recording(Arc<Mutex<Vec<String>>>);

    impl Runner for Recording {
        fn run_interactive(&self, step: &Step) -> io::Result<bool> {
            self.0.lock().unwrap().push(step.command_line());
            Ok(true)
        }
        fn run_captured(&self, _: &Step) -> io::Result<RunOutput> {
            Ok(RunOutput {
                success: false,
                ..RunOutput::default()
            })
        }
        fn read_file(&self, _: &std::path::Path) -> Option<String> {
            None
        }
    }

    fn backend(os: Os) -> (Backend, Arc<Mutex<Vec<String>>>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let b =
            Backend::with_runner(dir.path(), os, Box::new(Recording(Arc::clone(&log)))).unwrap();
        (b, log, dir)
    }

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn body_views() {
        let json = body_view(
            Some("application/json; charset=utf-8"),
            None,
            br#"{"a":1}"#,
            7,
            false,
            true,
        );
        assert_eq!(json.kind, BodyKind::Json);
        assert_eq!(json.text.as_deref(), Some(r#"{"a":1}"#));

        let sniffed = body_view(None, None, b"[1,2]", 5, false, true);
        assert_eq!(sniffed.kind, BodyKind::Json, "JSON sin content-type");

        let html = body_view(
            Some("text/html"),
            Some("gzip"),
            &gzip(b"<p>hola</p>"),
            30,
            false,
            true,
        );
        assert_eq!(html.kind, BodyKind::Text);
        assert_eq!(
            html.text.as_deref(),
            Some("<p>hola</p>"),
            "gzip decodificado"
        );

        let png = body_view(
            Some("image/png"),
            None,
            &[0x89, b'P', b'N', b'G'],
            4,
            false,
            true,
        );
        assert_eq!(png.kind, BodyKind::Image);
        assert!(png.base64.is_some() && png.text.is_none());

        let svg = body_view(Some("image/svg+xml"), None, b"<svg></svg>", 11, false, true);
        assert_eq!(
            svg.kind,
            BodyKind::Text,
            "un SVG no va como <img>: podría llevar scripts"
        );

        let bin = body_view(
            Some("application/octet-stream"),
            None,
            &[0, 1, 2, 255],
            4,
            false,
            true,
        );
        assert_eq!(bin.kind, BodyKind::Binary);

        let empty = body_view(None, None, b"", 0, false, true);
        assert_eq!(empty.kind, BodyKind::Empty);

        let cut = body_view(Some("text/plain"), None, b"abc", 10, true, false);
        let note = cut.note.unwrap();
        assert!(
            note.contains("3 de 10") && note.contains("se cortó"),
            "{note}"
        );

        let odd = body_view(Some("text/plain"), Some("zstd"), b"abc", 3, false, true);
        assert_eq!(odd.text.as_deref(), Some("abc"));
        assert!(odd.note.unwrap().contains("zstd"));
    }

    #[test]
    fn long_text_is_capped_on_a_char_boundary() {
        let text = "ñ".repeat(MAX_TEXT_VIEW);
        let view = body_view(
            Some("text/plain"),
            None,
            text.as_bytes(),
            text.len() as u64,
            false,
            true,
        );
        let shown = view.text.unwrap();
        assert!(shown.len() <= MAX_TEXT_VIEW);
        assert!(view.note.unwrap().contains("primeros"));
    }

    #[test]
    fn install_runs_the_plan_for_this_os() {
        let (b, log, _dir) = backend(Os::Windows);
        let overview = b.ca_overview();
        assert!(!overview.installed);
        assert!(overview.name.starts_with("ProxyRR CA ("));
        b.ca_install().unwrap();
        b.ca_uninstall().unwrap();
        let ran = log.lock().unwrap().clone();
        assert!(
            ran[0].starts_with("certutil -user -addstore Root"),
            "{ran:?}"
        );
        assert!(ran[1].contains("-delstore Root"), "{ran:?}");
    }

    #[test]
    fn guides_and_simulator() {
        let (b, log, _dir) = backend(Os::Macos);
        let ios = b.guide("ios", 9191).unwrap();
        assert!(ios.qr_url.as_deref().unwrap().ends_with(":9191/cert"));
        assert!(ios.qr_svg.as_deref().unwrap().contains("<svg"));
        assert!(!ios.can_install);
        assert!(b.guide("ios-simulator", 9090).unwrap().can_install);
        assert!(b.guide("nada", 9090).is_err());
        b.install_ios_simulator().unwrap();
        assert!(log.lock().unwrap()[0].starts_with("xcrun simctl keychain booted add-root-cert"));

        let (win, _, _dir) = backend(Os::Windows);
        assert!(!win.guide("ios-simulator", 9090).unwrap().can_install);
        assert!(win.install_ios_simulator().is_err());
    }

    /// De punta a punta sin ventana: prender, pasar un request, verlo en la lista y en el aviso, leer
    /// el body, apagar y vaciar.
    #[tokio::test]
    async fn capture_flow_end_to_end() {
        let (b, _, _dir) = backend(Os::current());
        let mut notices = b.subscribe();
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = origin.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf).await;
            let body = gzip(br#"{"ok":true}"#);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            s.write_all(head.as_bytes()).await.unwrap();
            s.write_all(&body).await.unwrap();
        });

        assert!(
            b.start_proxy(StartRequest {
                listen: "acá".into(),
                mitm: false,
                bypass: vec![]
            })
            .await
            .is_err()
        );
        let status = b
            .start_proxy(StartRequest {
                listen: "127.0.0.1:0".into(),
                mitm: true,
                bypass: vec![" ".into()],
            })
            .await
            .unwrap();
        let proxy = status.listen.clone().unwrap();

        let mut client = TcpStream::connect(&proxy).await.unwrap();
        let req = format!(
            "GET http://{origin_addr}/x HTTP/1.1\r\nHost: {origin_addr}\r\nConnection: close\r\n\r\n"
        );
        client.write_all(req.as_bytes()).await.unwrap();
        let mut reply = Vec::new();
        client.read_to_end(&mut reply).await.unwrap();

        let id = loop {
            let notice = tokio::time::timeout(std::time::Duration::from_secs(10), notices.recv())
                .await
                .unwrap()
                .unwrap();
            if let Notice::Flow(f) = notice
                && !f.in_progress
            {
                break f.id;
            }
        };
        assert_eq!(b.list_flows(None).len(), 1);
        assert!(b.list_flows(Some(id)).is_empty());
        let body = b.get_body(id, Side::Response).unwrap();
        assert_eq!(body.kind, BodyKind::Json);
        assert_eq!(body.text.as_deref(), Some(r#"{"ok":true}"#));
        assert_eq!(body.content_encoding.as_deref(), Some("gzip"));
        assert!(matches!(b.get_flow(id).unwrap(), FlowDto::Http { .. }));

        let stopped = b.stop_proxy().await;
        assert!(!stopped.running);
        assert_eq!(b.status().await.flows, 1, "apagar no borra");
        b.clear();
        assert!(b.get_flow(id).is_err());
    }
}
