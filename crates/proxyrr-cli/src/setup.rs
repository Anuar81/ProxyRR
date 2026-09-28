//! `proxyrr ca install|uninstall|status` y `proxyrr setup <destino>` (spec 0006).

use std::path::{Path, PathBuf};

use clap::ValueEnum;
use proxyrr_cert::CA_CERT_FILE;
use proxyrr_devices::guide::{self, GuideContext, Target};
use proxyrr_devices::trust::{self, LinuxTools, Os, Step, SystemRunner};
use proxyrr_devices::{CaFiles, lan_ip, qr_text};

use crate::load;

/// Destinos de `proxyrr setup`, tal como se escriben en la línea de comandos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum TargetArg {
    /// iPhone / iPad físico.
    Ios,
    /// Simulador de iOS (macOS).
    IosSimulator,
    /// Emulador de Android.
    AndroidEmulator,
    /// Android físico.
    AndroidDevice,
    /// Este equipo con Windows.
    Windows,
    /// Este equipo con macOS.
    Macos,
    /// Este equipo con Linux.
    Linux,
}

impl From<TargetArg> for Target {
    fn from(t: TargetArg) -> Self {
        match t {
            TargetArg::Ios => Self::Ios,
            TargetArg::IosSimulator => Self::IosSimulator,
            TargetArg::AndroidEmulator => Self::AndroidEmulator,
            TargetArg::AndroidDevice => Self::AndroidDevice,
            TargetArg::Windows => Self::Windows,
            TargetArg::Macos => Self::Macos,
            TargetArg::Linux => Self::Linux,
        }
    }
}

/// Argumentos de `proxyrr setup`.
#[derive(Debug)]
pub(crate) struct SetupArgs {
    pub target: Target,
    pub host: Option<String>,
    pub port: u16,
    pub install: bool,
    pub qr: bool,
}

/// La CA de `data_dir` (se crea si no existe), sus archivos y la ruta del PEM.
fn ca_files(data_dir: &Path) -> Result<(CaFiles, PathBuf), String> {
    let ca = load(data_dir)?;
    let files = CaFiles::from_ca(&ca).map_err(|e| e.to_string())?;
    Ok((files, data_dir.join(CA_CERT_FILE)))
}

fn linux_tools(os: Os) -> LinuxTools {
    if os == Os::Linux {
        LinuxTools::detect()
    } else {
        LinuxTools::default()
    }
}

fn print_plan(plan: &[Step]) {
    for step in plan {
        println!("  {}\n    $ {}", step.what, step.command_line());
    }
}

pub(crate) fn ca_install(data_dir: &Path, dry_run: bool) -> Result<(), String> {
    let (files, pem) = ca_files(data_dir)?;
    let os = Os::current();
    let plan = trust::install_plan(os, &pem, &files, &linux_tools(os))?;
    println!("{}\n", trust::INSTALL_WARNING);
    println!("Instalar {} (SHA-256 {}):", files.name, files.sha256);
    print_plan(&plan);
    if dry_run {
        println!("\n--dry-run: no se ejecutó nada.");
        return Ok(());
    }
    trust::apply(&plan, &SystemRunner)?;
    println!("\nListo: {} es de confianza en este equipo.", files.name);
    if let Some(note) = trust::firefox_note(os) {
        println!("{note}");
    }
    Ok(())
}

pub(crate) fn ca_uninstall(data_dir: &Path, dry_run: bool) -> Result<(), String> {
    let (files, pem) = ca_files(data_dir)?;
    let os = Os::current();
    let plan = trust::uninstall_plan(os, &pem, &files, &linux_tools(os))?;
    println!("Quitar {} (SHA-1 {}):", files.name, files.sha1);
    print_plan(&plan);
    if dry_run {
        println!("\n--dry-run: no se ejecutó nada.");
        return Ok(());
    }
    trust::apply(&plan, &SystemRunner)?;
    println!(
        "\nListo: {} ya no es de confianza en este equipo.",
        files.name
    );
    Ok(())
}

pub(crate) fn ca_status(data_dir: &Path) -> Result<(), String> {
    let (files, pem) = ca_files(data_dir)?;
    let os = Os::current();
    println!("{} (SHA-256 {})", files.name, files.sha256);
    let results = trust::status(os, &pem, &files, &linux_tools(os), &SystemRunner);
    for s in &results {
        match (s.installed, &s.detail) {
            (Some(true), _) => println!("  [x] {}", s.store),
            (Some(false), _) => println!("  [ ] {}", s.store),
            (None, detail) => println!(
                "  [?] {}: no se pudo verificar ({})",
                s.store,
                detail.as_deref().unwrap_or("sin detalle")
            ),
        }
    }
    if results.iter().all(|s| s.installed != Some(true)) {
        println!("No está instalada: `proxyrr ca install`.");
    }
    if let Some(note) = trust::firefox_note(os) {
        println!("{note}");
    }
    Ok(())
}

pub(crate) fn run_setup(data_dir: &Path, args: &SetupArgs) -> Result<(), String> {
    let (files, pem) = ca_files(data_dir)?;
    let lan_host = args
        .host
        .clone()
        .or_else(|| lan_ip().map(|ip| ip.to_string()))
        .unwrap_or_else(|| "<IP-de-esta-máquina>".to_owned());
    let g = guide::guide(
        args.target,
        &GuideContext {
            files: &files,
            lan_host,
            port: args.port,
        },
    );
    print!("{}", guide::render(&g));
    if args.qr
        && let Some(url) = &g.qr_url
        && let Some(qr) = qr_text(url)
    {
        println!("\nEscaneá con la cámara del teléfono para abrir {url}:\n{qr}");
    }
    if !args.install {
        return Ok(());
    }
    let os = Os::current();
    match (args.target, os) {
        (Target::IosSimulator, Os::Macos) => {
            let step = ios_simulator_step(&pem);
            println!("\nInstalando en el simulador que está abierto:");
            print_plan(std::slice::from_ref(&step));
            trust::apply(&[step], &SystemRunner)?;
            println!("Listo. Reiniciá la app en el simulador.");
            Ok(())
        }
        (Target::IosSimulator, _) => Err("el simulador de iOS solo existe en macOS".to_owned()),
        (Target::Windows, Os::Windows) | (Target::Macos, Os::Macos) | (Target::Linux, Os::Linux) => {
            println!();
            ca_install(data_dir, false)
        }
        (Target::AndroidEmulator, _) => Err(
            "la instalación automática en el emulador llega con la spec 0002; por ahora seguí los pasos de arriba"
                .to_owned(),
        ),
        _ => Err(format!(
            "`--install` no aplica a {}: en dispositivos físicos la instalación la confirma el usuario en el propio aparato",
            args.target
        )),
    }
}

fn ios_simulator_step(pem: &Path) -> Step {
    Step {
        what: "Agregar la CA al simulador de iOS abierto".to_owned(),
        program: "xcrun".to_owned(),
        args: vec![
            "simctl".to_owned(),
            "keychain".to_owned(),
            "booted".to_owned(),
            "add-root-cert".to_owned(),
            pem.display().to_string(),
        ],
        sudo: false,
    }
}
