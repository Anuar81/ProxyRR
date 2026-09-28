//! Compara `subject_hash_old` con el binario `openssl` real (spec 0003, CA 9).
//! Se saltea si `openssl` no está disponible (en CI de Linux/macOS sí está).

use std::process::Command;

use proxyrr_cert::{CertificateAuthority, android_cert_filename};

fn openssl() -> Option<Command> {
    let candidates = [
        "openssl",
        r"C:\Program Files\Git\usr\bin\openssl.exe",
        r"C:\Program Files\Git\mingw64\bin\openssl.exe",
    ];
    candidates.into_iter().find_map(|bin| {
        Command::new(bin)
            .arg("version")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|_| Command::new(bin))
    })
}

#[test]
fn matches_openssl_binary() {
    let Some(mut cmd) = openssl() else {
        eprintln!("openssl no encontrado: test salteado");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let ca = CertificateAuthority::load_or_create(dir.path()).unwrap();

    let out = cmd
        .args(["x509", "-noout", "-subject_hash_old", "-in"])
        .arg(dir.path().join(proxyrr_cert::CA_CERT_FILE))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let expected = String::from_utf8(out.stdout).unwrap();

    assert_eq!(
        android_cert_filename(ca.cert_der()).unwrap(),
        format!("{}.0", expected.trim())
    );
}
