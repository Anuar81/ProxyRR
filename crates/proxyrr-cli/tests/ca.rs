//! `proxyrr ca ...` contra el binario real (spec 0003, CA 10).

use std::path::Path;
use std::process::{Command, Output};

fn proxyrr(data_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_proxyrr"))
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .output()
        .expect("no se pudo ejecutar proxyrr")
}

fn stdout(o: &Output) -> String {
    assert!(
        o.status.success(),
        "falló: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn fingerprint(info: &str) -> String {
    info.lines()
        .find(|l| l.starts_with("SHA-256:"))
        .expect("línea SHA-256")
        .to_owned()
}

#[test]
fn info_creates_ca_once() {
    let dir = tempfile::tempdir().unwrap();
    let first = stdout(&proxyrr(dir.path(), &["ca", "info"]));
    assert!(first.contains("ProxyRR CA ("), "{first}");
    assert!(dir.path().join("ca.pem").exists());
    assert!(dir.path().join("ca.key.pem").exists());

    let second = stdout(&proxyrr(dir.path(), &["ca", "info"]));
    assert_eq!(fingerprint(&first), fingerprint(&second));
}

#[test]
fn export_pem_to_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let pem = stdout(&proxyrr(dir.path(), &["ca", "export"]));
    assert!(pem.starts_with("-----BEGIN CERTIFICATE-----"));
    assert_eq!(
        pem,
        std::fs::read_to_string(dir.path().join("ca.pem")).unwrap()
    );
}

#[test]
fn export_der_to_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("ca.der");
    let out_str = out.to_str().unwrap();
    stdout(&proxyrr(
        dir.path(),
        &["ca", "export", "--der", "--out", out_str],
    ));
    let der = std::fs::read(&out).unwrap();
    assert_eq!(der.first(), Some(&0x30), "DER empieza con SEQUENCE");
}

#[test]
fn export_der_without_out_fails() {
    let dir = tempfile::tempdir().unwrap();
    let o = proxyrr(dir.path(), &["ca", "export", "--der"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("--out"));
}

#[test]
fn path_prints_data_dir() {
    let dir = tempfile::tempdir().unwrap();
    let out = stdout(&proxyrr(dir.path(), &["ca", "path"]));
    assert_eq!(out.trim(), dir.path().display().to_string());
}

#[test]
fn corrupt_ca_reports_error_and_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ca.pem"), "basura").unwrap();
    std::fs::write(dir.path().join("ca.key.pem"), "basura").unwrap();
    let o = proxyrr(dir.path(), &["ca", "info"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).starts_with("error:"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("ca.pem")).unwrap(),
        "basura"
    );
}
