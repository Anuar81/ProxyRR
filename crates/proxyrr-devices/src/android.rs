//! Android con `adb` (spec 0002): detectar dispositivos, configurar proxy + CA y revertir.
//!
//! Dos modos, según lo que permita el dispositivo:
//! - **CA de sistema** (emulador "Google APIs" / AOSP, que acepta `adb root`): la CA se agrega al
//!   almacén del sistema montando un `tmpfs` encima (no toca la imagen; se pierde al reiniciar). En
//!   Android 14+ además se monta sobre el store APEX de Conscrypt en los procesos que ya corren. Todas
//!   las apps confían en ella sin cambiar su código.
//! - **CA de usuario** (dispositivo físico o emulador con Play Store): se configura el proxy y se copia
//!   la CA a `Download/`; instalarla y declararla en `network_security_config` queda a mano (guía).
//!
//! El proxy del dispositivo apunta a `10.0.2.2` en emuladores (el loopback del host) y a
//! `127.0.0.1` con `adb reverse` en dispositivos físicos por USB, así que el proxy puede seguir
//! escuchando solo en loopback.

use std::env;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Directorio de trabajo en el dispositivo.
const REMOTE_DIR: &str = "/data/local/tmp/proxyrr";
/// Dirección del loopback del host vista desde el emulador.
const EMULATOR_HOST: &str = "10.0.2.2";
/// Marca que imprime el script cuando la CA quedó en el store del sistema.
const OK_MARK: &str = "PROXYRR_OK";

/// Salida de un comando `adb`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdbOutput {
    /// `true` si terminó con código 0.
    pub success: bool,
    /// Salida estándar.
    pub stdout: String,
    /// Salida de error.
    pub stderr: String,
}

impl AdbOutput {
    fn text(&self) -> String {
        format!("{}{}", self.stdout, self.stderr).trim().to_owned()
    }
}

/// Ejecuta `adb`. Abstracto para poder probar la lógica sin dispositivos.
pub trait Adb: fmt::Debug + Send + Sync {
    /// Corre `adb <args>` y devuelve su salida.
    ///
    /// # Errors
    ///
    /// Si `adb` no se pudo ejecutar.
    fn run(&self, args: &[&str]) -> io::Result<AdbOutput>;
}

/// El binario `adb` real.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbCli {
    path: PathBuf,
}

impl AdbCli {
    /// Usa el `adb` de `path`.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Ruta del binario.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Adb for AdbCli {
    fn run(&self, args: &[&str]) -> io::Result<AdbOutput> {
        let mut command = Command::new(&self.path);
        command.args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Sin ventana de consola cuando lo llama la app de escritorio.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let output = command.output()?;
        Ok(AdbOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// `adb` no aparece en ningún lado.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "no se encontró `adb`. Instalá las Android SDK Platform-Tools (vienen con Android Studio) o \
     indicá su ruta con --adb / PROXYRR_ADB. Se buscó en: {}",
    searched.join(", ")
)]
pub struct AdbNotFound {
    /// Lugares revisados.
    pub searched: Vec<String>,
}

fn adb_file() -> &'static str {
    if cfg!(windows) { "adb.exe" } else { "adb" }
}

/// Busca `adb`: `explicit`, `PROXYRR_ADB`, `ANDROID_HOME` / `ANDROID_SDK_ROOT`, el `PATH` y las
/// ubicaciones por defecto del SDK de Android Studio en cada SO.
///
/// # Errors
///
/// [`AdbNotFound`] con la lista de lugares revisados.
pub fn locate_adb(explicit: Option<&Path>) -> Result<PathBuf, AdbNotFound> {
    let env_dir = |name: &str| {
        env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = explicit {
        // Una ruta explícita que no existe es un error, no se cae a otra.
        return if path.is_file() {
            Ok(path.to_path_buf())
        } else {
            Err(AdbNotFound {
                searched: vec![path.display().to_string()],
            })
        };
    }
    candidates.extend(env_dir("PROXYRR_ADB"));
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(sdk) = env_dir(var) {
            candidates.push(sdk.join("platform-tools").join(adb_file()));
        }
    }
    if let Some(path) = env::var_os("PATH") {
        candidates.extend(env::split_paths(&path).map(|dir| dir.join(adb_file())));
    }
    candidates.extend(
        default_sdk_dirs()
            .into_iter()
            .map(|sdk| sdk.join("platform-tools").join(adb_file())),
    );
    if let Some(found) = candidates.iter().find(|p| p.is_file()) {
        return Ok(found.clone());
    }
    Err(AdbNotFound {
        searched: candidates
            .iter()
            .filter(|p| !is_path_entry(p))
            .map(|p| p.display().to_string())
            .chain(std::iter::once("PATH".to_owned()))
            .collect(),
    })
}

/// `true` si `path` salió del `PATH` (para no listar cada directorio en el error).
fn is_path_entry(path: &Path) -> bool {
    env::var_os("PATH")
        .is_some_and(|all| env::split_paths(&all).any(|dir| path.parent() == Some(dir.as_path())))
}

fn default_sdk_dirs() -> Vec<PathBuf> {
    let home = env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let mut dirs = Vec::new();
    if cfg!(windows) {
        if let Some(local) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            dirs.push(local.join("Android").join("Sdk"));
        }
    } else if cfg!(target_os = "macos") {
        dirs.extend(home.map(|h| h.join("Library/Android/sdk")));
    } else {
        dirs.extend(home.map(|h| h.join("Android/Sdk")));
    }
    dirs
}

/// Estado de un dispositivo según `adb devices`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceState {
    /// Conectado y autorizado.
    Ready,
    /// Falta aceptar el aviso "¿Permitir depuración USB?" en el dispositivo.
    Unauthorized,
    /// Desconectado o arrancando.
    Offline,
    /// Otro estado que informe `adb`.
    Other(String),
}

/// Un emulador o dispositivo conectado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// Serial de `adb` (`emulator-5554`, `R58M…`).
    pub serial: String,
    /// Estado.
    pub state: DeviceState,
    /// Modelo (`ro.product.model`), si se pudo leer.
    pub model: Option<String>,
    /// Nivel de API (`ro.build.version.sdk`).
    pub api_level: Option<u32>,
    /// Versión de Android (`ro.build.version.release`).
    pub release: Option<String>,
    /// `true` si es un emulador.
    pub emulator: bool,
    /// `true` si acepta `adb root` (build `userdebug`/`eng`): la CA puede ir al store del sistema.
    pub rootable: bool,
}

impl Device {
    /// Nombre para mostrar: `Pixel 9 (Android 15, API 35)`.
    #[must_use]
    pub fn label(&self) -> String {
        let name = self.model.clone().unwrap_or_else(|| self.serial.clone());
        match (&self.release, self.api_level) {
            (Some(release), Some(api)) => format!("{name} (Android {release}, API {api})"),
            (None, Some(api)) => format!("{name} (API {api})"),
            _ => name,
        }
    }
}

/// Lista emuladores y dispositivos, con sus propiedades si están listos.
///
/// # Errors
///
/// Si `adb devices` falla.
pub fn list_devices(adb: &dyn Adb) -> Result<Vec<Device>, String> {
    let output = adb
        .run(&["devices", "-l"])
        .map_err(|e| format!("no se pudo ejecutar adb: {e}"))?;
    if !output.success {
        return Err(format!("`adb devices` falló: {}", output.text()));
    }
    let mut devices: Vec<Device> = parse_devices(&output.stdout);
    for device in &mut devices {
        if device.state == DeviceState::Ready
            && let Ok(props) = adb.run(&["-s", &device.serial, "shell", "getprop"])
            && props.success
        {
            apply_props(device, &parse_props(&props.stdout));
        }
    }
    Ok(devices)
}

fn parse_devices(text: &str) -> Vec<Device> {
    text.lines()
        .filter(|line| !line.starts_with("List of devices") && !line.starts_with('*'))
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let serial = parts.next()?.to_owned();
            let state = match parts.next()? {
                "device" => DeviceState::Ready,
                "unauthorized" => DeviceState::Unauthorized,
                "offline" => DeviceState::Offline,
                other => DeviceState::Other(other.to_owned()),
            };
            let model = parts
                .find_map(|p| p.strip_prefix("model:"))
                .map(|m| m.replace('_', " "));
            Some(Device {
                emulator: serial.starts_with("emulator-"),
                serial,
                state,
                model,
                api_level: None,
                release: None,
                rootable: false,
            })
        })
        .collect()
}

/// `[clave]: [valor]` de `getprop`.
fn parse_props(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once("]: [")?;
            let key = key.trim().strip_prefix('[')?;
            let value = value.trim().strip_suffix(']')?;
            Some((key.to_owned(), value.to_owned()))
        })
        .collect()
}

fn apply_props(device: &mut Device, props: &[(String, String)]) {
    let get = |name: &str| {
        props
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    };
    if let Some(model) = get("ro.product.model") {
        device.model = Some(model.to_owned());
    }
    device.api_level = get("ro.build.version.sdk").and_then(|v| v.parse().ok());
    device.release = get("ro.build.version.release").map(str::to_owned);
    device.emulator |= get("ro.kernel.qemu") == Some("1") || get("ro.boot.qemu") == Some("1");
    // Las imágenes con Play Store son builds `user`: `adb root` no anda ("production builds").
    device.rootable = matches!(get("ro.build.type"), Some("userdebug" | "eng"))
        || get("ro.debuggable") == Some("1") && get("ro.build.type") != Some("user");
}

/// La CA a instalar.
#[derive(Debug, Clone, Copy)]
pub struct CaPayload<'a> {
    /// Certificado en PEM.
    pub pem: &'a str,
    /// Nombre de archivo para el store de Android: `<subject_hash_old>.0`.
    pub android_name: &'a str,
}

/// Cómo quedó configurado un dispositivo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaMode {
    /// CA en el store del sistema: todas las apps confían.
    System,
    /// CA copiada al dispositivo; falta instalarla como certificado de usuario (a mano).
    UserManual {
        /// Ruta en el dispositivo.
        path: String,
    },
}

/// Resultado de [`configure`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configured {
    /// Serial.
    pub serial: String,
    /// `host:puerto` que quedó como proxy en el dispositivo.
    pub proxy: String,
    /// `true` si se usó `adb reverse` (hay que quitarlo al revertir).
    pub reverse: bool,
    /// Modo de la CA.
    pub ca: CaMode,
    /// Pasos que quedan a mano o advertencias.
    pub notes: Vec<String>,
}

/// Deja `device` usando el proxy en `port` y confiando en la CA.
///
/// # Errors
///
/// Si `adb` falla en algún paso; el mensaje dice cuál.
pub fn configure(
    adb: &dyn Adb,
    device: &Device,
    ca: CaPayload<'_>,
    port: u16,
) -> Result<Configured, String> {
    if device.state != DeviceState::Ready {
        return Err(match device.state {
            DeviceState::Unauthorized => format!(
                "{}: aceptá el aviso \"¿Permitir depuración USB?\" en el dispositivo y reintentá",
                device.serial
            ),
            _ => format!("{}: el dispositivo no está listo", device.serial),
        });
    }
    let serial = device.serial.as_str();
    let mut notes = Vec::new();

    let ca_mode = if device.rootable && become_root(adb, serial)? {
        install_system_ca(adb, serial, ca)?;
        CaMode::System
    } else {
        let path = "/sdcard/Download/proxyrr-ca.crt".to_owned();
        push_text(adb, serial, ca.pem, &path)?;
        notes.push(format!(
            "La CA quedó en {path}. Instalala en Ajustes → Seguridad → Más ajustes de seguridad → \
             Instalar desde el almacenamiento → Certificado de CA (el nombre exacto cambia según el \
             fabricante)."
        ));
        notes.push(
            "Con CA de usuario, solo Chrome y las apps que la acepten en su network_security_config \
             ven el tráfico descifrado (`proxyrr setup android-device` tiene el snippet)."
                .to_owned(),
        );
        if device.emulator {
            notes.push(
                "Este emulador no acepta `adb root` (imagen con Play Store). Con una imagen \
                 \"Google APIs\" la CA va al sistema y la ven todas las apps."
                    .to_owned(),
            );
        }
        CaMode::UserManual { path }
    };

    let (proxy, reverse) = if device.emulator {
        (format!("{EMULATOR_HOST}:{port}"), false)
    } else {
        let tcp = format!("tcp:{port}");
        run_ok(adb, &["-s", serial, "reverse", &tcp, &tcp], "adb reverse")?;
        (format!("127.0.0.1:{port}"), true)
    };
    run_ok(
        adb,
        &[
            "-s",
            serial,
            "shell",
            "settings",
            "put",
            "global",
            "http_proxy",
            &proxy,
        ],
        "configurar el proxy",
    )?;
    Ok(Configured {
        serial: serial.to_owned(),
        proxy,
        reverse,
        ca: ca_mode,
        notes,
    })
}

/// Quita el proxy del dispositivo (si no, se queda sin internet al cerrar ProxyRR) y el
/// `adb reverse`. La CA del sistema sigue hasta el próximo reinicio del emulador.
///
/// # Errors
///
/// Si `adb` no pudo quitar el proxy.
pub fn revert(adb: &dyn Adb, serial: &str, port: u16, reverse: bool) -> Result<(), String> {
    // `:0` lo quita en el momento; `settings delete` solo surte efecto tras reiniciar.
    run_ok(
        adb,
        &[
            "-s",
            serial,
            "shell",
            "settings",
            "put",
            "global",
            "http_proxy",
            ":0",
        ],
        "quitar el proxy",
    )?;
    if reverse {
        let tcp = format!("tcp:{port}");
        // Si el reverse ya no existe (se reinició adb), no es un error.
        let _ = adb.run(&["-s", serial, "reverse", "--remove", &tcp]);
    }
    Ok(())
}

fn run_ok(adb: &dyn Adb, args: &[&str], what: &str) -> Result<AdbOutput, String> {
    let output = adb
        .run(args)
        .map_err(|e| format!("{what}: no se pudo ejecutar adb: {e}"))?;
    if output.success {
        Ok(output)
    } else {
        Err(format!("{what} falló: {}", output.text()))
    }
}

/// `adb root` y esperar a que vuelva. `false` si el build no lo permite.
fn become_root(adb: &dyn Adb, serial: &str) -> Result<bool, String> {
    let output = run_ok(adb, &["-s", serial, "root"], "adb root")?;
    if output.text().contains("cannot run as root") {
        return Ok(false);
    }
    run_ok(
        adb,
        &["-s", serial, "wait-for-device"],
        "esperar al dispositivo",
    )?;
    let whoami = run_ok(adb, &["-s", serial, "shell", "id", "-u"], "comprobar root")?;
    Ok(whoami.stdout.trim() == "0")
}

/// Sube `text` a `remote` pasando por un archivo temporal local.
fn push_text(adb: &dyn Adb, serial: &str, text: &str, remote: &str) -> Result<(), String> {
    let dir = tempfile::tempdir().map_err(|e| format!("archivo temporal: {e}"))?;
    let local = dir.path().join("payload");
    std::fs::write(&local, text).map_err(|e| format!("archivo temporal: {e}"))?;
    let local: OsString = local.into_os_string();
    let local = local.to_string_lossy();
    run_ok(adb, &["-s", serial, "push", &local, remote], "adb push")?;
    Ok(())
}

fn install_system_ca(adb: &dyn Adb, serial: &str, ca: CaPayload<'_>) -> Result<(), String> {
    run_ok(
        adb,
        &["-s", serial, "shell", "mkdir", "-p", REMOTE_DIR],
        "preparar el dispositivo",
    )?;
    push_text(
        adb,
        serial,
        ca.pem,
        &format!("{REMOTE_DIR}/{}", ca.android_name),
    )?;
    let script_path = format!("{REMOTE_DIR}/install-ca.sh");
    push_text(adb, serial, &install_script(ca.android_name), &script_path)?;
    let output = run_ok(
        adb,
        &["-s", serial, "shell", "sh", &script_path],
        "instalar la CA",
    )?;
    if output.stdout.contains(OK_MARK) {
        Ok(())
    } else {
        Err(format!(
            "la CA no quedó en el store del sistema: {}",
            output.text()
        ))
    }
}

/// Script (sh de Android) que agrega la CA al store del sistema y, en Android 14+, al de Conscrypt.
#[must_use]
pub fn install_script(android_name: &str) -> String {
    INSTALL_SCRIPT
        // Si git convirtió el fuente a CRLF (Windows), el sh de Android no lo tolera.
        .replace('\r', "")
        .replace("@DIR@", REMOTE_DIR)
        .replace("@NAME@", android_name)
        .replace("@OK@", OK_MARK)
}

const INSTALL_SCRIPT: &str = r#"#!/system/bin/sh
# Generado por ProxyRR. Agrega la CA al store del sistema montando un tmpfs encima:
# no modifica la imagen y desaparece al reiniciar el emulador.
set -e
CERT="@DIR@/@NAME@"
WORK="@DIR@/cacerts"
SYS=/system/etc/security/cacerts
APEX=/apex/com.android.conscrypt/cacerts

rm -rf "$WORK"
mkdir -p "$WORK"
# Android 14+: el store real es el del APEX de Conscrypt.
if [ -d "$APEX" ]; then SRC="$APEX"; else SRC="$SYS"; fi
cp "$SRC"/* "$WORK"/
cp "$CERT" "$WORK"/

if ! grep -q " $SYS tmpfs " /proc/mounts; then
  mount -t tmpfs tmpfs "$SYS"
fi
cp "$WORK"/* "$SYS"/
chown root:root "$SYS" "$SYS"/*
chmod 755 "$SYS"
chmod 644 "$SYS"/*
chcon u:object_r:system_file:s0 "$SYS" "$SYS"/* 2>/dev/null || true

if [ -d "$APEX" ]; then
  set +e
  grep -q " $APEX tmpfs " /proc/mounts || mount --bind "$SYS" "$APEX"
  # Cada app vive en su propio namespace de montaje, heredado de zygote: se monta en zygote (apps
  # nuevas) y en las apps que ya están corriendo.
  for Z in $(pidof zygote zygote64); do
    nsenter --mount=/proc/$Z/ns/mnt -- /bin/mount --bind "$SYS" "$APEX"
    for P in $(ps -o PID -P "$Z" | grep -v PID); do
      nsenter --mount=/proc/$P/ns/mnt -- /bin/mount --bind "$SYS" "$APEX" 2>/dev/null
    done
  done
  set -e
fi

rm -rf "$WORK"
if [ -f "$SYS/@NAME@" ] && { [ ! -d "$APEX" ] || [ -f "$APEX/@NAME@" ]; }; then
  echo @OK@
fi
"#;

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// `adb` falso: responde según el primer patrón que matchee y registra cada llamada.
    #[derive(Debug, Default)]
    struct FakeAdb {
        replies: Vec<(&'static str, AdbOutput)>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeAdb {
        fn reply(mut self, contains: &'static str, stdout: &str) -> Self {
            self.replies.push((
                contains,
                AdbOutput {
                    success: true,
                    stdout: stdout.to_owned(),
                    stderr: String::new(),
                },
            ));
            self
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Adb for FakeAdb {
        fn run(&self, args: &[&str]) -> io::Result<AdbOutput> {
            let line = args.join(" ");
            self.calls.lock().unwrap().push(line.clone());
            Ok(self
                .replies
                .iter()
                .find(|(pattern, _)| line.contains(pattern))
                .map_or_else(
                    || AdbOutput {
                        success: true,
                        ..AdbOutput::default()
                    },
                    |(_, out)| out.clone(),
                ))
        }
    }

    const DEVICES: &str = "List of devices attached\n\
        emulator-5554          device product:sdk_gphone64_x86_64 model:sdk_gphone64_x86_64 device:emu64xa transport_id:1\n\
        R58M12345              unauthorized usb:1-1 transport_id:2\n\n";

    const PROPS_GOOGLE_APIS: &str = "[ro.build.type]: [userdebug]\n[ro.build.version.sdk]: [35]\n\
        [ro.build.version.release]: [15]\n[ro.product.model]: [Pixel 9]\n[ro.kernel.qemu]: [1]\n";

    const PROPS_PLAY_STORE: &str = "[ro.build.type]: [user]\n[ro.debuggable]: [0]\n\
        [ro.build.version.sdk]: [35]\n[ro.build.version.release]: [15]\n[ro.product.model]: [Pixel 9]\n";

    fn ca() -> CaPayload<'static> {
        CaPayload {
            pem: "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
            android_name: "1a2b3c4d.0",
        }
    }

    #[test]
    fn lists_devices_with_their_properties() {
        let adb = FakeAdb::default()
            .reply("devices -l", DEVICES)
            .reply("getprop", PROPS_GOOGLE_APIS);
        let devices = list_devices(&adb).unwrap();
        assert_eq!(devices.len(), 2);
        let emu = &devices[0];
        assert_eq!(emu.serial, "emulator-5554");
        assert_eq!(emu.state, DeviceState::Ready);
        assert_eq!(emu.api_level, Some(35));
        assert!(emu.emulator && emu.rootable);
        assert_eq!(emu.label(), "Pixel 9 (Android 15, API 35)");
        let phone = &devices[1];
        assert_eq!(phone.state, DeviceState::Unauthorized);
        assert!(!phone.emulator);
        // A un dispositivo sin autorizar no se le piden propiedades.
        assert_eq!(
            adb.calls().iter().filter(|c| c.contains("getprop")).count(),
            1
        );
    }

    #[test]
    fn play_store_images_are_not_rootable() {
        let mut device = parse_devices(DEVICES).remove(0);
        apply_props(&mut device, &parse_props(PROPS_PLAY_STORE));
        assert!(!device.rootable);
        assert!(
            device.emulator,
            "el serial emulator-* alcanza para saber que es emulador"
        );
    }

    #[test]
    fn rootable_emulator_gets_system_ca_and_host_loopback_proxy() {
        let adb = FakeAdb::default()
            .reply(" root", "restarting adbd as root\n")
            .reply("id -u", "0\n")
            .reply("install-ca.sh", "PROXYRR_OK\n");
        let mut device = parse_devices(DEVICES).remove(0);
        apply_props(&mut device, &parse_props(PROPS_GOOGLE_APIS));
        let done = configure(&adb, &device, ca(), 9090).unwrap();
        assert_eq!(done.ca, CaMode::System);
        assert_eq!(done.proxy, "10.0.2.2:9090");
        assert!(!done.reverse);
        let calls = adb.calls();
        assert!(
            calls
                .iter()
                .any(|c| c.ends_with("/data/local/tmp/proxyrr/1a2b3c4d.0"))
        );
        assert!(
            calls
                .iter()
                .any(|c| c.ends_with("settings put global http_proxy 10.0.2.2:9090"))
        );
        assert!(!calls.iter().any(|c| c.contains("reverse")));
    }

    #[test]
    fn failed_system_install_is_an_error() {
        let adb = FakeAdb::default()
            .reply(" root", "restarting adbd as root\n")
            .reply("id -u", "0\n")
            .reply("install-ca.sh", "mount: Permission denied\n");
        let mut device = parse_devices(DEVICES).remove(0);
        apply_props(&mut device, &parse_props(PROPS_GOOGLE_APIS));
        let error = configure(&adb, &device, ca(), 9090).unwrap_err();
        assert!(error.contains("no quedó en el store"), "{error}");
        assert!(!adb.calls().iter().any(|c| c.contains("http_proxy")));
    }

    #[test]
    fn production_build_falls_back_to_user_ca() {
        let adb =
            FakeAdb::default().reply(" root", "adbd cannot run as root in production builds\n");
        let mut device = parse_devices(DEVICES).remove(0);
        // Por propiedades parecía rooteable, pero adbd se niega: se cae al modo usuario.
        apply_props(&mut device, &parse_props(PROPS_GOOGLE_APIS));
        let done = configure(&adb, &device, ca(), 9090).unwrap();
        assert!(matches!(done.ca, CaMode::UserManual { .. }));
        assert!(done.notes.iter().any(|n| n.contains("Google APIs")));
    }

    #[test]
    fn physical_device_uses_adb_reverse_and_user_ca() {
        let adb = FakeAdb::default();
        let device = Device {
            serial: "R58M12345".into(),
            state: DeviceState::Ready,
            model: Some("SM-G990".into()),
            api_level: Some(34),
            release: Some("14".into()),
            emulator: false,
            rootable: false,
        };
        let done = configure(&adb, &device, ca(), 9191).unwrap();
        assert_eq!(done.proxy, "127.0.0.1:9191");
        assert!(done.reverse);
        let calls = adb.calls();
        assert!(calls.contains(&"-s R58M12345 reverse tcp:9191 tcp:9191".to_owned()));
        assert!(
            !calls.iter().any(|c| c.contains(" root")),
            "nunca intenta root en un físico"
        );
    }

    #[test]
    fn unauthorized_device_explains_what_to_do() {
        let device = parse_devices(DEVICES).remove(1);
        let error = configure(&FakeAdb::default(), &device, ca(), 9090).unwrap_err();
        assert!(error.contains("depuración USB"), "{error}");
    }

    #[test]
    fn revert_clears_proxy_and_reverse() {
        let adb = FakeAdb::default();
        revert(&adb, "R58M12345", 9090, true).unwrap();
        assert_eq!(
            adb.calls(),
            [
                "-s R58M12345 shell settings put global http_proxy :0",
                "-s R58M12345 reverse --remove tcp:9090",
            ]
        );
    }

    #[test]
    fn script_targets_both_stores() {
        let script = install_script("1a2b3c4d.0");
        assert!(script.contains("CERT=\"/data/local/tmp/proxyrr/1a2b3c4d.0\""));
        assert!(script.contains("/apex/com.android.conscrypt/cacerts"));
        assert!(script.contains("echo PROXYRR_OK"));
        assert!(!script.contains('\r'), "el sh de Android no tolera CRLF");
        assert!(!script.contains('@'), "quedó un marcador sin reemplazar");
    }

    #[test]
    fn explicit_adb_path_must_exist() {
        let error = locate_adb(Some(Path::new("/no/existe/adb"))).unwrap_err();
        assert_eq!(error.searched, ["/no/existe/adb"]);
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join(adb_file());
        std::fs::write(&fake, b"").unwrap();
        assert_eq!(locate_adb(Some(&fake)).unwrap(), fake);
    }
}
