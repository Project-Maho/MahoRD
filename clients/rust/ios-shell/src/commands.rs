use maho_app::{IpcError, IpcErrorStage, PairingStore, PairingSummary};
use serde::Deserialize;
use tauri::State;

use crate::qa::{check_qa_provisioning, StartupResponse};
use crate::state::{build_session_config, AppState, SessionStats};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TouchEventPayload {
    pub id: u64,
    pub x: f32,
    pub y: f32,
    pub phase: String,
}

#[tauri::command]
pub async fn list_hosts(
    state: State<'_, AppState>,
) -> Result<Vec<maho_net::discovery::DiscoveredHost>, String> {
    state.list_discovered_hosts().await
}

#[tauri::command]
pub async fn stop_discovery(state: State<'_, AppState>) -> Result<(), String> {
    state.stop_discovery().await
}

#[tauri::command]
pub fn list_pairings() -> Result<Vec<PairingSummary>, IpcError> {
    let store = PairingStore::open_default().map_err(|e| {
        IpcError::connection_failed(IpcErrorStage::Client, format!("Pairing store error: {e}"))
    })?;
    let records = store.load_all().map_err(|e| {
        IpcError::connection_failed(IpcErrorStage::Client, format!("Load pairings error: {e}"))
    })?;
    Ok(records.into_iter().map(PairingSummary::from).collect())
}

#[tauri::command]
pub fn forget_pairing(id: String) -> Result<(), IpcError> {
    let store = PairingStore::open_default().map_err(|e| {
        IpcError::connection_failed(IpcErrorStage::Client, format!("Pairing store error: {e}"))
    })?;
    store.delete(&id).map_err(|e| {
        IpcError::connection_failed(IpcErrorStage::Client, format!("Delete pairing error: {e}"))
    })
}

#[tauri::command]
pub async fn connect(
    state: State<'_, AppState>,
    host: String,
    tcp_port: Option<u16>,
    udp_port: Option<u16>,
    pin: Option<String>,
    pairing_id: Option<String>,
) -> Result<SessionStats, IpcError> {
    if let Some(ref p) = pin {
        let trimmed_pin = p.trim();
        if !trimmed_pin.is_empty()
            && (trimmed_pin.len() != 8 || !trimmed_pin.chars().all(|c| c.is_ascii_digit()))
        {
            return Err(IpcError::invalid_pin("PIN must be exactly 8 ASCII digits"));
        }
    }
    let trimmed_pin = pin.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let trimmed_id = pairing_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if trimmed_pin.is_none() && trimmed_id.is_none() {
        return Err(IpcError::pairing_required(
            "PIN required for initial authorization",
        ));
    }

    let _ = build_session_config(&host, tcp_port, udp_port, "MahoRD iOS")
        .map_err(|e| IpcError::connection_failed(IpcErrorStage::Client, e))?;
    state
        .connect_async(host, tcp_port, udp_port, pin, pairing_id)
        .await
}

#[tauri::command]
pub async fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    state.disconnect_async().await
}

#[tauri::command]
pub fn stats(state: State<'_, AppState>) -> Result<SessionStats, String> {
    state.stats()
}

#[tauri::command]
pub fn poll_frame(state: State<'_, AppState>) -> Result<tauri::ipc::Response, String> {
    match state.poll_frame()? {
        Some(frame_bytes) => Ok(tauri::ipc::Response::new((*frame_bytes).clone())),
        None => Ok(tauri::ipc::Response::new(Vec::new())),
    }
}

#[tauri::command]
pub fn touch(state: State<'_, AppState>, event: TouchEventPayload) -> Result<(), String> {
    state.handle_touch(event.id, event.x, event.y, &event.phase)
}

#[tauri::command]
pub fn set_touch_mode(state: State<'_, AppState>, mode: String) -> Result<(), String> {
    state.set_touch_mode(&mode)
}

#[tauri::command]
pub fn send_key(
    state: State<'_, AppState>,
    key_code: u16,
    down: bool,
    modifiers: u16,
) -> Result<(), String> {
    state.handle_key(key_code, down, modifiers)
}

#[tauri::command]
pub fn set_muted(state: State<'_, AppState>, muted: bool) -> Result<(), String> {
    state.set_muted(muted)
}

#[tauri::command]
pub fn presented(state: State<'_, AppState>, sequence: u64) -> Result<(), String> {
    state.presented(sequence)
}

#[tauri::command]
pub fn startup() -> Result<StartupResponse, String> {
    Ok(check_qa_provisioning())
}
