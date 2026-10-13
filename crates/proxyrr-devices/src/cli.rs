//! La CLI `proxyrr` en el `PATH` desde la app instalada (spec 0013, criterio 5).
//!
//! En macOS el `.app` trae la CLI en `Contents/MacOS/proxyrr`. Para usarla desde la terminal se
//! enlaza en [`LINK`], que es de root: el plan pide la contraseña con el diálogo de administrador del
//! sistema (`osascript … with administrator privileges`), igual que haría cualquier instalador.

use std::path::Path;

use crate::trust::{Os, Step};

/// Dónde queda el enlace a la CLI.
pub const LINK: &str = "/usr/local/bin/proxyrr";

/// Comando para hacer lo mismo a mano en la terminal.
#[must_use]
pub fn manual_command(cli: &Path) -> String {
    let quoted = cli.display().to_string().replace('\'', "'\\''");
    format!("sudo mkdir -p /usr/local/bin && sudo ln -sf '{quoted}' {LINK}")
}

/// Plan para enlazar `cli` en [`LINK`].
///
/// # Errors
/// Fuera de macOS (Windows y Linux tienen su propio instalador, fuera de esta spec), o si la ruta tiene
/// caracteres que no se pueden pasar con seguridad dentro del `AppleScript` (`"`, `\`, saltos de línea).
pub fn link_plan(os: Os, cli: &Path) -> Result<Step, String> {
    if os != Os::Macos {
        return Err("instalar la CLI desde la app solo está disponible en macOS".to_owned());
    }
    let path = cli
        .to_str()
        .ok_or_else(|| "la ruta de la CLI no es UTF-8".to_owned())?;
    // Ruta de macOS: absoluta = empieza con `/`. No `Path::is_absolute`, que en Windows exige letra de
    // unidad y haría fallar los tests de este plan (que corren en los 3 sistemas).
    if !path.starts_with('/') || path.contains(['"', '\\', '\n', '\r']) {
        return Err(format!(
            "ruta de la CLI no soportada para enlazar desde la app: {path}"
        ));
    }
    // `quoted form of` hace el escape para el shell; la ruta va como literal de AppleScript (sin `"`
    // ni `\`, ya validado), así que ningún carácter puede cerrar el string ni inyectar comandos.
    let script = format!(
        "do shell script \"mkdir -p /usr/local/bin && ln -sf \" & quoted form of \"{path}\" & \" {LINK}\" with administrator privileges"
    );
    Ok(Step {
        what: format!("Enlazar la CLI en {LINK} (pide la contraseña de administrador)"),
        program: "osascript".to_owned(),
        args: vec!["-e".to_owned(), script],
        sudo: false,
    })
}

/// `true` si [`LINK`] (o `link`, en tests) apunta a `cli`.
#[must_use]
pub fn is_linked(link: &Path, cli: &Path) -> bool {
    std::fs::read_link(link).is_ok_and(|target| target == cli)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLI: &str = "/Applications/ProxyRR.app/Contents/MacOS/proxyrr";

    #[test]
    fn cli_link_plan() {
        let step = link_plan(Os::Macos, Path::new(CLI)).unwrap();
        assert_eq!(step.program, "osascript");
        assert!(!step.sudo, "la elevación la hace el diálogo del sistema");
        assert_eq!(step.args[0], "-e");
        assert_eq!(
            step.args[1],
            format!(
                "do shell script \"mkdir -p /usr/local/bin && ln -sf \" & quoted form of \"{CLI}\" & \" /usr/local/bin/proxyrr\" with administrator privileges"
            )
        );
    }

    #[test]
    fn cli_link_plan_keeps_spaces_and_quotes_inside_quoted_form() {
        let path = "/Users/ana/Apps de prueba/ProxyRR.app/Contents/MacOS/proxyrr";
        let step = link_plan(Os::Macos, Path::new(path)).unwrap();
        assert!(step.args[1].contains(&format!("quoted form of \"{path}\"")));
        let quirky = "/Users/o'neil/ProxyRR.app/Contents/MacOS/proxyrr";
        assert!(
            link_plan(Os::Macos, Path::new(quirky)).is_ok(),
            "`'` lo resuelve quoted form"
        );
    }

    #[test]
    fn cli_link_plan_rejects_what_could_break_the_script() {
        for bad in [
            "/tmp/x\" & do shell script \"rm -rf ~\" & \"",
            "/tmp/back\\slash/proxyrr",
            "/tmp/salto\nde-linea/proxyrr",
            "relativa/proxyrr",
        ] {
            assert!(link_plan(Os::Macos, Path::new(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn cli_link_plan_only_on_macos() {
        assert!(link_plan(Os::Windows, Path::new(CLI)).is_err());
        assert!(link_plan(Os::Linux, Path::new(CLI)).is_err());
    }

    #[test]
    fn manual_command_quotes_the_path() {
        assert_eq!(
            manual_command(Path::new(CLI)),
            format!("sudo mkdir -p /usr/local/bin && sudo ln -sf '{CLI}' /usr/local/bin/proxyrr")
        );
        assert_eq!(
            manual_command(Path::new("/Users/o'neil/proxyrr")),
            "sudo mkdir -p /usr/local/bin && sudo ln -sf '/Users/o'\\''neil/proxyrr' /usr/local/bin/proxyrr"
        );
    }

    #[test]
    fn is_linked_checks_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let cli = dir.path().join("proxyrr-real");
        std::fs::write(&cli, "").unwrap();
        let link = dir.path().join("proxyrr");
        assert!(!is_linked(&link, &cli));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&cli, &link).unwrap();
            assert!(is_linked(&link, &cli));
            assert!(!is_linked(&link, &dir.path().join("otra")));
        }
    }
}
