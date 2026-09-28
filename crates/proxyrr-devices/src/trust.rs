//! Instalar, quitar y verificar la CA en el almacén de confianza de este equipo (spec 0006, CA 1–5).
//!
//! Cada operación se arma como un plan de [`Step`]s (comandos del SO) que se puede mostrar sin correr
//! (`--dry-run`) y probar en cualquier SO. [`Runner`] los ejecuta; los tests usan uno falso.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::files::CaFiles;

/// Sistema operativo del almacén.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Windows: `CurrentUser\Root`, sin admin.
    Windows,
    /// macOS: llavero de inicio de sesión con confianza SSL.
    Macos,
    /// Linux: store del sistema (sudo) + NSS del usuario.
    Linux,
}

impl Os {
    /// El SO en que corre el binario.
    #[must_use]
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Linux
        }
    }
}

/// Un comando del plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// Qué hace, para el usuario.
    pub what: String,
    /// Programa.
    pub program: String,
    /// Argumentos.
    pub args: Vec<String>,
    /// Necesita `sudo` (solo Linux, y solo si no se corre como root).
    pub sudo: bool,
}

impl Step {
    fn new(what: &str, program: &str, args: &[&str]) -> Self {
        Self {
            what: what.to_owned(),
            program: program.to_owned(),
            args: args.iter().map(|&a| a.to_owned()).collect(),
            sudo: false,
        }
    }

    fn sudo(mut self) -> Self {
        self.sudo = true;
        self
    }

    /// Línea de comando para mostrar (con comillas donde hace falta).
    #[must_use]
    pub fn command_line(&self) -> String {
        let quote = |s: &str| {
            if s.is_empty() || s.contains([' ', '"', '\'']) {
                format!("\"{}\"", s.replace('"', "\\\""))
            } else {
                s.to_owned()
            }
        };
        let mut parts = Vec::new();
        if self.sudo {
            parts.push("sudo".to_owned());
        }
        parts.push(quote(&self.program));
        parts.extend(self.args.iter().map(|a| quote(a)));
        parts.join(" ")
    }
}

/// Herramientas presentes en Linux (se detectan una vez; en tests se fijan a mano).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinuxTools {
    /// `update-ca-certificates` (Debian, Ubuntu).
    pub update_ca_certificates: bool,
    /// `trust` de p11-kit (Fedora, Arch, openSUSE).
    pub trust: bool,
    /// Base NSS del usuario (`~/.pki/nssdb`) con `certutil` disponible.
    pub nss_db: Option<PathBuf>,
}

impl LinuxTools {
    /// Detecta lo que hay en el PATH y en `$HOME`.
    #[must_use]
    pub fn detect() -> Self {
        let nss_db = home()
            .map(|h| h.join(".pki/nssdb"))
            .filter(|db| db.is_dir() && in_path("certutil"));
        Self {
            update_ca_certificates: in_path("update-ca-certificates"),
            trust: in_path("trust"),
            nss_db,
        }
    }
}

/// Dónde deja Debian/Ubuntu la CA; el nombre lleva el id de la CA para no pisar otras.
fn debian_path(files: &CaFiles) -> String {
    format!(
        "/usr/local/share/ca-certificates/proxyrr-{}.crt",
        files.id()
    )
}

fn macos_keychain() -> String {
    home().map_or_else(
        || "login.keychain-db".to_owned(),
        |h| {
            h.join("Library/Keychains/login.keychain-db")
                .display()
                .to_string()
        },
    )
}

/// Plan para instalar la CA (`pem` es el archivo en disco).
///
/// # Errors
/// En Linux, si no hay ninguna herramienta de store del sistema conocida.
pub fn install_plan(
    os: Os,
    pem: &Path,
    files: &CaFiles,
    linux: &LinuxTools,
) -> Result<Vec<Step>, String> {
    let pem = pem.display().to_string();
    Ok(match os {
        Os::Windows => vec![Step::new(
            "Agregar la CA a las raíces de confianza de tu usuario (CurrentUser\\Root)",
            "certutil",
            &["-user", "-addstore", "Root", &pem],
        )],
        Os::Macos => vec![Step::new(
            "Agregar la CA al llavero de inicio de sesión con confianza SSL",
            "security",
            &[
                "add-trusted-cert",
                "-r",
                "trustRoot",
                "-p",
                "ssl",
                "-k",
                &macos_keychain(),
                &pem,
            ],
        )],
        Os::Linux => {
            let mut steps = linux_system_install(&pem, files, linux)?;
            if let Some(db) = &linux.nss_db {
                let db = format!("sql:{}", db.display());
                steps.push(Step::new(
                    "Agregar la CA a la base NSS de tu usuario (Chrome/Chromium)",
                    "certutil",
                    &["-d", &db, "-A", "-t", "C,,", "-n", &files.name, "-i", &pem],
                ));
            }
            steps
        }
    })
}

fn linux_system_install(
    pem: &str,
    files: &CaFiles,
    linux: &LinuxTools,
) -> Result<Vec<Step>, String> {
    if linux.update_ca_certificates {
        let target = debian_path(files);
        Ok(vec![
            Step::new(
                "Copiar la CA al store del sistema",
                "install",
                &["-m", "644", pem, &target],
            )
            .sudo(),
            Step::new(
                "Regenerar el bundle del sistema",
                "update-ca-certificates",
                &[],
            )
            .sudo(),
        ])
    } else if linux.trust {
        Ok(vec![
            Step::new(
                "Agregar la CA al store del sistema (p11-kit)",
                "trust",
                &["anchor", "--store", pem],
            )
            .sudo(),
        ])
    } else {
        Err("no encontré `update-ca-certificates` ni `trust` (p11-kit): instalá el paquete ca-certificates o p11-kit".to_owned())
    }
}

/// Plan para quitar exactamente esta CA (por huella o por su nombre único, nunca por un nombre genérico).
///
/// # Errors
/// En Linux, si no hay ninguna herramienta de store del sistema conocida.
pub fn uninstall_plan(
    os: Os,
    pem: &Path,
    files: &CaFiles,
    linux: &LinuxTools,
) -> Result<Vec<Step>, String> {
    let pem = pem.display().to_string();
    Ok(match os {
        Os::Windows => vec![Step::new(
            "Quitar la CA (por huella SHA-1) de CurrentUser\\Root",
            "certutil",
            &["-user", "-delstore", "Root", &files.sha1],
        )],
        Os::Macos => vec![
            Step::new(
                "Quitar la confianza SSL de la CA",
                "security",
                &["remove-trusted-cert", &pem],
            ),
            Step::new(
                "Borrar la CA (por huella SHA-1) del llavero",
                "security",
                &["delete-certificate", "-Z", &files.sha1, &macos_keychain()],
            ),
        ],
        Os::Linux => {
            let mut steps = if linux.update_ca_certificates {
                vec![
                    Step::new(
                        "Borrar la CA del store del sistema",
                        "rm",
                        &["-f", &debian_path(files)],
                    )
                    .sudo(),
                    Step::new(
                        "Regenerar el bundle del sistema",
                        "update-ca-certificates",
                        &["--fresh"],
                    )
                    .sudo(),
                ]
            } else if linux.trust {
                vec![
                    Step::new(
                        "Quitar la CA del store del sistema (p11-kit)",
                        "trust",
                        &["anchor", "--remove", &pem],
                    )
                    .sudo(),
                ]
            } else {
                return Err("no encontré `update-ca-certificates` ni `trust` (p11-kit)".to_owned());
            };
            if let Some(db) = &linux.nss_db {
                let db = format!("sql:{}", db.display());
                steps.push(Step::new(
                    "Quitar la CA de la base NSS de tu usuario",
                    "certutil",
                    &["-d", &db, "-D", "-n", &files.name],
                ));
            }
            steps
        }
    })
}

/// Resultado de correr un comando.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutput {
    /// Terminó con código 0.
    pub success: bool,
    /// Salida estándar.
    pub stdout: String,
    /// Salida de error.
    pub stderr: String,
}

/// Ejecuta pasos. El real usa el SO; los tests, uno falso.
pub trait Runner {
    /// Corre `step` heredando la terminal (para que `sudo` o el SO puedan pedir confirmación).
    ///
    /// # Errors
    /// Si el programa no existe o no se pudo lanzar.
    fn run_interactive(&self, step: &Step) -> io::Result<bool>;
    /// Corre `step` capturando la salida (para verificar sin mostrar nada).
    ///
    /// # Errors
    /// Si el programa no existe o no se pudo lanzar.
    fn run_captured(&self, step: &Step) -> io::Result<RunOutput>;
    /// Lee un archivo (para revisar los bundles del sistema en Linux).
    fn read_file(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
}

/// Runner que ejecuta de verdad.
#[derive(Debug, Clone, Copy)]
pub struct SystemRunner {
    /// Programa para elevar los pasos `sudo` en Linux: `sudo` en la terminal, `pkexec` en la app
    /// (no hay terminal donde pedir la contraseña; `pkexec` muestra su propio diálogo).
    elevator: &'static str,
}

impl Default for SystemRunner {
    fn default() -> Self {
        Self { elevator: "sudo" }
    }
}

impl SystemRunner {
    /// Runner para apps gráficas: eleva con `pkexec`.
    #[must_use]
    pub fn graphical() -> Self {
        Self { elevator: "pkexec" }
    }

    fn command(&self, step: &Step) -> Command {
        // `sudo` solo existe en los planes de Linux (Windows y macOS instalan para el usuario, sin
        // elevación); el `cfg!` lo deja explícito aunque un plan futuro marcara `sudo` por error.
        if step.sudo && cfg!(target_os = "linux") && !is_root() {
            let mut cmd = Command::new(self.elevator);
            cmd.arg(&step.program).args(&step.args);
            cmd
        } else {
            let mut cmd = Command::new(&step.program);
            cmd.args(&step.args);
            cmd
        }
    }
}

impl Runner for SystemRunner {
    fn run_interactive(&self, step: &Step) -> io::Result<bool> {
        Ok(self.command(step).status()?.success())
    }

    fn run_captured(&self, step: &Step) -> io::Result<RunOutput> {
        let out = self.command(step).output()?;
        Ok(RunOutput {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// Corre un plan en orden; corta en el primer paso que falla.
///
/// # Errors
/// El paso que falló y por qué.
pub fn apply(plan: &[Step], runner: &dyn Runner) -> Result<(), String> {
    for step in plan {
        match runner.run_interactive(step) {
            Ok(true) => {}
            Ok(false) => return Err(format!("falló: {} (`{}`)", step.what, step.command_line())),
            Err(e) => return Err(format!("no se pudo correr `{}`: {e}", step.program)),
        }
    }
    Ok(())
}

/// Estado de la CA en un almacén.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreStatus {
    /// Nombre del almacén.
    pub store: String,
    /// `Some(true)` instalada (y de confianza), `Some(false)` no, `None` no se pudo verificar.
    pub installed: Option<bool>,
    /// Detalle si no se pudo verificar.
    pub detail: Option<String>,
}

/// Bundles de CAs del sistema en Linux: basta con que alguno tenga la CA.
const LINUX_BUNDLES: [&str; 4] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/cert.pem",
    "/etc/ca-certificates/extracted/tls-ca-bundle.pem",
];

/// Verifica en qué almacenes está la CA (CA 3). Solo lee: no cambia nada.
#[must_use]
pub fn status(
    os: Os,
    pem: &Path,
    files: &CaFiles,
    linux: &LinuxTools,
    runner: &dyn Runner,
) -> Vec<StoreStatus> {
    let probe = |store: &str, step: Step, check: &dyn Fn(&RunOutput) -> bool| match runner
        .run_captured(&step)
    {
        Ok(out) => StoreStatus {
            store: store.to_owned(),
            installed: Some(check(&out)),
            detail: None,
        },
        Err(e) => StoreStatus {
            store: store.to_owned(),
            installed: None,
            detail: Some(format!("`{}`: {e}", step.program)),
        },
    };
    match os {
        Os::Windows => vec![probe(
            "Windows CurrentUser\\Root",
            Step::new(
                "buscar",
                "certutil",
                &["-user", "-verifystore", "Root", &files.sha1],
            ),
            &|out| out.success,
        )],
        Os::Macos => {
            let sha1 = files.sha1.clone();
            vec![
                probe(
                    "llavero de inicio de sesión",
                    Step::new(
                        "buscar",
                        "security",
                        &["find-certificate", "-a", "-Z", &macos_keychain()],
                    ),
                    &move |out| out.success && out.stdout.to_ascii_uppercase().contains(&sha1),
                ),
                probe(
                    "confianza SSL",
                    Step::new(
                        "verificar",
                        "security",
                        &[
                            "verify-cert",
                            "-c",
                            &pem.display().to_string(),
                            "-p",
                            "ssl",
                            "-L",
                        ],
                    ),
                    &|out| out.success,
                ),
            ]
        }
        Os::Linux => {
            let body = pem_body(&files.pem);
            let found = LINUX_BUNDLES
                .iter()
                .filter_map(|p| runner.read_file(Path::new(p)))
                .any(|bundle| {
                    bundle
                        .split_whitespace()
                        .collect::<String>()
                        .contains(&body)
                });
            let mut out = vec![StoreStatus {
                store: "store del sistema".to_owned(),
                installed: Some(found),
                detail: None,
            }];
            if let Some(db) = &linux.nss_db {
                let db = format!("sql:{}", db.display());
                out.push(probe(
                    "NSS del usuario (Chrome)",
                    Step::new("buscar", "certutil", &["-d", &db, "-L", "-n", &files.name]),
                    &|o| o.success,
                ));
            }
            out
        }
    }
}

/// Contenido base64 del PEM sin cortes de línea, para buscarlo dentro de un bundle.
fn pem_body(pem: &str) -> String {
    pem.lines()
        .filter(|l| !l.starts_with("-----"))
        .flat_map(str::split_whitespace)
        .collect()
}

/// Aviso sobre Firefox si está instalado para este usuario (CA 4).
#[must_use]
pub fn firefox_note(os: Os) -> Option<String> {
    let profiles = match os {
        Os::Windows => {
            std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Mozilla/Firefox/Profiles"))
        }
        Os::Macos => home().map(|h| h.join("Library/Application Support/Firefox/Profiles")),
        Os::Linux => home().map(|h| h.join(".mozilla/firefox")),
    }?;
    let snap = home().map(|h| h.join("snap/firefox/common/.mozilla/firefox"));
    if !profiles.is_dir() && !snap.is_some_and(|s| s.is_dir()) {
        return None;
    }
    Some(match os {
        Os::Windows | Os::Macos => "Firefox: usa las raíces del sistema si `security.enterprise_roots.enabled` está en true (por defecto en versiones recientes). Si ves errores de certificado, activalo en about:config o importá ca.pem en Ajustes → Privacidad y seguridad → Certificados.".to_owned(),
        Os::Linux => "Firefox en Linux usa su propio almacén: importá la CA en Ajustes → Privacidad y seguridad → Certificados → Ver certificados → Autoridades → Importar (marcá \"confiar para sitios web\"). `proxyrr ca export --out ca.pem` te da el archivo.".to_owned(),
    })
}

/// Aviso previo a instalar (CA 5).
pub const INSTALL_WARNING: &str = "Vas a confiar en la CA de ProxyRR: quien tenga su clave privada (está en tu directorio de datos) puede leer tu HTTPS. Desinstalala con `proxyrr ca uninstall` cuando no la uses.";

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn in_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// `true` si el proceso ya es root (Linux): se lee de `/proc` para no depender de libc.
fn is_root() -> bool {
    std::fs::read_to_string("/proc/self/status").is_ok_and(|s| {
        s.lines()
            .find_map(|l| l.strip_prefix("Uid:"))
            .and_then(|l| l.split_whitespace().nth(1))
            == Some("0")
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;
    use crate::files::tests::files;

    fn debian() -> LinuxTools {
        LinuxTools {
            update_ca_certificates: true,
            trust: false,
            nss_db: Some(PathBuf::from("/home/u/.pki/nssdb")),
        }
    }

    #[test]
    fn windows_installs_for_the_user_and_removes_by_thumbprint() {
        let f = files();
        let install = install_plan(
            Os::Windows,
            Path::new("C:\\d\\ca.pem"),
            &f,
            &LinuxTools::default(),
        )
        .unwrap();
        assert_eq!(
            install[0].command_line(),
            "certutil -user -addstore Root C:\\d\\ca.pem"
        );
        let remove =
            uninstall_plan(Os::Windows, Path::new("ca.pem"), &f, &LinuxTools::default()).unwrap();
        assert_eq!(
            remove[0].args,
            ["-user", "-delstore", "Root", f.sha1.as_str()]
        );
        assert!(install.iter().chain(&remove).all(|s| !s.sudo), "sin admin");
    }

    #[test]
    fn macos_uses_the_login_keychain_with_ssl_trust() {
        let f = files();
        let install = install_plan(
            Os::Macos,
            Path::new("/d/ca.pem"),
            &f,
            &LinuxTools::default(),
        )
        .unwrap();
        let line = install[0].command_line();
        assert!(
            line.starts_with("security add-trusted-cert -r trustRoot -p ssl -k "),
            "{line}"
        );
        assert!(
            line.contains("login.keychain-db") && line.ends_with("/d/ca.pem"),
            "{line}"
        );
        let remove = uninstall_plan(
            Os::Macos,
            Path::new("/d/ca.pem"),
            &f,
            &LinuxTools::default(),
        )
        .unwrap();
        assert!(
            remove.iter().any(|s| s.args.contains(&f.sha1)),
            "borra por huella"
        );
        assert!(
            install.iter().chain(&remove).all(|s| !s.sudo),
            "sin sudo en macOS"
        );
    }

    #[test]
    fn linux_system_store_needs_sudo_and_nss_does_not() {
        let f = files();
        let install = install_plan(Os::Linux, Path::new("/d/ca.pem"), &f, &debian()).unwrap();
        assert!(install[0].sudo && install[1].sudo);
        assert!(
            install[0]
                .args
                .last()
                .unwrap()
                .contains(&format!("proxyrr-{}.crt", f.id()))
        );
        assert_eq!(install[1].program, "update-ca-certificates");
        let nss = install.last().unwrap();
        assert!(!nss.sudo && nss.program == "certutil" && nss.args.contains(&f.name));
        assert!(
            nss.command_line().contains(&format!("\"{}\"", f.name)),
            "nombre con espacios entre comillas"
        );

        let fedora = LinuxTools {
            trust: true,
            ..LinuxTools::default()
        };
        let plan = install_plan(Os::Linux, Path::new("/d/ca.pem"), &f, &fedora).unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(
            plan[0].command_line(),
            "sudo trust anchor --store /d/ca.pem"
        );

        assert!(
            install_plan(
                Os::Linux,
                Path::new("/d/ca.pem"),
                &f,
                &LinuxTools::default()
            )
            .is_err()
        );
    }

    /// Runner falso: respuestas por programa, y archivos en memoria.
    #[derive(Default)]
    struct Fake {
        outputs: HashMap<&'static str, RunOutput>,
        files: HashMap<String, String>,
        ran: RefCell<Vec<String>>,
    }

    impl Runner for Fake {
        fn run_interactive(&self, step: &Step) -> io::Result<bool> {
            self.ran.borrow_mut().push(step.command_line());
            Ok(self
                .outputs
                .get(step.program.as_str())
                .is_none_or(|o| o.success))
        }
        fn run_captured(&self, step: &Step) -> io::Result<RunOutput> {
            self.outputs
                .get(step.program.as_str())
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no está"))
        }
        fn read_file(&self, path: &Path) -> Option<String> {
            self.files.get(&path.display().to_string()).cloned()
        }
    }

    #[test]
    fn status_per_os() {
        let f = files();
        let mut fake = Fake::default();
        fake.outputs.insert(
            "certutil",
            RunOutput {
                success: true,
                ..RunOutput::default()
            },
        );
        let win = status(
            Os::Windows,
            Path::new("ca.pem"),
            &f,
            &LinuxTools::default(),
            &fake,
        );
        assert_eq!(win[0].installed, Some(true));

        let mut fake = Fake::default();
        fake.outputs.insert(
            "security",
            RunOutput {
                success: true,
                stdout: format!("SHA-1 hash: {}\n", f.sha1.to_ascii_lowercase()),
                ..RunOutput::default()
            },
        );
        let mac = status(
            Os::Macos,
            Path::new("ca.pem"),
            &f,
            &LinuxTools::default(),
            &fake,
        );
        assert!(mac.iter().all(|s| s.installed == Some(true)), "{mac:?}");

        let missing = status(
            Os::Windows,
            Path::new("ca.pem"),
            &f,
            &LinuxTools::default(),
            &Fake::default(),
        );
        assert_eq!(missing[0].installed, None, "sin certutil no se sabe");
        assert!(missing[0].detail.is_some());
    }

    #[test]
    fn linux_status_finds_the_ca_inside_a_bundle() {
        let f = files();
        let mut fake = Fake::default();
        let tools = LinuxTools::default();
        assert_eq!(
            status(Os::Linux, Path::new("ca.pem"), &f, &tools, &fake)[0].installed,
            Some(false)
        );
        fake.files.insert(
            "/etc/ssl/certs/ca-certificates.crt".into(),
            format!(
                "-----BEGIN CERTIFICATE-----\nOTRA\n-----END CERTIFICATE-----\n{}",
                f.pem
            ),
        );
        assert_eq!(
            status(Os::Linux, Path::new("ca.pem"), &f, &tools, &fake)[0].installed,
            Some(true)
        );
    }

    #[test]
    fn apply_stops_at_the_first_failure() {
        let f = files();
        let plan = install_plan(Os::Linux, Path::new("/d/ca.pem"), &f, &debian()).unwrap();
        let mut fake = Fake::default();
        fake.outputs.insert("install", RunOutput::default());
        let err = apply(&plan, &fake).unwrap_err();
        assert!(err.contains("Copiar la CA"), "{err}");
        assert_eq!(fake.ran.borrow().len(), 1, "no sigue después del fallo");
    }
}
