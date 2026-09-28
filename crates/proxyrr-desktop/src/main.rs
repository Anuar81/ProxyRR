//! App de escritorio de ProxyRR (spec 0009).
//!
//! La ventana habla con el motor por IPC de Tauri (comandos + un evento de avisos), no por la API HTTP:
//! así no hay puerto ni token expuestos a otras apps del equipo. La API sigue para CLI y scripts.

// En Windows, sin consola detrás de la ventana en release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(
    clippy::needless_pass_by_value,
    reason = "Tauri inyecta `State` y los argumentos de los comandos por valor"
)]

use std::sync::Arc;

use proxyrr_api::{FlowDto, FlowSummaryDto, ProxyStatusDto, StatusDto, WsMessage};
use tauri::{Emitter, Manager, State};
use tokio::sync::broadcast::error::RecvError;

mod backend;

use backend::{Backend, BodyView, CaOverview, GuideDto, Side, StartRequest};

/// Evento con los avisos del engine (mismo formato que el WebSocket de la API).
const NOTICE_EVENT: &str = "proxyrr://notice";

type AppState<'a> = State<'a, Arc<Backend>>;

#[tauri::command]
async fn status(state: AppState<'_>) -> Result<StatusDto, String> {
    Ok(state.status().await)
}

#[tauri::command]
async fn start_proxy(state: AppState<'_>, request: StartRequest) -> Result<ProxyStatusDto, String> {
    state.start_proxy(request).await
}

#[tauri::command]
async fn stop_proxy(state: AppState<'_>) -> Result<ProxyStatusDto, String> {
    Ok(state.stop_proxy().await)
}

#[tauri::command]
fn list_flows(state: AppState<'_>, after: Option<u64>) -> Vec<FlowSummaryDto> {
    state.list_flows(after)
}

#[tauri::command]
fn get_flow(state: AppState<'_>, id: u64) -> Result<FlowDto, String> {
    state.get_flow(id)
}

#[tauri::command]
fn get_body(state: AppState<'_>, id: u64, side: Side) -> Result<BodyView, String> {
    state.get_body(id, side)
}

#[tauri::command]
fn clear_flows(state: AppState<'_>) {
    state.clear();
}

/// Los comandos del SO (certutil, security, pkexec) pueden abrir diálogos y tardar: fuera del hilo de la UI.
async fn blocking<T: Send + 'static>(
    state: &Arc<Backend>,
    f: impl FnOnce(&Backend) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let backend = Arc::clone(state);
    tauri::async_runtime::spawn_blocking(move || f(&backend))
        .await
        .map_err(|e| format!("tarea interrumpida: {e}"))?
}

#[tauri::command]
async fn ca_overview(state: AppState<'_>) -> Result<CaOverview, String> {
    blocking(&state, |b| Ok(b.ca_overview())).await
}

#[tauri::command]
async fn ca_install(state: AppState<'_>) -> Result<CaOverview, String> {
    blocking(&state, Backend::ca_install).await
}

#[tauri::command]
async fn ca_uninstall(state: AppState<'_>) -> Result<CaOverview, String> {
    blocking(&state, Backend::ca_uninstall).await
}

#[tauri::command]
fn guide(state: AppState<'_>, target: String, port: u16) -> Result<GuideDto, String> {
    state.guide(&target, port)
}

#[tauri::command]
async fn install_ios_simulator(state: AppState<'_>) -> Result<(), String> {
    blocking(&state, Backend::install_ios_simulator).await
}

fn main() {
    let data_dir = proxyrr_cert::default_data_dir()
        .expect("no se pudo determinar el directorio de datos de ProxyRR");
    let backend = Arc::new(
        Backend::new(&data_dir)
            .unwrap_or_else(|e| panic!("no se pudo cargar la CA de {}: {e}", data_dir.display())),
    );
    tauri::Builder::default()
        .manage(Arc::clone(&backend))
        .setup(move |app| {
            let autostart = Arc::clone(&backend);
            let handle = app.handle().clone();
            let mut notices = backend.subscribe();
            tauri::async_runtime::spawn(async move {
                loop {
                    let message = match notices.recv().await {
                        Ok(notice) => WsMessage::from(notice),
                        Err(RecvError::Lagged(missed)) => WsMessage::Lagged { missed },
                        Err(RecvError::Closed) => break,
                    };
                    let _ = handle.emit(NOTICE_EVENT, message);
                }
            });
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(&format!("ProxyRR {}", env!("CARGO_PKG_VERSION")));
            }
            // Para desarrollo y pruebas: `PROXYRR_AUTOSTART=127.0.0.1:9090` prende el proxy al abrir.
            if let Some(listen) = std::env::var("PROXYRR_AUTOSTART")
                .ok()
                .filter(|v| !v.is_empty())
            {
                let backend = Arc::clone(&autostart);
                tauri::async_runtime::spawn(async move {
                    let request = StartRequest {
                        listen,
                        mitm: false,
                        bypass: Vec::new(),
                    };
                    if let Err(e) = backend.start_proxy(request).await {
                        eprintln!("PROXYRR_AUTOSTART: {e}");
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            status,
            start_proxy,
            stop_proxy,
            list_flows,
            get_flow,
            get_body,
            clear_flows,
            ca_overview,
            ca_install,
            ca_uninstall,
            guide,
            install_ios_simulator,
        ])
        .run(tauri::generate_context!())
        .expect("no se pudo abrir la ventana de ProxyRR");
}
