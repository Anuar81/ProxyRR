//! Directorio de datos de ProxyRR (CA, sesiones), compartido por el CLI y la app de escritorio.

use std::path::PathBuf;

/// Directorio de datos por SO, sin dependencias externas:
/// Windows `%APPDATA%\ProxyRR`, macOS `~/Library/Application Support/ProxyRR`,
/// Linux/otros `$XDG_DATA_HOME/proxyrr` o `~/.local/share/proxyrr`. `None` si no hay variables de entorno
/// con qué calcularlo.
#[must_use]
pub fn default_data_dir() -> Option<PathBuf> {
    let env_dir = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        env_dir("APPDATA").map(|d| d.join("ProxyRR"))
    } else if cfg!(target_os = "macos") {
        env_dir("HOME").map(|h| h.join("Library/Application Support/ProxyRR"))
    } else {
        env_dir("XDG_DATA_HOME")
            .or_else(|| env_dir("HOME").map(|h| h.join(".local/share")))
            .map(|d| d.join("proxyrr"))
    }
}
