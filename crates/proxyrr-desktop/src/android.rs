//! Android desde la app (spec 0002): listar, configurar, revertir y recordar la ruta de `adb`.
//!
//! Lo que se configura se recuerda para revertirlo al cerrar la app: un dispositivo con el proxy
//! puesto y sin ProxyRR corriendo se queda sin internet.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use proxyrr_cert::CertificateAuthority;
use proxyrr_devices::android::{
    self, Adb, AdbCli, CaMode, CaPayload, Configured, Device, DeviceState,
};
use serde::{Deserialize, Serialize};

/// Archivo (en el directorio de datos) con la ruta de `adb` elegida por el usuario.
const SETTINGS_FILE: &str = "android.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct Settings {
    adb: Option<PathBuf>,
}

/// Un dispositivo para la vista.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceDto {
    /// Serial de `adb`.
    pub serial: String,
    /// Nombre para mostrar.
    pub label: String,
    /// `ready`, `unauthorized`, `offline` u otro.
    pub state: String,
    /// Emulador o físico.
    pub emulator: bool,
    /// CA de sistema posible.
    pub rootable: bool,
    /// Ya configurado por esta sesión de la app (proxy puesto).
    pub configured: bool,
}

/// Lista de dispositivos, o por qué no hay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DevicesDto {
    /// Ruta de `adb` en uso.
    pub adb: Option<String>,
    /// Error (sin `adb`, `adb` que falla).
    pub error: Option<String>,
    /// Dispositivos.
    pub devices: Vec<DeviceDto>,
}

/// Resultado de configurar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfiguredDto {
    /// Serial.
    pub serial: String,
    /// Proxy puesto en el dispositivo.
    pub proxy: String,
    /// `true` si la CA quedó en el store del sistema.
    pub system_ca: bool,
    /// Pasos que quedan o advertencias.
    pub notes: Vec<String>,
}

/// Estado de Android de la app.
#[derive(Debug)]
pub struct Android {
    settings_path: PathBuf,
    adb_override: Mutex<Option<PathBuf>>,
    configured: Mutex<Vec<(Configured, u16)>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Android {
    /// Lee la ruta de `adb` guardada en `data_dir`, si la hay.
    pub fn new(data_dir: &Path) -> Self {
        let settings_path = data_dir.join(SETTINGS_FILE);
        let saved: Settings = std::fs::read(&settings_path)
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        Self {
            settings_path,
            adb_override: Mutex::new(saved.adb),
            configured: Mutex::new(Vec::new()),
        }
    }

    fn adb(&self) -> Result<AdbCli, String> {
        let explicit = lock(&self.adb_override).clone();
        android::locate_adb(explicit.as_deref())
            .map(AdbCli::new)
            .map_err(|e| e.to_string())
    }

    /// Fija (y guarda) la ruta de `adb`. Vacía: volver a buscarla sola.
    ///
    /// # Errors
    /// Si la ruta no es un archivo o no se pudo guardar.
    pub fn set_adb_path(&self, path: &str) -> Result<(), String> {
        let path = path.trim();
        let value = if path.is_empty() {
            None
        } else {
            let path = PathBuf::from(path);
            if !path.is_file() {
                return Err(format!("{} no existe o no es un archivo", path.display()));
            }
            Some(path)
        };
        let json = serde_json::to_vec_pretty(&Settings { adb: value.clone() })
            .map_err(|e| e.to_string())?;
        std::fs::write(&self.settings_path, json)
            .map_err(|e| format!("no se pudo guardar {}: {e}", self.settings_path.display()))?;
        *lock(&self.adb_override) = value;
        Ok(())
    }

    /// Dispositivos conectados. Nunca falla: el error va en el DTO para mostrarlo.
    pub fn devices(&self) -> DevicesDto {
        let adb = match self.adb() {
            Ok(adb) => adb,
            Err(error) => {
                return DevicesDto {
                    adb: None,
                    error: Some(error),
                    devices: Vec::new(),
                };
            }
        };
        let configured: Vec<String> = lock(&self.configured)
            .iter()
            .map(|(c, _)| c.serial.clone())
            .collect();
        match android::list_devices(&adb) {
            Ok(devices) => DevicesDto {
                adb: Some(adb.path().display().to_string()),
                error: None,
                devices: devices
                    .iter()
                    .map(|d| to_dto(d, configured.contains(&d.serial)))
                    .collect(),
            },
            Err(error) => DevicesDto {
                adb: Some(adb.path().display().to_string()),
                error: Some(error),
                devices: Vec::new(),
            },
        }
    }

    /// Configura `serial` para el proxy en `port`.
    ///
    /// # Errors
    /// Sin `adb`, dispositivo inexistente o un paso de `adb` que falla.
    pub fn configure(
        &self,
        ca: &CertificateAuthority,
        serial: &str,
        port: u16,
    ) -> Result<ConfiguredDto, String> {
        let adb = self.adb()?;
        configure_with(&adb, &self.configured, ca, serial, port)
    }

    /// Quita el proxy de `serial`.
    ///
    /// # Errors
    /// Si `adb` falla.
    pub fn revert(&self, serial: &str) -> Result<(), String> {
        let adb = self.adb()?;
        revert_with(&adb, &self.configured, serial)
    }

    /// Revierte todo lo configurado (al cerrar la app). Los errores se ignoran: no hay a quién avisar.
    pub fn revert_all(&self) {
        let Ok(adb) = self.adb() else { return };
        let pending: Vec<String> = lock(&self.configured)
            .iter()
            .map(|(c, _)| c.serial.clone())
            .collect();
        for serial in pending {
            let _ = revert_with(&adb, &self.configured, &serial);
        }
    }
}

fn to_dto(device: &Device, configured: bool) -> DeviceDto {
    DeviceDto {
        serial: device.serial.clone(),
        label: device.label(),
        state: match &device.state {
            DeviceState::Ready => "ready".to_owned(),
            DeviceState::Unauthorized => "unauthorized".to_owned(),
            DeviceState::Offline => "offline".to_owned(),
            DeviceState::Other(other) => other.clone(),
        },
        emulator: device.emulator,
        rootable: device.rootable,
        configured,
    }
}

fn configure_with(
    adb: &dyn Adb,
    configured: &Mutex<Vec<(Configured, u16)>>,
    ca: &CertificateAuthority,
    serial: &str,
    port: u16,
) -> Result<ConfiguredDto, String> {
    let device = android::list_devices(adb)?
        .into_iter()
        .find(|d| d.serial == serial)
        .ok_or_else(|| format!("{serial} ya no está conectado"))?;
    let info = ca.info().map_err(|e| e.to_string())?;
    let name = format!("{:08x}.0", info.subject_hash_old);
    let done = android::configure(
        adb,
        &device,
        CaPayload {
            pem: ca.cert_pem(),
            android_name: &name,
        },
        port,
    )?;
    let dto = ConfiguredDto {
        serial: done.serial.clone(),
        proxy: done.proxy.clone(),
        system_ca: done.ca == CaMode::System,
        notes: done.notes.clone(),
    };
    let mut list = lock(configured);
    list.retain(|(c, _)| c.serial != done.serial);
    list.push((done, port));
    Ok(dto)
}

fn revert_with(
    adb: &dyn Adb,
    configured: &Mutex<Vec<(Configured, u16)>>,
    serial: &str,
) -> Result<(), String> {
    let known = lock(configured)
        .iter()
        .find(|(c, _)| c.serial == serial)
        .map(|(c, port)| (c.reverse, *port));
    // Sin registro (configurado por otra sesión), se quita el proxy y el reverse del puerto por defecto.
    let (reverse, port) =
        known.unwrap_or((!serial.starts_with("emulator-"), proxyrr_core::DEFAULT_PORT));
    android::revert(adb, serial, port, reverse)?;
    lock(configured).retain(|(c, _)| c.serial != serial);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io;

    use proxyrr_devices::android::AdbOutput;

    use super::*;

    #[derive(Debug, Default)]
    struct FakeAdb {
        calls: Mutex<Vec<String>>,
    }

    impl Adb for FakeAdb {
        fn run(&self, args: &[&str]) -> io::Result<AdbOutput> {
            let line = args.join(" ");
            lock(&self.calls).push(line.clone());
            let stdout = if line == "devices -l" {
                "List of devices attached\nR58M1 device model:SM_G990\n"
            } else if line.ends_with("getprop") {
                "[ro.build.type]: [user]\n[ro.build.version.sdk]: [34]\n"
            } else {
                ""
            };
            Ok(AdbOutput {
                success: true,
                stdout: stdout.to_owned(),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn remembers_configured_devices_and_reverts_them() {
        let adb = FakeAdb::default();
        let configured = Mutex::new(Vec::new());
        let ca = CertificateAuthority::generate().unwrap();
        let done = configure_with(&adb, &configured, &ca, "R58M1", 9191).unwrap();
        assert_eq!(done.proxy, "127.0.0.1:9191");
        assert!(!done.system_ca);
        assert_eq!(lock(&configured).len(), 1);
        revert_with(&adb, &configured, "R58M1").unwrap();
        assert!(lock(&configured).is_empty());
        let calls = lock(&adb.calls).clone();
        assert!(
            calls.contains(&"-s R58M1 reverse --remove tcp:9191".to_owned()),
            "{calls:?}"
        );
    }

    #[test]
    fn adb_path_is_validated_and_saved() {
        let dir = tempfile::tempdir().unwrap();
        let android = Android::new(dir.path());
        assert!(android.set_adb_path("/no/existe/adb").is_err());
        let fake = dir.path().join("adb-falso");
        std::fs::write(&fake, b"").unwrap();
        android.set_adb_path(&fake.display().to_string()).unwrap();
        // Una instancia nueva la lee del archivo.
        let again = Android::new(dir.path());
        assert_eq!(lock(&again.adb_override).as_deref(), Some(fake.as_path()));
        android.set_adb_path("").unwrap();
        assert_eq!(*lock(&Android::new(dir.path()).adb_override), None);
    }
}
