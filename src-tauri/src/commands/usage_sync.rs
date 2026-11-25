use tauri::State;

use crate::services::usage_sync::{self, UsageSyncStatus};
use crate::settings::{self, UsageSyncSettings};
use crate::store::AppState;

#[tauri::command]
pub fn usage_sync_status(state: State<'_, AppState>) -> Result<UsageSyncStatus, String> {
    usage_sync::status(&state.db).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn usage_sync_save_settings(
    state: State<'_, AppState>,
    enabled: bool,
    auto_sync: bool,
) -> Result<UsageSyncStatus, String> {
    settings::set_usage_sync_settings(UsageSyncSettings { enabled, auto_sync })
        .map_err(|error| error.to_string())?;
    usage_sync::status(&state.db).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn usage_sync_now(state: State<'_, AppState>) -> Result<UsageSyncStatus, String> {
    usage_sync::sync_now(&state.db)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn usage_sync_forget_device(
    state: State<'_, AppState>,
    device_id: String,
) -> Result<(), String> {
    usage_sync::forget_peer(&state.db, &device_id).map_err(|error| error.to_string())
}
