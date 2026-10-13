use tauri::State;

use crate::services::oss_backup::{self, OssBackupResult};
use crate::settings;
use crate::store::AppState;

#[tauri::command]
pub async fn upload_sql_backup_to_oss(
    state: State<'_, AppState>,
    file_path: Option<String>,
) -> Result<OssBackupResult, String> {
    // A manual SQL backup uses saved cloud credentials without enabling sync.
    let mut config = settings::get_s3_sync_settings()
        .ok_or_else(|| "请先在云同步中保存 S3／OSS 配置".to_string())?;
    config.normalize();
    oss_backup::credentials(&config).map_err(|e| e.to_string())?;
    let db = state.db.clone();
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        oss_backup::sql_bytes(&db, file_path.as_deref().map(std::path::Path::new))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    oss_backup::upload(&config, bytes)
        .await
        .map_err(|e| e.to_string())
}
