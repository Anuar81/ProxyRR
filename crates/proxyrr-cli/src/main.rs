//! `proxyrr`: interfaz de línea de comandos de ProxyRR.

use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use proxyrr_api::{ApiConfig, ApiServer, Engine, EngineOptions, ProxySettings};
use proxyrr_cert::{CA_CERT_FILE, CertificateAuthority};
use proxyrr_core::{FlowEvent, LocalSite, ProxyConfig};
use proxyrr_devices::{CaFiles, CertSite};
use tokio::sync::broadcast::error::RecvError;

mod setup;

/// Proxy HTTP(S) de depuración multiplataforma.
#[derive(Debug, Parser)]
#[command(name = "proxyrr", version, about)]
struct Cli {
    /// Directorio de datos (CA, sesiones). Por defecto, el del usuario para ProxyRR.
    #[arg(long, global = true, value_name = "DIR")]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Levanta el proxy y muestra cada request en la terminal.
    Start {
        /// Dirección de escucha. Usá 0.0.0.0:9090 para aceptar dispositivos de tu red.
        #[arg(long, value_name = "IP:PUERTO", default_value_t = ProxyConfig::default().listen)]
        listen: SocketAddr,
        /// Descifrar HTTPS con la CA de ProxyRR (hay que instalarla como raíz de confianza).
        #[arg(long)]
        mitm: bool,
        /// Host que no se descifra (repetible): `example.com` o `*.example.com` para subdominios.
        #[arg(long, value_name = "HOST", requires = "mitm")]
        bypass: Vec<String>,
        /// Levantar también la API de control local (la usa la app de escritorio y scripts).
        /// El token se imprime al arrancar; `PROXYRR_API_TOKEN` lo fija.
        #[arg(long)]
        api: bool,
        /// Dirección de la API (solo loopback).
        #[arg(long, value_name = "IP:PUERTO", default_value_t = ApiConfig::default().listen, requires = "api")]
        api_listen: SocketAddr,
    },
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
    match command {
        Command::Start {
            listen,
            mitm,
            bypass,
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
            run_start(
                ca,
                ProxySettings {
                    listen,
                    mitm,
                    bypass,
                },
                api,
            )
        }
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

fn run_start(
    ca: Arc<CertificateAuthority>,
    settings: ProxySettings,
    api: Option<ApiConfig>,
) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("no se pudo iniciar el runtime: {e}"))?;
    let mitm_banner = if settings.mitm {
        Some(mitm_banner(&ca, &settings.bypass)?)
    } else {
        None
    };
    let files = CaFiles::from_ca(&ca).map_err(|e| e.to_string())?;
    let site: Arc<dyn LocalSite> = Arc::new(CertSite::new(files));
    runtime.block_on(async move {
        let engine = Arc::new(Engine::new(EngineOptions {
            ca: Some(ca),
            proxy: ProxyConfig {
                local_site: Some(site),
                ..ProxyConfig::default()
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
        println!("ProxyRR escuchando en {addr} (Ctrl+C para salir)");
        println!("Configurá {addr} como proxy HTTP y HTTPS en tu navegador o SO.");
        match &mitm_banner {
            Some(banner) => println!("{banner}"),
            None => println!("HTTPS pasa por túnel sin descifrar; usá --mitm para descifrarlo."),
        }
        println!(
            "Certificado: abrí http://proxyrr.cert en un dispositivo que ya use el proxy, \
             o `proxyrr setup <destino>` para la guía paso a paso."
        );
        if let Some(api) = &api {
            println!("API de control: {}  token: {}", api.base_url(), api.token());
        }
        if !addr.ip().is_loopback() {
            eprintln!(
                "aviso: el proxy escucha fuera de loopback; cualquiera en tu red puede usarlo."
            );
        }

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
        if let Some(api) = api {
            api.shutdown().await;
        }
        engine.stop_proxy().await;
        println!("ProxyRR detenido.");
        Ok(())
    })
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

/// Directorio de datos por SO, sin dependencias externas:
/// Windows `%APPDATA%\ProxyRR`, macOS `~/Library/Application Support/ProxyRR`,
/// Linux/otros `$XDG_DATA_HOME/proxyrr` o `~/.local/share/proxyrr`.
fn default_data_dir() -> Result<PathBuf, String> {
    let env_dir = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let dir = if cfg!(windows) {
        env_dir("APPDATA").map(|d| d.join("ProxyRR"))
    } else if cfg!(target_os = "macos") {
        env_dir("HOME").map(|h| h.join("Library/Application Support/ProxyRR"))
    } else {
        env_dir("XDG_DATA_HOME")
            .or_else(|| env_dir("HOME").map(|h| h.join(".local/share")))
            .map(|d| d.join("proxyrr"))
    };
    dir.ok_or_else(|| "no se pudo determinar el directorio de datos; usá --data-dir".to_owned())
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
