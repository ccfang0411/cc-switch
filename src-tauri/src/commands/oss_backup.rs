use serde_json::Value;
use tauri::State;

use crate::commands::sync_support::{
    post_sync_warning_from_result, run_post_import_sync, success_payload_with_warning,
};
use crate::error::AppError;
use crate::services::oss_backup::{self, OssBackupResult, RemoteSqlBackup};
use crate::services::skill::skill_state_write_guard;
use crate::services::sync_protocol::sync_mutex;
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

/// List SQL backups from every host under the shared OSS backup prefix.
#[tauri::command]
pub async fn list_sql_backups_from_oss() -> Result<Vec<RemoteSqlBackup>, String> {
    let mut config = settings::get_s3_sync_settings()
        .ok_or_else(|| "请先在云同步中保存 S3／OSS 配置".to_string())?;
    config.normalize();
    oss_backup::credentials(&config).map_err(|e| e.to_string())?;
    oss_backup::list(&config).await.map_err(|e| e.to_string())
}

/// Download a listed backup and import it with the same safety pipeline as a local SQL import.
#[tauri::command]
pub async fn import_sql_backup_from_oss(
    key: String,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    let mut config = settings::get_s3_sync_settings()
        .ok_or_else(|| "请先在云同步中保存 S3／OSS 配置".to_string())?;
    config.normalize();
    let bytes = oss_backup::download(&config, &key)
        .await
        .map_err(|e| e.to_string())?;

    let app_state_for_sync = state.inner().clone();
    let db = app_state_for_sync.db.clone();
    let _sync_guard = sync_mutex().lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        use std::io::Write;
        let mut temp = tempfile::NamedTempFile::new().map_err(|e| AppError::IoContext {
            context: "创建临时 SQL 文件失败".to_string(),
            source: e,
        })?;
        temp.write_all(&bytes).map_err(|e| AppError::IoContext {
            context: "写入临时 SQL 文件失败".to_string(),
            source: e,
        })?;
        let backup_id = {
            // SQL restore replaces the `skills` table; exclude local Skill mutations.
            let _skill_state_guard = skill_state_write_guard();
            db.import_sql(temp.path())?
        };
        let warning = post_sync_warning_from_result(Ok(run_post_import_sync(&app_state_for_sync)));
        if let Some(msg) = warning.as_ref() {
            log::warn!("[OSS Import] post-import sync warning: {msg}");
        }
        Ok::<_, AppError>(success_payload_with_warning(backup_id, warning))
    })
    .await
    .map_err(|e| format!("导入 OSS 备份失败: {e}"))?
    .map_err(|e: AppError| e.to_string())
}
