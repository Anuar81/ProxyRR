//! Test de humo: ejecuta el binario real y verifica `--version` (spec 0001, CA 7).

use std::process::Command;

#[test]
fn prints_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_proxyrr"))
        .arg("--version")
        .output()
        .expect("no se pudo ejecutar el binario proxyrr");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("stdout no es UTF-8");
    assert_eq!(
        stdout.trim(),
        format!("proxyrr {}", env!("CARGO_PKG_VERSION"))
    );
}
