//! `proxyrr`: interfaz de línea de comandos de ProxyRR.

use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use proxyrr_cert::{CA_CERT_FILE, CertificateAuthority};
use proxyrr_core::{FlowEvent, Proxy, ProxyConfig};
use tokio::sync::broadcast::error::RecvError;

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
    },
    /// Autoridad certificante de ProxyRR.
    #[command(subcommand)]
    Ca(CaCommand),
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
    match command {
        Command::Start { listen } => run_start(listen),
        Command::Ca(ca) => {
            let data_dir = match cli.data_dir {
                Some(dir) => dir,
                None => default_data_dir()?,
            };
            run_ca(ca, &data_dir)
        }
    }
}

fn run_start(listen: SocketAddr) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("no se pudo iniciar el runtime: {e}"))?;
    runtime.block_on(async move {
        let proxy = Proxy::start(ProxyConfig { listen })
            .await
            .map_err(|e| format!("no se pudo escuchar en {listen}: {e}"))?;
        // Suscribir antes de anunciar la dirección: no se pierde ningún flujo.
        let mut events = proxy.subscribe();
        let addr = proxy.local_addr();
        println!("ProxyRR escuchando en {addr} (Ctrl+C para salir)");
        println!(
            "Configurá {addr} como proxy HTTP y HTTPS en tu navegador o SO. \
             HTTPS pasa por túnel sin descifrar (el descifrado llega en la spec https-mitm)."
        );
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
                    Ok(event) => println!("{}", format_event(&event)),
                    Err(RecvError::Lagged(n)) => eprintln!("aviso: se omitieron {n} eventos"),
                    Err(RecvError::Closed) => break,
                },
            }
        }
        proxy.shutdown().await;
        println!("ProxyRR detenido.");
        Ok(())
    })
}

/// Una línea por flujo: `#id  MÉTODO  status  destino  tiempo  tamaño|error`.
fn format_event(event: &FlowEvent) -> String {
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
    format!(
        "#{id:<5} {method:<7} {status}  {target}  {} ms  {extra}",
        elapsed.as_millis()
    )
    .trim_end()
    .to_owned()
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
            status: 200,
            error: None,
            elapsed: Duration::from_millis(42),
            content_length: Some(2048),
        });
        assert_eq!(
            format_event(&http),
            "#7     GET     200  http://example.com/  42 ms  2.0 KB"
        );
        let tunnel = FlowEvent::Tunnel(TunnelFlow {
            id: 8,
            authority: "example.com:443".into(),
            status: 502,
            error: Some("sin conexión".into()),
            elapsed: Duration::from_millis(3),
        });
        assert_eq!(
            format_event(&tunnel),
            "#8     CONNECT 502  example.com:443  3 ms  sin conexión"
        );
    }
}
