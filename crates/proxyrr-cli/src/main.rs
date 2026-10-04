//! `proxyrr`: interfaz de línea de comandos de ProxyRR.

use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use proxyrr_api::{ApiConfig, ApiServer, Engine, EngineOptions, ProxySettings};
use proxyrr_cert::{CA_CERT_FILE, CertificateAuthority};
use proxyrr_core::{FlowEvent, LocalSite, ProxyConfig};
use proxyrr_devices::{CaFiles, CertSite};
use tokio::sync::broadcast::error::RecvError;
use tracing_subscriber::Layer as _;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

mod android;
mod setup;

/// Proxy HTTP(S) de depuración multiplataforma.
#[derive(Debug, Parser)]
#[command(name = "proxyrr", version, about)]
struct Cli {
    /// Directorio de datos (CA, sesiones). Por defecto, el del usuario para ProxyRR.
    #[arg(long, global = true, value_name = "DIR")]
    data_dir: Option<PathBuf>,

    /// Ruta de `adb`. Por defecto: `PROXYRR_ADB`, el SDK de Android (`ANDROID_HOME`) o el `PATH`.
    #[arg(long, global = true, value_name = "RUTA")]
    adb: Option<PathBuf>,

    /// Nivel de log del motor por stderr: error, warn, info, debug, trace.
    #[arg(long, global = true, value_name = "NIVEL", default_value = "warn")]
    log_level: tracing_subscriber::filter::LevelFilter,

    #[command(subcommand)]
    command: Option<Command>,
}

/// Opciones de `start` que no son de la API.
#[derive(Debug, clap::Args)]
struct StartArgs {
    /// Dirección de escucha. Usá 0.0.0.0:9090 para aceptar dispositivos de tu red por Wi-Fi.
    #[arg(long, value_name = "IP:PUERTO", default_value_t = ProxyConfig::default().listen)]
    listen: SocketAddr,
    /// Descifrar HTTPS con la CA de ProxyRR (hay que instalarla como raíz de confianza).
    #[arg(long)]
    mitm: bool,
    /// Host que no se descifra (repetible): `example.com` o `*.example.com` para subdominios.
    #[arg(long, value_name = "HOST", requires = "mitm")]
    bypass: Vec<String>,
    /// Segundos para conectar con un origen antes de devolver 504.
    #[arg(long, value_name = "SEGUNDOS", default_value_t = proxyrr_core::DEFAULT_CONNECT_TIMEOUT.as_secs())]
    connect_timeout: u64,
    /// CA extra (PEM o DER) en la que confiar al hablar con orígenes HTTPS (repetible): CA
    /// corporativa o servidor de desarrollo con certificado propio.
    #[arg(long, value_name = "ARCHIVO")]
    upstream_ca: Vec<PathBuf>,
    /// NO verificar los certificados de los orígenes. Solo para desarrollo local: cualquiera en el
    /// camino puede hacerse pasar por el origen.
    #[arg(long)]
    insecure_upstream: bool,
    /// Al salir, guardar todos los flujos HTTP en este archivo HAR.
    #[arg(long, value_name = "ARCHIVO")]
    har: Option<PathBuf>,
    /// Configurar un emulador o dispositivo Android (serial, o `auto` si hay uno solo) al arrancar,
    /// y revertirlo al salir.
    #[arg(long, value_name = "SERIAL", num_args = 0..=1, default_missing_value = "auto")]
    android: Option<String>,
}

#[derive(Debug, Subcommand)]
enum AndroidCommand {
    /// Lista emuladores y dispositivos y qué modo de CA admite cada uno.
    Devices,
    /// Configura el proxy y la CA en un emulador o dispositivo (sin levantar el proxy).
    Setup {
        /// Serial de `adb`. Sin esto, el único conectado.
        serial: Option<String>,
        /// Puerto del proxy.
        #[arg(long, default_value_t = proxyrr_core::DEFAULT_PORT)]
        port: u16,
    },
    /// Quita el proxy del dispositivo (si no, se queda sin internet sin ProxyRR).
    Revert {
        /// Serial de `adb`. Sin esto, el único conectado.
        serial: Option<String>,
        /// Puerto del proxy (para quitar el `adb reverse`).
        #[arg(long, default_value_t = proxyrr_core::DEFAULT_PORT)]
        port: u16,
    },
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Levanta el proxy y muestra cada request en la terminal.
    Start {
        #[command(flatten)]
        start: StartArgs,
        /// Levantar también la API de control local (la usa la app de escritorio y scripts).
        /// El token se imprime al arrancar; `PROXYRR_API_TOKEN` lo fija.
        #[arg(long)]
        api: bool,
        /// Dirección de la API (solo loopback).
        #[arg(long, value_name = "IP:PUERTO", default_value_t = ApiConfig::default().listen, requires = "api")]
        api_listen: SocketAddr,
    },
    /// Emuladores y dispositivos Android por `adb`.
    #[command(subcommand)]
    Android(AndroidCommand),
    /// Autoridad certificante de ProxyRR.
    #[command(subcommand)]
    Ca(CaCommand),
    /// Guía paso a paso para usar ProxyRR con un dispositivo o este equipo.
    Setup {
        /// Destino.
        #[arg(value_enum)]
        target: setup::TargetArg,
        /// IP de esta máquina que ve el dispositivo. Por defecto se detecta la de la LAN.
        #[arg(long, value_name = "IP")]
        host: Option<String>,
        /// Puerto del proxy.
        #[arg(long, default_value_t = proxyrr_core::DEFAULT_PORT)]
        port: u16,
        /// Hacer la instalación automática cuando el destino la permite
        /// (`ios-simulator` en macOS, o este mismo equipo).
        #[arg(long)]
        install: bool,
        /// No imprimir el QR.
        #[arg(long)]
        no_qr: bool,
    },
}

#[derive(Debug, Subcommand)]
enum CaCommand {
    /// Muestra los datos de la CA (la crea si no existe).
    Info,
    /// Exporta el certificado de la CA (PEM por defecto).
    Export {
        /// Exportar en DER (binario) en vez de PEM. Requiere --out.
        #[arg(long)]
        der: bool,
        /// Archivo de salida. Sin esto, PEM por stdout.
        #[arg(long, short, value_name = "ARCHIVO")]
        out: Option<PathBuf>,
    },
    /// Imprime el directorio donde vive la CA.
    Path,
    /// Instala la CA como raíz de confianza de este equipo (tu usuario).
    Install {
        /// Mostrar los comandos sin ejecutarlos.
        #[arg(long)]
        dry_run: bool,
    },
    /// Quita esta CA (por huella) del almacén de confianza de este equipo.
    Uninstall {
        /// Mostrar los comandos sin ejecutarlos.
        #[arg(long)]
        dry_run: bool,
    },
    /// Dice si la CA está instalada y es de confianza, y en qué almacenes.
    Status,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let Some(command) = cli.command else {
        println!(
            "proxyrr {}: usá `proxyrr --help` para ver los comandos.",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    };
    let data_dir = cli.data_dir;
    init_logging(cli.log_level);
    match command {
        Command::Start {
            start,
            api,
            api_listen,
        } => {
            // La CA se carga siempre: sirve la página `proxyrr.cert` y la UI puede prender el MITM más tarde.
            let ca = Arc::new(load(&resolve_data_dir(data_dir)?)?);
            let api = api.then(|| ApiConfig {
                listen: api_listen,
                token: std::env::var("PROXYRR_API_TOKEN")
                    .ok()
                    .filter(|t| !t.is_empty()),
            });
            let template = ProxyConfig {
                connect_timeout: Duration::from_secs(start.connect_timeout.max(1)),
                upstream_roots: read_upstream_cas(&start.upstream_ca)?,
                insecure_upstream: start.insecure_upstream,
                ..ProxyConfig::default()
            };
            if start.insecure_upstream {
                eprintln!(
                    "aviso: --insecure-upstream NO verifica los certificados de los orígenes; \
                     usalo solo en desarrollo local."
                );
            }
            run_start(StartPlan {
                ca,
                settings: ProxySettings {
                    listen: start.listen,
                    mitm: start.mitm,
                    bypass: start.bypass,
                },
                template,
                api,
                har: start.har,
                android: start.android,
                adb: cli.adb,
            })
        }
        Command::Android(cmd) => run_android(cmd, cli.adb.as_deref(), &resolve_data_dir(data_dir)?),
        Command::Ca(ca) => run_ca(ca, &resolve_data_dir(data_dir)?),
        Command::Setup {
            target,
            host,
            port,
            install,
            no_qr,
        } => setup::run_setup(
            &resolve_data_dir(data_dir)?,
            &setup::SetupArgs {
                target: target.into(),
                host,
                port,
                install,
                qr: !no_qr,
            },
        ),
    }
}

fn resolve_data_dir(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    explicit.map_or_else(default_data_dir, Ok)
}

/// Logs del motor por stderr (TD-006). Solo los de ProxyRR; las dependencias, desde `warn`.
fn init_logging(level: LevelFilter) {
    let filter = tracing_subscriber::filter::Targets::new()
        .with_default(LevelFilter::WARN.min(level))
        .with_target("proxyrr", level);
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(io::stderr)
                .with_target(false)
                .with_filter(filter),
        )
        .try_init();
}

/// Lee certificados PEM (uno o varios por archivo) o DER y los devuelve en DER.
fn read_upstream_cas(paths: &[PathBuf]) -> Result<Vec<Vec<u8>>, String> {
    let mut all = Vec::new();
    for path in paths {
        let data =
            fs::read(path).map_err(|e| format!("no se pudo leer {}: {e}", path.display()))?;
        let certs = pem_certificates(&data)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .unwrap_or_else(|| vec![data.clone()]);
        if certs.is_empty() {
            return Err(format!("{}: no tiene ningún certificado", path.display()));
        }
        all.extend(certs);
    }
    Ok(all)
}

/// `Ok(None)` si `data` no es PEM (se trata como DER); error si es PEM pero está roto.
fn pem_certificates(data: &[u8]) -> Result<Option<Vec<Vec<u8>>>, String> {
    use base64::Engine as _;
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let Ok(text) = std::str::from_utf8(data) else {
        return Ok(None);
    };
    if !text.contains(BEGIN) {
        return Ok(None);
    }
    let mut certs = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(BEGIN) {
        let after = &rest[start + BEGIN.len()..];
        let end = after.find(END).ok_or("PEM sin `END CERTIFICATE`")?;
        let body: String = after[..end]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let der = base64::engine::general_purpose::STANDARD
            .decode(body)
            .map_err(|e| format!("PEM inválido: {e}"))?;
        certs.push(der);
        rest = &after[end + END.len()..];
    }
    Ok(Some(certs))
}

fn run_android(
    cmd: AndroidCommand,
    adb_path: Option<&Path>,
    data_dir: &Path,
) -> Result<(), String> {
    let adb = android::adb(adb_path)?;
    match cmd {
        AndroidCommand::Devices => android::print_devices(&adb),
        AndroidCommand::Setup { serial, port } => {
            let ca = load(data_dir)?;
            let device = android::pick(&adb, serial.as_deref())?;
            android::setup(&adb, &device, &ca, port)?;
            println!(
                "Listo. Levantá el proxy con `proxyrr start --mitm --listen 127.0.0.1:{port}` y, al \
                 terminar, `proxyrr android revert` (o usá `proxyrr start --mitm --android`, que \
                 revierte solo al salir)."
            );
            Ok(())
        }
        AndroidCommand::Revert { serial, port } => {
            let device = android::pick(&adb, serial.as_deref())?;
            android::revert_device(&adb, &device, port)
        }
    }
}

/// Todo lo que necesita `proxyrr start`.
struct StartPlan {
    ca: Arc<CertificateAuthority>,
    settings: ProxySettings,
    template: ProxyConfig,
    api: Option<ApiConfig>,
    har: Option<PathBuf>,
    android: Option<String>,
    adb: Option<PathBuf>,
}

fn run_start(plan: StartPlan) -> Result<(), String> {
    let StartPlan {
        ca,
        settings,
        template,
        api,
        har,
        android: android_serial,
        adb: adb_path,
    } = plan;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("no se pudo iniciar el runtime: {e}"))?;
    let mitm_banner = if settings.mitm {
        Some(mitm_banner(&ca, &settings.bypass)?)
    } else {
        None
    };
    // `--android` antes de prender nada: si falla adb, no queda un proxy a medio arrancar.
    let android_target = match &android_serial {
        Some(serial) => {
            let adb = android::adb(adb_path.as_deref())?;
            let device = android::pick(&adb, Some(serial))?;
            Some((adb, device))
        }
        None => None,
    };
    let files = CaFiles::from_ca(&ca).map_err(|e| e.to_string())?;
    let site: Arc<dyn LocalSite> = Arc::new(CertSite::new(files));
    runtime.block_on(async move {
        let engine = Arc::new(Engine::new(EngineOptions {
            ca: Some(Arc::clone(&ca)),
            proxy: ProxyConfig {
                local_site: Some(site),
                local_site_ca: Some(Arc::clone(&ca)),
                ..template
            },
            ..EngineOptions::default()
        }));
        // Suscribir antes de prender: no se pierde ningún flujo.
        let mut events = engine.subscribe_flows();
        let status = engine
            .start_proxy(settings)
            .await
            .map_err(|e| e.to_string())?;
        let api = match api {
            Some(config) => match ApiServer::start(config, Arc::clone(&engine)).await {
                Ok(api) => Some(api),
                Err(e) => {
                    engine.stop_proxy().await;
                    return Err(e.to_string());
                }
            },
            None => None,
        };
        let addr = status
            .listen
            .expect("el proxy recién prendido tiene dirección");
        print_banner(addr, mitm_banner.as_deref(), api.as_ref());
        let configured = match &android_target {
            Some((adb, device)) => match android::setup(adb, device, &ca, addr.port()) {
                Ok(done) => Some(done),
                Err(e) => {
                    eprintln!("error configurando Android: {e}");
                    None
                }
            },
            None => None,
        };

        print_until_ctrl_c(&mut events).await;
        // Primero el dispositivo: sin proxy corriendo se quedaría sin internet.
        if let (Some((adb, _)), Some(done)) = (&android_target, &configured)
            && let Err(e) = android::revert(adb, done, addr.port())
        {
            eprintln!("error quitando el proxy de Android: {e} (`proxyrr android revert`)");
        }
        if let Some(api) = api {
            api.shutdown().await;
        }
        engine.stop_proxy().await;
        if let Some(path) = har {
            let json = serde_json::to_vec_pretty(&engine.har()).map_err(|e| e.to_string())?;
            write_file(&path, &json)?;
            println!(
                "HAR guardado en {} ({} flujos).",
                path.display(),
                engine.store().len()
            );
        }
        println!("ProxyRR detenido.");
        Ok(())
    })
}

/// Imprime una línea por flujo hasta Ctrl+C (o hasta que se cierre el canal).
async fn print_until_ctrl_c(events: &mut tokio::sync::broadcast::Receiver<FlowEvent>) {
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    loop {
        tokio::select! {
            _ = &mut ctrl_c => break,
            event = events.recv() => match event {
                Ok(event) => {
                    if let Some(line) = format_event(&event) {
                        println!("{line}");
                    }
                }
                Err(RecvError::Lagged(n)) => eprintln!("aviso: se omitieron {n} eventos"),
                Err(RecvError::Closed) => break,
            },
        }
    }
}

/// Texto de arranque: dónde escucha, el MITM, la página de la CA y la API.
fn print_banner(addr: SocketAddr, mitm_banner: Option<&str>, api: Option<&ApiServer>) {
    println!("ProxyRR escuchando en {addr} (Ctrl+C para salir)");
    println!("Configurá {addr} como proxy HTTP y HTTPS en tu navegador o SO.");
    match mitm_banner {
        Some(banner) => println!("{banner}"),
        None => println!("HTTPS pasa por túnel sin descifrar; usá --mitm para descifrarlo."),
    }
    println!(
        "Certificado: abrí http://proxyrr.cert en un dispositivo que ya use el proxy, \
         o `proxyrr setup <destino>` para la guía paso a paso."
    );
    if let Some(api) = api {
        println!("API de control: {}  token: {}", api.base_url(), api.token());
    }
    if !addr.ip().is_loopback() {
        eprintln!("aviso: el proxy escucha fuera de loopback; cualquiera en tu red puede usarlo.");
    }
}

/// Texto de arranque con MITM: qué CA se usa y cómo instalarla.
fn mitm_banner(ca: &CertificateAuthority, bypass: &[String]) -> Result<String, String> {
    let info = ca.info().map_err(|e| e.to_string())?;
    let mut banner = format!(
        "Descifrando HTTPS con la CA {} (SHA-256 {}).\n\
         Si el navegador da error de certificado, instalá la CA como raíz de confianza: \
         `proxyrr ca export --out ca.pem`.",
        info.subject, info.sha256_fingerprint
    );
    if !bypass.is_empty() {
        let _ = write!(banner, "\nSin descifrar: {}", bypass.join(", "));
    }
    Ok(banner)
}

/// Una línea por flujo: `#id  MÉTODO  status  destino  tiempo  tamaño|error`. Los túneles descifrados
/// sin error no se muestran: sus requests ya aparecen como `https://…`.
fn format_event(event: &FlowEvent) -> Option<String> {
    let (id, method, status, target, elapsed, extra) = match event {
        FlowEvent::Http(flow) => (
            flow.id,
            flow.method.as_str(),
            flow.status,
            flow.url.as_str(),
            flow.elapsed,
            flow.error
                .clone()
                .or_else(|| flow.content_length.map(format_size))
                .unwrap_or_default(),
        ),
        FlowEvent::Tunnel(flow) if flow.intercepted && flow.error.is_none() => return None,
        // Los bodies no se muestran en la terminal; los consume el store / la API.
        FlowEvent::HttpBodies(_) => return None,
        FlowEvent::Tunnel(flow) => (
            flow.id,
            "CONNECT",
            flow.status,
            flow.authority.as_str(),
            flow.elapsed,
            flow.error
                .clone()
                .unwrap_or_else(|| "túnel sin descifrar".to_owned()),
        ),
    };
    let line = format!(
        "#{id:<5} {method:<7} {status}  {target}  {} ms  {extra}",
        elapsed.as_millis()
    );
    Some(line.trim_end().to_owned())
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    match bytes {
        b if b >= MB => format!("{}.{} MB", b / MB, b % MB * 10 / MB),
        b if b >= KB => format!("{}.{} KB", b / KB, b % KB * 10 / KB),
        b => format!("{b} B"),
    }
}

fn default_data_dir() -> Result<PathBuf, String> {
    proxyrr_cert::default_data_dir()
        .ok_or_else(|| "no se pudo determinar el directorio de datos; usá --data-dir".to_owned())
}

fn run_ca(cmd: CaCommand, data_dir: &Path) -> Result<(), String> {
    match cmd {
        CaCommand::Path => {
            println!("{}", data_dir.display());
            Ok(())
        }
        CaCommand::Install { dry_run } => setup::ca_install(data_dir, dry_run),
        CaCommand::Uninstall { dry_run } => setup::ca_uninstall(data_dir, dry_run),
        CaCommand::Status => setup::ca_status(data_dir),
        CaCommand::Info => {
            let ca = load(data_dir)?;
            let info = ca.info().map_err(|e| e.to_string())?;
            println!("Subject:        {}", info.subject);
            println!("Válida desde:   {}", info.not_before.date());
            println!("Válida hasta:   {}", info.not_after.date());
            println!("SHA-256:        {}", info.sha256_fingerprint);
            println!("Android:        {:08x}.0", info.subject_hash_old);
            println!("Archivo:        {}", data_dir.join(CA_CERT_FILE).display());
            Ok(())
        }
        CaCommand::Export { der, out } => {
            let ca = load(data_dir)?;
            match (der, out) {
                (true, None) => Err("--der requiere --out <ARCHIVO>".to_owned()),
                (true, Some(path)) => write_file(&path, ca.cert_der()),
                (false, Some(path)) => write_file(&path, ca.cert_pem().as_bytes()),
                (false, None) => io::stdout()
                    .write_all(ca.cert_pem().as_bytes())
                    .map_err(|e| e.to_string()),
            }
        }
    }
}

fn load(data_dir: &Path) -> Result<CertificateAuthority, String> {
    CertificateAuthority::load_or_create(data_dir).map_err(|e| e.to_string())
}

fn write_file(path: &Path, data: &[u8]) -> Result<(), String> {
    fs::write(path, data).map_err(|e| format!("no se pudo escribir {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{Cli, format_event, format_size};
    use clap::CommandFactory;
    use proxyrr_core::{FlowEvent, HttpFlow, TunnelFlow};
    use std::time::Duration;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn reads_one_or_many_pem_certificates() {
        let pem = "basura\n-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n\
                   -----BEGIN CERTIFICATE-----\nBAUG\n-----END CERTIFICATE-----\n";
        assert_eq!(
            super::pem_certificates(pem.as_bytes()),
            Ok(Some(vec![vec![1, 2, 3], vec![4, 5, 6]]))
        );
        // Sin cabecera PEM se trata como DER.
        assert_eq!(super::pem_certificates(&[0x30, 0x82]), Ok(None));
        // PEM roto es un error, no un DER.
        assert!(
            super::pem_certificates(b"-----BEGIN CERTIFICATE-----\n!!!\n-----END CERTIFICATE-----")
                .is_err()
        );
    }

    #[test]
    fn start_flags_parse() {
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "proxyrr",
            "--log-level",
            "debug",
            "start",
            "--connect-timeout",
            "45",
            "--upstream-ca",
            "corp.pem",
            "--har",
            "out.har",
            "--android",
        ])
        .unwrap();
        let Some(super::Command::Start { start, .. }) = cli.command else {
            panic!("se esperaba start");
        };
        assert_eq!(start.connect_timeout, 45);
        assert_eq!(start.android.as_deref(), Some("auto"));
        assert_eq!(start.upstream_ca.len(), 1);
    }

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn formats_http_and_tunnel_events() {
        let http = FlowEvent::Http(HttpFlow {
            id: 7,
            method: "GET".into(),
            url: "http://example.com/".into(),
            request_headers: Vec::new(),
            status: 200,
            response_headers: Vec::new(),
            error: None,
            elapsed: Duration::from_millis(42),
            content_length: Some(2048),
        });
        assert_eq!(
            format_event(&http).as_deref(),
            Some("#7     GET     200  http://example.com/  42 ms  2.0 KB")
        );
        let tunnel = FlowEvent::Tunnel(TunnelFlow {
            id: 8,
            authority: "example.com:443".into(),
            status: 502,
            intercepted: false,
            error: Some("sin conexión".into()),
            elapsed: Duration::from_millis(3),
        });
        assert_eq!(
            format_event(&tunnel).as_deref(),
            Some("#8     CONNECT 502  example.com:443  3 ms  sin conexión")
        );
    }

    #[test]
    fn hides_successful_intercepted_tunnels_only() {
        let tunnel = |error: Option<&str>| {
            FlowEvent::Tunnel(TunnelFlow {
                id: 1,
                authority: "example.com:443".into(),
                status: 200,
                intercepted: true,
                error: error.map(Into::into),
                elapsed: Duration::from_millis(1),
            })
        };
        assert_eq!(format_event(&tunnel(None)), None);
        let failed = format_event(&tunnel(Some("el cliente no confía en la CA"))).unwrap();
        assert!(failed.contains("no confía en la CA"), "{failed}");
    }
}
