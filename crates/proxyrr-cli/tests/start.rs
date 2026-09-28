//! `proxyrr start` contra el binario real (CA 8–9 de la spec 0004).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const LIMIT: Duration = Duration::from_secs(20);

/// Mata el proxy aunque el test falle a mitad de camino.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Origen mínimo con std: responde `hola` a una sola conexión.
fn start_origin() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nhola");
    });
    addr
}

#[test]
fn start_prints_address_and_logs_requests() {
    let child = Command::new(env!("CARGO_BIN_EXE_proxyrr"))
        .args(["start", "--listen", "127.0.0.1:0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut running = Running(child);

    // Las líneas de stdout llegan por un canal para poder cortar con timeout.
    let stdout = running.0.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });

    let banner = lines.recv_timeout(LIMIT).expect("no imprimió la dirección");
    let proxy = banner
        .strip_prefix("ProxyRR escuchando en ")
        .and_then(|rest| rest.split(' ').next())
        .unwrap_or_else(|| panic!("banner inesperado: {banner}"))
        .to_owned();
    assert!(
        !proxy.ends_with(":0"),
        "debe mostrar el puerto real: {proxy}"
    );

    let origin = start_origin();
    let mut stream = TcpStream::connect(&proxy).unwrap();
    stream.set_read_timeout(Some(LIMIT)).unwrap();
    write!(
        stream,
        "GET http://{origin}/ HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(reply.ends_with("hola"), "{reply}");

    let expected = format!("http://{origin}/");
    let logged = loop {
        let line = lines.recv_timeout(LIMIT).expect("no registró el request");
        if line.starts_with('#') {
            break line;
        }
    };
    assert!(logged.contains("GET"), "{logged}");
    assert!(logged.contains(" 200 "), "{logged}");
    assert!(logged.contains(&expected), "{logged}");
    assert!(logged.ends_with("4 B"), "{logged}");
}

#[test]
fn non_loopback_listen_warns() {
    let child = Command::new(env!("CARGO_BIN_EXE_proxyrr"))
        .args(["start", "--listen", "0.0.0.0:0"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut running = Running(child);
    let stderr = running.0.stderr.take().unwrap();
    let (tx, lines) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let warning = lines.recv_timeout(LIMIT).expect("no advirtió");
    assert!(warning.contains("cualquiera en tu red"), "{warning}");
}

#[test]
fn busy_port_fails_with_clear_error() {
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = taken.local_addr().unwrap().to_string();
    let output = Command::new(env!("CARGO_BIN_EXE_proxyrr"))
        .args(["start", "--listen", &addr])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no se pudo escuchar"), "{stderr}");
}

#[test]
fn mitm_uses_ca_from_data_dir_and_shows_fingerprint() {
    let data = tempfile::tempdir().unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_proxyrr"))
        .args(["start", "--listen", "127.0.0.1:0", "--mitm", "--bypass"])
        .arg("*.apple.com")
        .arg("--data-dir")
        .arg(data.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut running = Running(child);
    let stdout = running.0.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let mut seen = Vec::new();
    while !seen.iter().any(|l: &String| l.starts_with("Sin descifrar")) {
        seen.push(
            lines
                .recv_timeout(LIMIT)
                .expect("faltan líneas de arranque"),
        );
    }
    let banner = seen.join("\n");
    assert!(
        banner.contains("Descifrando HTTPS con la CA CN=ProxyRR CA ("),
        "{banner}"
    );
    assert!(banner.contains("SHA-256"), "{banner}");
    assert!(banner.contains("*.apple.com"), "{banner}");
    assert!(
        data.path().join("ca.pem").exists(),
        "la CA debe crearse en --data-dir"
    );
}

#[test]
fn bypass_requires_mitm() {
    let output = Command::new(env!("CARGO_BIN_EXE_proxyrr"))
        .args(["start", "--bypass", "example.com"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--mitm"), "{stderr}");
}
