//! `proxyrr android …` y `proxyrr start --android` (spec 0002).

use std::path::Path;

use proxyrr_cert::CertificateAuthority;
use proxyrr_devices::android::{self, AdbCli, CaMode, CaPayload, Configured, Device, DeviceState};

/// Abre `adb` (ruta explícita, `PROXYRR_ADB`, SDK o `PATH`).
pub fn adb(explicit: Option<&Path>) -> Result<AdbCli, String> {
    android::locate_adb(explicit)
        .map(AdbCli::new)
        .map_err(|e| e.to_string())
}

/// `proxyrr android devices`.
pub fn print_devices(adb: &AdbCli) -> Result<(), String> {
    let devices = android::list_devices(adb)?;
    if devices.is_empty() {
        println!(
            "No hay emuladores ni dispositivos conectados (adb: {}).",
            adb.path().display()
        );
        println!(
            "Abrí un emulador desde Android Studio, o conectá un teléfono con la depuración USB activada."
        );
        return Ok(());
    }
    for device in &devices {
        println!("{}", describe(device));
    }
    Ok(())
}

fn describe(device: &Device) -> String {
    let kind = if device.emulator {
        "emulador"
    } else {
        "dispositivo"
    };
    let mode = match (&device.state, device.rootable) {
        (DeviceState::Ready, true) => "CA de sistema (todas las apps)",
        (DeviceState::Ready, false) => "CA de usuario (Chrome y apps propias)",
        (DeviceState::Unauthorized, _) => "sin autorizar: aceptá la depuración USB en el teléfono",
        (DeviceState::Offline, _) => "desconectado",
        (DeviceState::Other(_), _) => "no disponible",
    };
    format!(
        "{:<20} {kind:<11} {:<36} {mode}",
        device.serial,
        device.label()
    )
}

/// Elige el dispositivo: el pedido por serial, o el único conectado y listo.
pub fn pick(adb: &AdbCli, serial: Option<&str>) -> Result<Device, String> {
    let devices = android::list_devices(adb)?;
    if let Some(serial) = serial.filter(|s| *s != "auto") {
        return devices
            .into_iter()
            .find(|d| d.serial == serial)
            .ok_or_else(|| {
                format!("no hay ningún dispositivo con serial {serial} (`proxyrr android devices`)")
            });
    }
    let mut ready: Vec<Device> = devices
        .into_iter()
        .filter(|d| d.state == DeviceState::Ready)
        .collect();
    match ready.len() {
        0 => Err("no hay emuladores ni dispositivos listos (`proxyrr android devices`)".to_owned()),
        1 => Ok(ready.remove(0)),
        _ => Err(format!(
            "hay {} dispositivos; elegí uno con su serial: {}",
            ready.len(),
            ready
                .iter()
                .map(|d| d.serial.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Configura `device` para el proxy en `port` e imprime qué quedó hecho.
pub fn setup(
    adb: &AdbCli,
    device: &Device,
    ca: &CertificateAuthority,
    port: u16,
) -> Result<Configured, String> {
    let info = ca.info().map_err(|e| e.to_string())?;
    let name = format!("{:08x}.0", info.subject_hash_old);
    println!("Configurando {} …", device.label());
    let done = android::configure(
        adb,
        device,
        CaPayload {
            pem: ca.cert_pem(),
            android_name: &name,
        },
        port,
    )?;
    println!("  proxy del dispositivo: {}", done.proxy);
    match &done.ca {
        CaMode::System => println!(
            "  CA de ProxyRR en el store del sistema: todas las apps confían (hasta reiniciar el emulador)"
        ),
        CaMode::UserManual { path } => println!("  CA copiada a {path}"),
    }
    for note in &done.notes {
        println!("  • {note}");
    }
    Ok(done)
}

/// Quita el proxy del dispositivo.
pub fn revert(adb: &AdbCli, configured: &Configured, port: u16) -> Result<(), String> {
    android::revert(adb, &configured.serial, port, configured.reverse)?;
    println!("Proxy quitado de {}.", configured.serial);
    Ok(())
}

/// `proxyrr android revert [serial]`: sin `Configured` guardado, se asume `reverse` en físicos.
pub fn revert_device(adb: &AdbCli, device: &Device, port: u16) -> Result<(), String> {
    android::revert(adb, &device.serial, port, !device.emulator)?;
    println!("Proxy quitado de {}.", device.label());
    Ok(())
}
