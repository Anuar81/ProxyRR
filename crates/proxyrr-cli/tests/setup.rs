//! `proxyrr ca install|uninstall|status`, `proxyrr setup` y la página `proxyrr.cert` contra el binario
//! real (spec 0006). Instalar de verdad no se prueba acá: tocaría el almacén del equipo que corre los
//! tests. Se prueba el plan (`--dry-run`) y, en `proxyrr-devices`, cada plan por SO.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const LIMIT: Duration = Duration::from_secs(20);

fn proxyrr(data_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_proxyrr"))
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    assert!(
        o.status.success(),
        "falló: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn sha256(data_dir: &Path) -> String {
    let info = stdout(&proxyrr(data_dir, &["ca", "info"]));
    info.lines()
        .find_map(|l| l.strip_prefix("SHA-256:"))
        .unwrap()
        .trim()
        .to_owned()
}

#[test]
fn install_dry_run_shows_the_plan_and_changes_nothing() {
    let data = tempfile::tempdir().unwrap();
    let output = proxyrr(data.path(), &["ca", "install", "--dry-run"]);
    if cfg!(target_os = "linux") && !output.status.success() {
        // Linux sin update-ca-certificates ni trust: el error tiene que decir qué instalar.
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("ca-certificates"), "{stderr}");
        return;
    }
    let text = stdout(&output);
    assert!(
        text.contains("quien tenga su clave privada"),
        "advertencia previa: {text}"
    );
    assert!(text.contains(&sha256(data.path())), "{text}");
    assert!(text.contains("--dry-run: no se ejecutó nada"), "{text}");
    let expected = if cfg!(windows) {
        "certutil -user -addstore Root"
    } else if cfg!(target_os = "macos") {
        "security add-trusted-cert -r trustRoot -p ssl"
    } else {
        "sudo "
    };
    assert!(text.contains(expected), "{text}");
}

#[test]
fn uninstall_dry_run_targets_this_ca_by_fingerprint() {
    let data = tempfile::tempdir().unwrap();
    let output = proxyrr(data.path(), &["ca", "uninstall", "--dry-run"]);
    if cfg!(target_os = "linux") && !output.status.success() {
        return;
    }
    let text = stdout(&output);
    let sha1 = text
        .lines()
        .next()
        .and_then(|l| l.split("SHA-1 ").nth(1))
        .map_or_else(|| panic!("{text}"), |s| s.trim_end_matches("):").to_owned());
    assert_eq!(sha1.len(), 40, "{text}");
    if !cfg!(target_os = "linux") {
        assert!(
            text.matches(&sha1).count() >= 2,
            "el comando usa la huella: {text}"
        );
    }
}

#[test]
fn status_only_reads() {
    let data = tempfile::tempdir().unwrap();
    let text = stdout(&proxyrr(data.path(), &["ca", "status"]));
    assert!(text.contains(&sha256(data.path())), "{text}");
    // Una CA recién creada en un directorio temporal no puede estar instalada.
    assert!(!text.contains("[x]"), "{text}");
}

#[test]
fn setup_ios_uses_the_given_host_and_prints_a_qr() {
    let data = tempfile::tempdir().unwrap();
    let text = stdout(&proxyrr(
        data.path(),
        &["setup", "ios", "--host", "192.168.1.50", "--port", "9191"],
    ));
    assert!(
        text.contains("servidor 192.168.1.50, puerto 9191"),
        "{text}"
    );
    assert!(text.contains("0.0.0.0:9191"), "{text}");
    assert!(
        text.contains("Ajustes de confianza de certificados"),
        "{text}"
    );
    assert!(text.contains("http://192.168.1.50:9191/cert"), "{text}");
    assert!(text.contains('█') || text.contains('▀'), "QR: {text}");

    let quiet = stdout(&proxyrr(
        data.path(),
        &["setup", "ios", "--host", "192.168.1.50", "--no-qr"],
    ));
    assert!(!quiet.contains("Escaneá"), "{quiet}");
}

#[test]
fn setup_android_device_includes_network_security_config() {
    let data = tempfile::tempdir().unwrap();
    let text = stdout(&proxyrr(
        data.path(),
        &["setup", "android-device", "--host", "10.1.1.2", "--no-qr"],
    ));
    assert!(text.contains("<debug-overrides>"), "{text}");
    assert!(text.contains("android:networkSecurityConfig"), "{text}");
    assert!(text.contains("10.1.1.2"), "{text}");
}

#[test]
fn install_flag_is_refused_where_it_makes_no_sense() {
    let data = tempfile::tempdir().unwrap();
    let output = proxyrr(data.path(), &["setup", "ios", "--install", "--no-qr"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("confirma el usuario"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Con el proxy corriendo, `http://proxyrr.cert/ca.pem` a través del proxy devuelve la CA del disco,
/// y `/cert` directo al proxy devuelve la página.
#[test]
fn start_serves_the_ca_page() {
    let data = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_proxyrr"))
        .args(["start", "--listen", "127.0.0.1:0", "--data-dir"])
        .arg(data.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let banner = lines.recv_timeout(LIMIT).unwrap();
    let proxy = banner
        .strip_prefix("ProxyRR escuchando en ")
        .and_then(|r| r.split(' ').next())
        .unwrap()
        .to_owned();

    let get = |request: &str| {
        let mut stream = TcpStream::connect(&proxy).unwrap();
        stream.set_read_timeout(Some(LIMIT)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        reply
    };
    let pem = get(
        "GET http://proxyrr.cert/ca.pem HTTP/1.1\r\nHost: proxyrr.cert\r\nConnection: close\r\n\r\n",
    );
    let on_disk = std::fs::read_to_string(data.path().join("ca.pem")).unwrap();
    assert!(pem.starts_with("HTTP/1.1 200"), "{pem}");
    assert!(
        pem.ends_with(&on_disk),
        "sirve la misma CA que está en disco"
    );
    assert!(!pem.contains("PRIVATE"));

    let page = get(&format!(
        "GET /cert HTTP/1.1\r\nHost: {proxy}\r\nUser-Agent: Mozilla/5.0 (Linux; Android 14)\r\nConnection: close\r\n\r\n"
    ));
    assert!(page.contains("text/html"), "{page}");
    assert!(page.contains(r#"href="/cert/ca.crt""#), "{page}");
    let _ = child.kill();
    let _ = child.wait();
}
