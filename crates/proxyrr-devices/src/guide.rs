//! Guías paso a paso por destino, con la IP y el puerto reales ya completados (spec 0006, CA 6–10).

use std::fmt::{self, Write as _};

use crate::files::CaFiles;

/// Destino de `proxyrr setup`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// iPhone / iPad físico.
    Ios,
    /// Simulador de iOS (solo macOS).
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

impl Target {
    /// Todos, en el orden del menú.
    pub const ALL: [Self; 7] = [
        Self::Ios,
        Self::IosSimulator,
        Self::AndroidEmulator,
        Self::AndroidDevice,
        Self::Windows,
        Self::Macos,
        Self::Linux,
    ];

    /// Nombre en la línea de comandos.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Ios => "ios",
            Self::IosSimulator => "ios-simulator",
            Self::AndroidEmulator => "android-emulator",
            Self::AndroidDevice => "android-device",
            Self::Windows => "windows",
            Self::Macos => "macos",
            Self::Linux => "linux",
        }
    }

    /// `true` si el destino es otro aparato en la red (el proxy tiene que escuchar fuera de loopback).
    #[must_use]
    pub fn is_remote(self) -> bool {
        matches!(self, Self::Ios | Self::AndroidDevice)
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Datos para completar la guía.
#[derive(Debug, Clone)]
pub struct GuideContext<'a> {
    /// CA.
    pub files: &'a CaFiles,
    /// IP de LAN de este equipo (para dispositivos físicos).
    pub lan_host: String,
    /// Puerto del proxy.
    pub port: u16,
}

/// Una guía lista para mostrar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guide {
    /// Título.
    pub title: String,
    /// Pasos en orden.
    pub steps: Vec<String>,
    /// Bloques de código para copiar: (título, contenido).
    pub snippets: Vec<(String, String)>,
    /// URL para mostrar como QR.
    pub qr_url: Option<String>,
    /// Avisos al final.
    pub notes: Vec<String>,
}

/// `network_security_config.xml` limitado a builds debug (spec 0002, CA 6).
pub const NETWORK_SECURITY_CONFIG: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!-- app/src/main/res/xml/network_security_config.xml -->
<network-security-config>
    <!-- Solo en builds debug: en release la app sigue sin confiar en CAs de usuario. -->
    <debug-overrides>
        <trust-anchors>
            <certificates src="user" />
            <certificates src="system" />
        </trust-anchors>
    </debug-overrides>
</network-security-config>"#;

/// Línea del `AndroidManifest.xml` que activa el archivo anterior.
pub const MANIFEST_LINE: &str = r#"<application
    android:networkSecurityConfig="@xml/network_security_config"
    ... >"#;

const KEY_WARNING: &str =
    "Quien tenga la clave de la CA puede leer tu HTTPS: desinstalala cuando no la uses.";

/// Guía para `target`.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "es una tabla de textos, una rama por destino; partirla no la hace más clara"
)]
pub fn guide(target: Target, ctx: &GuideContext<'_>) -> Guide {
    let port = ctx.port;
    let lan = &ctx.lan_host;
    let name = &ctx.files.name;
    let direct_url = format!("http://{lan}:{port}/cert");
    let listen_all = format!("proxyrr start --mitm --listen 0.0.0.0:{port}");
    let local = format!("proxyrr start --mitm --listen 127.0.0.1:{port}");
    let mut notes = vec![KEY_WARNING.to_owned()];
    let (title, steps, snippets, qr_url) = match target {
        Target::Ios => (
            "iPhone / iPad".to_owned(),
            vec![
                format!("En esta máquina: `{listen_all}` (el teléfono tiene que poder llegar a {lan}:{port})."),
                format!("En el iPhone: Ajustes → Wi-Fi → (i) de tu red → Configurar proxy → Manual: servidor {lan}, puerto {port}."),
                format!("Abrí Safari en http://proxyrr.cert (o escaneá el QR, que abre {direct_url}) y tocá \"Descargar perfil\"."),
                "Ajustes → Perfil descargado → Instalar (pide el código del teléfono).".to_owned(),
                format!("PASO QUE TODOS OLVIDAN: Ajustes → General → Información → Ajustes de confianza de certificados → activá \"{name}\"."),
                "Para dejar de usarlo: quitá el proxy en Wi-Fi y borrá el perfil en Ajustes → General → VPN y gestión de dispositivos.".to_owned(),
            ],
            Vec::new(),
            Some(direct_url.clone()),
        ),
        Target::IosSimulator => (
            "Simulador de iOS (macOS)".to_owned(),
            vec![
                format!("Levantá el proxy: `{local}`."),
                format!("El simulador usa el proxy de macOS: Ajustes del Sistema → Red → (tu red) → Detalles → Proxies → Web y Web segura: 127.0.0.1:{port}."),
                "Con el simulador abierto: `proxyrr setup ios-simulator --install` instala la CA sola (`xcrun simctl keychain booted add-root-cert`).".to_owned(),
                "Reiniciá la app en el simulador para que tome la CA nueva.".to_owned(),
            ],
            Vec::new(),
            None,
        ),
        Target::AndroidEmulator => {
            notes.push("La configuración automática del emulador (proxy + CA en el store del sistema vía adb, también en Android 14+) llega con la spec 0002. Mientras tanto, estos pasos sirven para Chrome y para apps con network_security_config.".to_owned());
            (
                "Emulador de Android".to_owned(),
                vec![
                    format!("Levantá el proxy: `{local}` (el emulador ve el loopback de esta máquina como 10.0.2.2)."),
                    format!("En el emulador: ⋯ (Extended controls) → Settings → Proxy → Manual: 10.0.2.2, puerto {port} → Apply. O arrancalo con `emulator -avd <AVD> -http-proxy http://127.0.0.1:{port}`."),
                    "En el Chrome del emulador abrí http://proxyrr.cert y tocá \"Descargar certificado\".".to_owned(),
                    "Ajustes → Seguridad → Cifrado y credenciales → Instalar certificado → Certificado de CA → proxyrr-ca.crt.".to_owned(),
                    "Chrome ya descifra. Para tus apps, agregá el network_security_config de abajo (solo debug).".to_owned(),
                ],
                android_snippets(),
                None,
            )
        }
        Target::AndroidDevice => (
            "Android físico".to_owned(),
            vec![
                format!("En esta máquina: `{listen_all}` (el teléfono tiene que poder llegar a {lan}:{port})."),
                format!("En el teléfono: Ajustes → Wi-Fi → mantené apretada tu red → Modificar → Opciones avanzadas → Proxy manual: {lan}, puerto {port}."),
                format!("Abrí http://proxyrr.cert en Chrome (o escaneá el QR, que abre {direct_url}) y tocá \"Descargar certificado\"."),
                "Ajustes → Seguridad → Cifrado y credenciales → Instalar certificado → Certificado de CA → proxyrr-ca.crt (confirmá la advertencia).".to_owned(),
                "Desde Android 7 las apps no confían en CAs de usuario: agregá el network_security_config de abajo a tu app (solo builds debug). Chrome sí confía.".to_owned(),
                "Para dejar de usarlo: quitá el proxy en Wi-Fi y borrá la CA en Cifrado y credenciales → Credenciales de usuario.".to_owned(),
            ],
            android_snippets(),
            Some(direct_url.clone()),
        ),
        Target::Windows => (
            "Este equipo (Windows)".to_owned(),
            vec![
                "`proxyrr ca install` (Windows muestra un aviso de seguridad: aceptalo). No hace falta admin.".to_owned(),
                format!("Levantá el proxy: `{local}`."),
                format!("Configuración → Red e Internet → Proxy → Configuración manual: 127.0.0.1, puerto {port}. O solo un navegador: `chrome --proxy-server=http://127.0.0.1:{port}`."),
                "Para dejar de usarlo: apagá el proxy manual y corré `proxyrr ca uninstall`.".to_owned(),
            ],
            Vec::new(),
            None,
        ),
        Target::Macos => (
            "Este equipo (macOS)".to_owned(),
            vec![
                "`proxyrr ca install` (macOS pide tu contraseña para confiar en la CA).".to_owned(),
                format!("Levantá el proxy: `{local}`."),
                format!("Ajustes del Sistema → Red → (tu red) → Detalles → Proxies → Web y Web segura: 127.0.0.1:{port}."),
                "Para dejar de usarlo: apagá los proxies y corré `proxyrr ca uninstall`.".to_owned(),
            ],
            Vec::new(),
            None,
        ),
        Target::Linux => (
            "Este equipo (Linux)".to_owned(),
            vec![
                "`proxyrr ca install` (pide sudo para el store del sistema; también instala en la base NSS de Chrome si está `certutil`, paquete libnss3-tools / nss-tools).".to_owned(),
                format!("Levantá el proxy: `{local}`."),
                format!("GNOME: Configuración → Red → Proxy → Manual: 127.0.0.1:{port} para HTTP y HTTPS. En terminal: `export http_proxy=http://127.0.0.1:{port} https_proxy=http://127.0.0.1:{port}`."),
                "Para dejar de usarlo: apagá el proxy y corré `proxyrr ca uninstall`.".to_owned(),
            ],
            Vec::new(),
            None,
        ),
    };
    if target.is_remote() {
        notes.push(format!(
            "Si el teléfono no llega a {lan}:{port}, revisá el firewall de esta máquina (Windows pregunta la primera vez que se escucha en 0.0.0.0) y que estén en la misma red."
        ));
    }
    Guide {
        title,
        steps,
        snippets,
        qr_url,
        notes,
    }
}

fn android_snippets() -> Vec<(String, String)> {
    vec![
        (
            "res/xml/network_security_config.xml".to_owned(),
            NETWORK_SECURITY_CONFIG.to_owned(),
        ),
        ("AndroidManifest.xml".to_owned(), MANIFEST_LINE.to_owned()),
    ]
}

/// Texto de la guía para la terminal (sin el QR, que se imprime aparte).
#[must_use]
pub fn render(guide: &Guide) -> String {
    let mut out = format!("{}\n\n", guide.title);
    for (i, step) in guide.steps.iter().enumerate() {
        let _ = writeln!(out, "{}. {step}", i + 1);
    }
    for (title, code) in &guide.snippets {
        let _ = write!(out, "\n--- {title} ---\n{code}\n");
    }
    for note in &guide.notes {
        let _ = write!(out, "\nNota: {note}");
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::tests::files;

    fn ctx(files: &CaFiles) -> GuideContext<'_> {
        GuideContext {
            files,
            lan_host: "192.168.1.50".into(),
            port: 9090,
        }
    }

    #[test]
    fn remote_guides_use_the_lan_address_and_a_qr() {
        let f = files();
        for target in [Target::Ios, Target::AndroidDevice] {
            let g = guide(target, &ctx(&f));
            let text = render(&g);
            assert!(text.contains("192.168.1.50"), "{target}: {text}");
            assert!(text.contains("0.0.0.0:9090"), "{target}");
            assert_eq!(g.qr_url.as_deref(), Some("http://192.168.1.50:9090/cert"));
        }
    }

    #[test]
    fn ios_includes_the_trust_step() {
        let f = files();
        let text = render(&guide(Target::Ios, &ctx(&f)));
        assert!(text.contains("Ajustes de confianza de certificados"));
        assert!(text.contains(&f.name));
    }

    #[test]
    fn android_includes_network_security_config() {
        let f = files();
        for target in [Target::AndroidDevice, Target::AndroidEmulator] {
            let text = render(&guide(target, &ctx(&f)));
            assert!(text.contains("<debug-overrides>"), "{target}");
            assert!(text.contains("android:networkSecurityConfig"), "{target}");
        }
        assert!(render(&guide(Target::AndroidEmulator, &ctx(&f))).contains("10.0.2.2"));
    }

    #[test]
    fn every_target_has_steps_and_the_key_warning() {
        let f = files();
        for target in Target::ALL {
            let g = guide(target, &ctx(&f));
            assert!(g.steps.len() >= 3, "{target}");
            assert!(g.notes.iter().any(|n| n.contains("clave")), "{target}");
        }
    }
}
