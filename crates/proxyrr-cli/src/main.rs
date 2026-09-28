//! `proxyrr`: interfaz de línea de comandos de ProxyRR.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use proxyrr_cert::{CA_CERT_FILE, CertificateAuthority};

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
    let data_dir = match cli.data_dir {
        Some(dir) => dir,
        None => default_data_dir()?,
    };
    match command {
        Command::Ca(ca) => run_ca(ca, &data_dir),
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
    use super::Cli;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
