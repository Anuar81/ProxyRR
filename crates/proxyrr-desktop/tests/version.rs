//! La versión sale de un solo lugar: el `Cargo.toml` del workspace (spec 0013, criterio 1).

/// `tauri.conf.json` no repite la versión (Tauri toma la del crate); si alguien la agrega, tiene que
/// coincidir con la del workspace o el instalador saldría con otra.
#[test]
fn version_matches_workspace() {
    let conf: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json válido");
    if let Some(version) = conf.get("version") {
        assert_eq!(
            version.as_str(),
            Some(env!("CARGO_PKG_VERSION")),
            "tauri.conf.json tiene otra versión que el workspace"
        );
    }
    assert_eq!(
        conf["identifier"], "com.proxyrr.desktop",
        "el identificador del bundle no cambia entre versiones"
    );
}
