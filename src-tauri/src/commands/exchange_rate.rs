use tauri::State;

use crate::services::exchange_rate::{self, UsdCnyRate};
use crate::store::AppState;

#[tauri::command]
pub async fn get_usd_cny_rate(state: State<'_, AppState>) -> Result<UsdCnyRate, String> {
    Ok(exchange_rate::get_usd_cny_rate(state.db.as_ref()).await)
}
