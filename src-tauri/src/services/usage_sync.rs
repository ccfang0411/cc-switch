//! 多终端用量汇总：每台设备把“仅用量”快照传到同一个 S3/OSS 前缀，
//! 其他设备拉取后合并进本机 `proxy_request_logs` / `usage_daily_rollups`。
//!
//! 快照不包含配置、密钥或提示词。对端明细的 request_id 统一加
//! `peer:<device>:<原ID>` 前缀，并用 `session_usage_dedup` 做永久回执，
//! 避免明细被 30 天清理后重复计入。

use std::collections::HashSet;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::TimeZone;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::services::s3;
use crate::services::sync_protocol::sha256_hex;
use crate::settings::{self, S3SyncSettings, UsageSyncSettings};

pub(crate) const FORMAT: &str = "cc-switch-usage-sync";
pub(crate) const VERSION: u32 = 1;
const PREFIX: &str = "usage-sync/v1";
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_SNAPSHOT_BYTES: u64 = 512 * 1024 * 1024;
const PEER_DATA_SOURCE: &str = "peer_usage";
const AUTO_SYNC_INTERVAL_SECS: u64 = 15 * 60;
const STARTUP_DELAY_SECS: u64 = 30;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSyncDevice {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSyncPeerStatus {
    pub device_id: String,
    pub name: String,
    pub last_sync_at: Option<i64>,
    pub imported_details: i64,
    pub imported_rollups: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSyncStatus {
    pub enabled: bool,
    pub auto_sync: bool,
    pub device_id: String,
    pub device_name: String,
    pub peers: Vec<UsageSyncPeerStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageManifest {
    format: String,
    version: u32,
    device_id: String,
    device_name: String,
    generated_at: i64,
    detail_cutoff: Option<i64>,
    detail_cutoff_date: Option<String>,
    detail_count: usize,
    rollup_count: usize,
    snapshot_sha256: String,
    snapshot_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageSnapshot {
    format: String,
    version: u32,
    device_id: String,
    device_name: String,
    generated_at: i64,
    details: Vec<SnapshotDetail>,
    rollups: Vec<SnapshotRollup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotDetail {
    request_id: String,
    provider_id: String,
    app_type: String,
    model: String,
    request_model: Option<String>,
    pricing_model: Option<String>,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    input_token_semantics: i64,
    input_cost_usd: String,
    output_cost_usd: String,
    cache_read_cost_usd: String,
    cache_creation_cost_usd: String,
    total_cost_usd: String,
    latency_ms: i64,
    first_token_ms: Option<i64>,
    duration_ms: Option<i64>,
    status_code: i64,
    error_message: Option<String>,
    session_id: Option<String>,
    provider_type: Option<String>,
    is_streaming: bool,
    cost_multiplier: String,
    created_at: i64,
    data_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotRollup {
    date: String,
    app_type: String,
    provider_id: String,
    model: String,
    request_model: String,
    pricing_model: String,
    request_count: i64,
    success_count: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    input_token_semantics: i64,
    total_cost_usd: String,
    avg_latency_ms: i64,
}

struct PeerDownload {
    device_id: String,
    device_name: String,
    snapshot_sha256: String,
    etag: Option<String>,
    snapshot: UsageSnapshot,
    detail_cutoff_date: Option<String>,
}

fn ensure_device(db: &Database) -> Result<UsageSyncDevice, AppError> {
    let conn = lock_conn!(db.conn);
    if let Ok(device) = conn.query_row(
        "SELECT id, name FROM usage_sync_device ORDER BY created_at ASC LIMIT 1",
        [],
        |row| {
            Ok(UsageSyncDevice {
                id: row.get(0)?,
                name: row.get(1)?,
            })
        },
    ) {
        return Ok(device);
    }
    let device = UsageSyncDevice {
        id: Uuid::new_v4().to_string(),
        name: crate::services::sync_protocol::detect_system_device_name()
            .unwrap_or_else(|| "Unknown Device".to_string()),
    };
    conn.execute(
        "INSERT INTO usage_sync_device (id, name, created_at) VALUES (?1, ?2, ?3)",
        params![device.id, device.name, chrono::Utc::now().timestamp()],
    )
    .map_err(|e| AppError::Database(format!("创建设备用量标识失败: {e}")))?;
    Ok(device)
}

fn usage_sync_settings() -> UsageSyncSettings {
    settings::get_usage_sync_settings()
}

fn s3_settings() -> Result<S3SyncSettings, AppError> {
    settings::get_s3_sync_settings().ok_or_else(|| {
        AppError::InvalidInput("请先在设置 → 数据 → 云同步中配置 S3/OSS".to_string())
    })
}

fn prefix_for(settings: &S3SyncSettings) -> String {
    let root = settings.remote_root.trim_matches('/');
    if root.is_empty() {
        PREFIX.to_string()
    } else {
        format!("{root}/{PREFIX}")
    }
}

fn manifest_key(prefix: &str, device_id: &str) -> String {
    format!("{prefix}/{device_id}/manifest.json")
}

fn snapshot_key(prefix: &str, device_id: &str) -> String {
    format!("{prefix}/{device_id}/snapshot.json.gz")
}

fn local_date(ts: i64) -> String {
    chrono::Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

fn gzip(bytes: &[u8]) -> Result<Vec<u8>, AppError> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(bytes)
        .map_err(|e| AppError::Message(format!("压缩用量快照失败: {e}")))?;
    encoder
        .finish()
        .map_err(|e| AppError::Message(format!("压缩用量快照失败: {e}")))
}

fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, AppError> {
    let mut decoder = GzDecoder::new(bytes);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|e| AppError::Message(format!("解压用量快照失败: {e}")))?;
    Ok(out)
}

fn build_snapshot(
    db: &Database,
    device: &UsageSyncDevice,
) -> Result<(UsageManifest, Vec<u8>), AppError> {
    let conn = lock_conn!(db.conn);
    let mut details = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT request_id, provider_id, app_type, model, request_model, pricing_model,
                        input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                        input_token_semantics, input_cost_usd, output_cost_usd,
                        cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
                        latency_ms, first_token_ms, duration_ms, status_code, error_message,
                        session_id, provider_type, is_streaming, cost_multiplier, created_at,
                        data_source
                 FROM proxy_request_logs
                 WHERE source_device_id = ''
                 ORDER BY created_at",
            )
            .map_err(|e| AppError::Database(format!("读取本机用量失败: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(SnapshotDetail {
                    request_id: row.get(0)?,
                    provider_id: row.get(1)?,
                    app_type: row.get(2)?,
                    model: row.get(3)?,
                    request_model: row.get(4)?,
                    pricing_model: row.get(5)?,
                    input_tokens: row.get(6)?,
                    output_tokens: row.get(7)?,
                    cache_read_tokens: row.get(8)?,
                    cache_creation_tokens: row.get(9)?,
                    input_token_semantics: row.get(10)?,
                    input_cost_usd: row.get(11)?,
                    output_cost_usd: row.get(12)?,
                    cache_read_cost_usd: row.get(13)?,
                    cache_creation_cost_usd: row.get(14)?,
                    total_cost_usd: row.get(15)?,
                    latency_ms: row.get(16)?,
                    first_token_ms: row.get(17)?,
                    duration_ms: row.get(18)?,
                    status_code: row.get(19)?,
                    error_message: row.get(20)?,
                    session_id: row.get(21)?,
                    provider_type: row.get(22)?,
                    is_streaming: row.get(23)?,
                    cost_multiplier: row.get(24)?,
                    created_at: row.get(25)?,
                    data_source: row.get(26)?,
                })
            })
            .map_err(|e| AppError::Database(format!("读取本机用量失败: {e}")))?;
        for row in rows {
            details.push(row.map_err(|e| AppError::Database(format!("读取本机用量失败: {e}")))?);
        }
    }

    let detail_cutoff = details.iter().map(|row| row.created_at).min();
    let detail_cutoff_date = detail_cutoff.map(local_date);
    let mut rollups = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT date, app_type, provider_id, model, request_model, pricing_model,
                        request_count, success_count, input_tokens, output_tokens,
                        cache_read_tokens, cache_creation_tokens, input_token_semantics,
                        total_cost_usd, avg_latency_ms
                 FROM usage_daily_rollups
                 WHERE source_device_id = ''
                 ORDER BY date",
            )
            .map_err(|e| AppError::Database(format!("读取本机用量汇总失败: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(SnapshotRollup {
                    date: row.get(0)?,
                    app_type: row.get(1)?,
                    provider_id: row.get(2)?,
                    model: row.get(3)?,
                    request_model: row.get(4)?,
                    pricing_model: row.get(5)?,
                    request_count: row.get(6)?,
                    success_count: row.get(7)?,
                    input_tokens: row.get(8)?,
                    output_tokens: row.get(9)?,
                    cache_read_tokens: row.get(10)?,
                    cache_creation_tokens: row.get(11)?,
                    input_token_semantics: row.get(12)?,
                    total_cost_usd: row.get(13)?,
                    avg_latency_ms: row.get(14)?,
                })
            })
            .map_err(|e| AppError::Database(format!("读取本机用量汇总失败: {e}")))?;
        for row in rows {
            let row = row.map_err(|e| AppError::Database(format!("读取本机用量汇总失败: {e}")))?;
            if detail_cutoff_date
                .as_deref()
                .is_some_and(|cutoff| row.date.as_str() >= cutoff)
            {
                continue;
            }
            rollups.push(row);
        }
    }

    let generated_at = chrono::Utc::now().timestamp();
    let snapshot = UsageSnapshot {
        format: FORMAT.to_string(),
        version: VERSION,
        device_id: device.id.clone(),
        device_name: device.name.clone(),
        generated_at,
        details,
        rollups,
    };
    let raw = serde_json::to_vec(&snapshot).map_err(|e| AppError::JsonSerialize { source: e })?;
    if raw.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err(AppError::InvalidInput(format!(
            "用量快照超过 {} MiB，请先清理历史用量",
            MAX_SNAPSHOT_BYTES / 1024 / 1024
        )));
    }
    let compressed = gzip(&raw)?;
    let manifest = UsageManifest {
        format: FORMAT.to_string(),
        version: VERSION,
        device_id: device.id.clone(),
        device_name: device.name.clone(),
        generated_at,
        detail_cutoff,
        detail_cutoff_date,
        detail_count: snapshot.details.len(),
        rollup_count: snapshot.rollups.len(),
        snapshot_sha256: sha256_hex(&compressed),
        snapshot_size: compressed.len() as u64,
    };
    Ok((manifest, compressed))
}

fn save_self_upload(
    conn: &rusqlite::Connection,
    device_id: &str,
    hash: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE usage_sync_device
         SET last_manifest_hash = ?2, last_upload_at = ?3
         WHERE id = ?1",
        params![device_id, hash, chrono::Utc::now().timestamp()],
    )?;
    Ok(())
}

async fn upload_snapshot(
    creds: &s3::S3Credentials,
    settings: &S3SyncSettings,
    db: &Database,
    device: &UsageSyncDevice,
) -> Result<(), AppError> {
    let prefix = prefix_for(settings);
    let (manifest, compressed) = build_snapshot(db, device)?;
    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|e| AppError::JsonSerialize { source: e })?;
    // generated_at 每次都会变，不能拿 manifest 的哈希判断数据有没有变；
    // 只比较压缩快照的内容哈希。
    let snapshot_hash = manifest.snapshot_sha256.clone();
    let previous: Option<String> = {
        let conn = lock_conn!(db.conn);
        conn.query_row(
            "SELECT last_manifest_hash FROM usage_sync_device WHERE id = ?1",
            [&device.id],
            |row| row.get(0),
        )
        .optional()?
        .flatten()
    };
    if previous.as_deref() != Some(snapshot_hash.as_str()) {
        s3::put_private_object(
            creds,
            &snapshot_key(&prefix, &device.id),
            compressed,
            "application/gzip",
        )
        .await?;
    }
    // 清单很小，仍然更新：远端对象被删掉时可以自愈。
    s3::put_private_object(
        creds,
        &manifest_key(&prefix, &device.id),
        manifest_bytes,
        "application/json",
    )
    .await?;
    let conn = lock_conn!(db.conn);
    save_self_upload(&conn, &device.id, &snapshot_hash)?;
    Ok(())
}

async fn list_peer_manifests(
    creds: &s3::S3Credentials,
    settings: &S3SyncSettings,
    self_id: &str,
) -> Result<Vec<(String, String)>, AppError> {
    let prefix = format!("{}/", prefix_for(settings));
    let mut token: Option<String> = None;
    let mut keys = Vec::new();
    loop {
        let (objects, next) = s3::list_objects(creds, &prefix, token.as_deref(), 1000).await?;
        for object in objects {
            let Some(rest) = object.key.strip_prefix(&prefix) else {
                continue;
            };
            let mut parts = rest.split('/');
            let Some(device_id) = parts.next() else {
                continue;
            };
            if parts.next() != Some("manifest.json")
                || parts.next().is_some()
                || device_id == self_id
            {
                continue;
            }
            keys.push((device_id.to_string(), object.key));
        }
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    Ok(keys)
}

async fn download_peer(
    db: &Database,
    creds: &s3::S3Credentials,
    settings: &S3SyncSettings,
    self_id: &str,
    device_id: &str,
    manifest_key: &str,
) -> Result<Option<PeerDownload>, AppError> {
    let Some((manifest_bytes, etag)) =
        s3::get_object(creds, manifest_key, MAX_MANIFEST_BYTES).await?
    else {
        return Ok(None);
    };
    let manifest: UsageManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| AppError::Message(format!("解析用量清单失败: {e}")))?;
    if manifest.format != FORMAT
        || manifest.version != VERSION
        || manifest.device_id != device_id
        || manifest.device_id == self_id
    {
        return Ok(None);
    }
    {
        let conn = lock_conn!(db.conn);
        let previous: Option<String> = conn
            .query_row(
                "SELECT manifest_hash FROM usage_sync_peer_state WHERE device_id = ?1",
                [device_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        if previous.as_deref() == Some(manifest.snapshot_sha256.as_str()) {
            return Ok(None);
        }
    }
    if manifest.snapshot_size > MAX_SNAPSHOT_BYTES {
        return Err(AppError::InvalidInput(
            "对端用量快照超过 512 MiB".to_string(),
        ));
    }
    let prefix = prefix_for(settings);
    let Some((bytes, _)) = s3::get_object(
        creds,
        &snapshot_key(&prefix, device_id),
        manifest.snapshot_size as usize,
    )
    .await?
    else {
        return Err(AppError::Message("对端用量快照不存在".to_string()));
    };
    if bytes.len() as u64 != manifest.snapshot_size
        || sha256_hex(&bytes) != manifest.snapshot_sha256
    {
        return Err(AppError::Message("对端用量快照校验失败".to_string()));
    }
    let raw = gunzip(&bytes)?;
    if raw.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err(AppError::InvalidInput(
            "对端用量数据超过 512 MiB".to_string(),
        ));
    }
    let snapshot: UsageSnapshot = serde_json::from_slice(&raw)
        .map_err(|e| AppError::Message(format!("解析对端用量失败: {e}")))?;
    if snapshot.device_id != device_id || snapshot.version != VERSION {
        return Err(AppError::Message("对端用量设备标识不一致".to_string()));
    }
    Ok(Some(PeerDownload {
        device_id: device_id.to_string(),
        device_name: manifest.device_name,
        snapshot_sha256: manifest.snapshot_sha256.clone(),
        etag,
        snapshot,
        detail_cutoff_date: manifest.detail_cutoff_date,
    }))
}

fn peer_receipt_id(device_id: &str, request_id: &str) -> String {
    format!("peer:{device_id}:{request_id}")
}

fn import_peer(db: &Database, peer: &PeerDownload) -> Result<(i64, i64), AppError> {
    let conn = lock_conn!(db.conn);
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| AppError::Database(format!("启动多终端用量导入事务失败: {e}")))?;
    let cutoff = peer.detail_cutoff_date.as_deref();

    if let Some(cutoff) = cutoff {
        tx.execute(
            "DELETE FROM proxy_request_logs
             WHERE source_device_id = ?1
               AND date(created_at, 'unixepoch', 'localtime') < ?2",
            params![peer.device_id, cutoff],
        )
        .map_err(|e| AppError::Database(format!("清理对端旧明细失败: {e}")))?;
    } else {
        tx.execute(
            "DELETE FROM proxy_request_logs WHERE source_device_id = ?1",
            [&peer.device_id],
        )
        .map_err(|e| AppError::Database(format!("清理对端明细失败: {e}")))?;
        tx.execute(
            "DELETE FROM usage_daily_rollups WHERE source_device_id = ?1",
            [&peer.device_id],
        )
        .map_err(|e| AppError::Database(format!("清理对端汇总失败: {e}")))?;
    }

    let mut imported_rollups = 0i64;
    for rollup in &peer.snapshot.rollups {
        if cutoff.is_some_and(|cutoff| rollup.date.as_str() >= cutoff) {
            continue;
        }
        tx.execute(
            "INSERT OR REPLACE INTO usage_daily_rollups (
                date, app_type, provider_id, model, request_model, pricing_model,
                request_count, success_count, input_tokens, output_tokens,
                cache_read_tokens, cache_creation_tokens, input_token_semantics,
                total_cost_usd, avg_latency_ms, source_device_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                rollup.date,
                rollup.app_type,
                rollup.provider_id,
                rollup.model,
                rollup.request_model,
                rollup.pricing_model,
                rollup.request_count,
                rollup.success_count,
                rollup.input_tokens,
                rollup.output_tokens,
                rollup.cache_read_tokens,
                rollup.cache_creation_tokens,
                rollup.input_token_semantics,
                rollup.total_cost_usd,
                rollup.avg_latency_ms,
                peer.device_id,
            ],
        )
        .map_err(|e| AppError::Database(format!("写入对端汇总失败: {e}")))?;
        imported_rollups += 1;
    }

    let mut imported_details = 0i64;
    for detail in &peer.snapshot.details {
        let day = local_date(detail.created_at);
        if cutoff.is_some_and(|cutoff| day.as_str() < cutoff) {
            continue;
        }
        let covered: bool = tx
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM usage_daily_rollups
                    WHERE source_device_id = ?1 AND date = ?2
                 )",
                params![peer.device_id, day],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Database(format!("查询对端汇总覆盖失败: {e}")))?;
        if covered {
            continue;
        }
        let peer_id = peer_receipt_id(&peer.device_id, &detail.request_id);
        let seen: bool = tx
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM session_usage_dedup
                    WHERE data_source = ?1 AND request_id = ?2
                 )",
                params![PEER_DATA_SOURCE, peer_id],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Database(format!("查询对端用量回执失败: {e}")))?;
        if seen {
            continue;
        }
        let changed = tx
            .execute(
                "INSERT OR IGNORE INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model, pricing_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    input_token_semantics, input_cost_usd, output_cost_usd,
                    cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
                    latency_ms, first_token_ms, duration_ms, status_code, error_message,
                    session_id, provider_type, is_streaming, cost_multiplier, created_at,
                    data_source, source_device_id, source_device_name
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                           ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29)",
                params![
                    peer_id,
                    detail.provider_id,
                    detail.app_type,
                    detail.model,
                    detail.request_model,
                    detail.pricing_model,
                    detail.input_tokens,
                    detail.output_tokens,
                    detail.cache_read_tokens,
                    detail.cache_creation_tokens,
                    detail.input_token_semantics,
                    detail.input_cost_usd,
                    detail.output_cost_usd,
                    detail.cache_read_cost_usd,
                    detail.cache_creation_cost_usd,
                    detail.total_cost_usd,
                    detail.latency_ms,
                    detail.first_token_ms,
                    detail.duration_ms,
                    detail.status_code,
                    detail.error_message,
                    detail.session_id,
                    detail.provider_type,
                    detail.is_streaming,
                    detail.cost_multiplier,
                    detail.created_at,
                    detail.data_source,
                    peer.device_id,
                    peer.device_name,
                ],
            )
            .map_err(|e| AppError::Database(format!("写入对端明细失败: {e}")))?;
        tx.execute(
            "INSERT OR IGNORE INTO session_usage_dedup
                (data_source, request_id, semantic_id, has_entry_id)
             VALUES (?1, ?2, ?2, 1)",
            params![PEER_DATA_SOURCE, peer_id],
        )
        .map_err(|e| AppError::Database(format!("写入对端用量回执失败: {e}")))?;
        imported_details += changed as i64;
    }

    tx.execute(
        "INSERT OR REPLACE INTO usage_sync_peer_state
            (device_id, name, manifest_hash, etag, last_sync_at, imported_details, imported_rollups, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)",
        params![
            peer.device_id,
            peer.device_name,
            peer.snapshot_sha256,
            peer.etag,
            chrono::Utc::now().timestamp(),
            imported_details,
            imported_rollups,
        ],
    )
    .map_err(|e| AppError::Database(format!("更新对端同步状态失败: {e}")))?;
    tx.commit()
        .map_err(|e| AppError::Database(format!("提交多终端用量导入失败: {e}")))?;
    crate::usage_events::notify_log_recorded();
    Ok((imported_details, imported_rollups))
}

fn record_peer_error(db: &Database, device_id: &str, name: &str, error: &str) {
    let Ok(conn) = crate::database::lock_recovering(&db.conn) else {
        return;
    };
    let _ = conn.execute(
        "INSERT INTO usage_sync_peer_state
            (device_id, name, manifest_hash, etag, last_sync_at, imported_details, imported_rollups, error)
         VALUES (?1, ?2, NULL, NULL, ?3, 0, 0, ?4)
         ON CONFLICT(device_id) DO UPDATE SET
            name = excluded.name,
            last_sync_at = excluded.last_sync_at,
            error = excluded.error",
        params![device_id, name, chrono::Utc::now().timestamp(), error],
    );
}

pub fn status(db: &Database) -> Result<UsageSyncStatus, AppError> {
    let device = ensure_device(db)?;
    let settings = usage_sync_settings();
    let conn = lock_conn!(db.conn);
    let mut stmt = conn
        .prepare(
            "SELECT device_id, name, last_sync_at, imported_details, imported_rollups, error
             FROM usage_sync_peer_state ORDER BY name, device_id",
        )
        .map_err(|e| AppError::Database(format!("读取对端同步状态失败: {e}")))?;
    let peers = stmt
        .query_map([], |row| {
            Ok(UsageSyncPeerStatus {
                device_id: row.get(0)?,
                name: row.get(1)?,
                last_sync_at: row.get(2)?,
                imported_details: row.get(3)?,
                imported_rollups: row.get(4)?,
                error: row.get(5)?,
            })
        })
        .map_err(|e| AppError::Database(format!("读取对端同步状态失败: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Database(format!("读取对端同步状态失败: {e}")))?;
    Ok(UsageSyncStatus {
        enabled: settings.enabled,
        auto_sync: settings.auto_sync,
        device_id: device.id,
        device_name: device.name,
        peers,
    })
}

fn sync_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

pub async fn sync_now(db: &Database) -> Result<UsageSyncStatus, AppError> {
    let _guard = sync_lock().lock().await;
    let settings = usage_sync_settings();
    if !settings.enabled {
        return Err(AppError::InvalidInput("多终端用量汇总未启用".to_string()));
    }
    let s3_settings = s3_settings()?;
    let creds = crate::services::oss_backup::credentials(&s3_settings)?;
    let device = ensure_device(db)?;

    upload_snapshot(&creds, &s3_settings, db, &device).await?;

    let mut seen = HashSet::new();
    for (device_id, key) in list_peer_manifests(&creds, &s3_settings, &device.id).await? {
        if !seen.insert(device_id.clone()) {
            continue;
        }
        match download_peer(db, &creds, &s3_settings, &device.id, &device_id, &key).await {
            Ok(Some(peer)) => {
                if let Err(error) = import_peer(db, &peer) {
                    record_peer_error(db, &peer.device_id, &peer.device_name, &error.to_string());
                }
            }
            Ok(None) => {}
            Err(error) => record_peer_error(db, &device_id, &device_id, &error.to_string()),
        }
    }
    status(db)
}

pub fn forget_peer(db: &Database, device_id: &str) -> Result<(), AppError> {
    let mut conn = lock_conn!(db.conn);
    let tx = conn
        .transaction()
        .map_err(|e| AppError::Database(format!("启动移除设备事务失败: {e}")))?;
    tx.execute(
        "DELETE FROM proxy_request_logs WHERE source_device_id = ?1",
        [device_id],
    )?;
    tx.execute(
        "DELETE FROM usage_daily_rollups WHERE source_device_id = ?1",
        [device_id],
    )?;
    tx.execute(
        "DELETE FROM session_usage_dedup
         WHERE data_source = ?1 AND request_id LIKE ?2",
        params![PEER_DATA_SOURCE, format!("peer:{device_id}:%")],
    )?;
    tx.execute(
        "DELETE FROM usage_sync_peer_state WHERE device_id = ?1",
        [device_id],
    )?;
    tx.commit()
        .map_err(|e| AppError::Database(format!("移除设备失败: {e}")))?;
    crate::usage_events::notify_log_recorded();
    Ok(())
}

pub fn start_worker(db: Arc<Database>) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(STARTUP_DELAY_SECS)).await;
        loop {
            let settings = usage_sync_settings();
            if settings.enabled && settings.auto_sync && s3_settings().is_ok() {
                if let Err(error) = sync_now(&db).await {
                    log::warn!("多终端用量同步失败: {error}");
                }
            }
            tokio::time::sleep(Duration::from_secs(AUTO_SYNC_INTERVAL_SECS)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert_local(
        db: &Database,
        request_id: &str,
        model: &str,
        created_at: i64,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(db.conn);
        conn.execute(
            "INSERT INTO proxy_request_logs
                (request_id, provider_id, app_type, model, input_tokens, output_tokens,
                 latency_ms, status_code, created_at, data_source)
             VALUES (?1, 'p', 'kimi', ?2, 10, 20, 1000, 200, ?3, 'kimicode_session')",
            params![request_id, model, created_at],
        )?;
        Ok(())
    }

    #[test]
    fn snapshot_exports_only_local_rows() -> Result<(), AppError> {
        let db = Database::memory()?;
        insert_local(&db, "local-1", "deepseek-flash", 1_700_000_000)?;
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs
                    (request_id, provider_id, app_type, model, input_tokens, output_tokens,
                     latency_ms, status_code, created_at, data_source, source_device_id,
                     source_device_name)
                 VALUES ('peer:other:r1', 'p', 'kimi', 'deepseek-flash', 5, 6, 500, 200,
                         ?1, 'kimicode_session', 'other', 'Other Mac')",
                [1_700_000_000i64],
            )?;
        }
        let device = UsageSyncDevice {
            id: "self".to_string(),
            name: "Self".to_string(),
        };
        let (manifest, _) = build_snapshot(&db, &device)?;
        assert_eq!(manifest.detail_count, 1);
        assert_eq!(manifest.rollup_count, 0);
        Ok(())
    }

    #[test]
    fn peer_import_is_idempotent_and_does_not_upload_peer_rows() -> Result<(), AppError> {
        let db = Database::memory().unwrap();
        insert_local(&db, "local-1", "deepseek-flash", 1_700_000_000)?;
        let snapshot = UsageSnapshot {
            format: FORMAT.to_string(),
            version: VERSION,
            device_id: "peer-a".to_string(),
            device_name: "Peer A".to_string(),
            generated_at: 0,
            details: vec![SnapshotDetail {
                request_id: "r1".to_string(),
                provider_id: "p".to_string(),
                app_type: "kimi".to_string(),
                model: "deepseek-flash".to_string(),
                request_model: None,
                pricing_model: None,
                input_tokens: 10,
                output_tokens: 20,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                input_token_semantics: 1,
                input_cost_usd: "0".to_string(),
                output_cost_usd: "0".to_string(),
                cache_read_cost_usd: "0".to_string(),
                cache_creation_cost_usd: "0".to_string(),
                total_cost_usd: "0".to_string(),
                latency_ms: 1000,
                first_token_ms: None,
                duration_ms: None,
                status_code: 200,
                error_message: None,
                session_id: None,
                provider_type: None,
                is_streaming: true,
                cost_multiplier: "1.0".to_string(),
                created_at: 1_700_000_000,
                data_source: "kimicode_session".to_string(),
            }],
            rollups: vec![],
        };
        let peer = PeerDownload {
            device_id: snapshot.device_id.clone(),
            device_name: snapshot.device_name.clone(),
            snapshot_sha256: "hash".to_string(),
            etag: None,
            snapshot,
            detail_cutoff_date: Some(local_date(1_700_000_000)),
        };
        assert_eq!(import_peer(&db, &peer).unwrap(), (1, 0));
        assert_eq!(import_peer(&db, &peer).unwrap(), (0, 0));

        let conn = lock_conn!(db.conn);
        let local: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM proxy_request_logs WHERE source_device_id = ''",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(local, 1);
        let peer_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM proxy_request_logs WHERE source_device_id = 'peer-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(peer_rows, 1);
        Ok(())
    }
}
#[cfg(test)]
mod aggregation_tests {
    use super::*;

    fn detail(request_id: &str, created_at: i64) -> SnapshotDetail {
        SnapshotDetail {
            request_id: request_id.to_string(),
            provider_id: "p".to_string(),
            app_type: "kimi".to_string(),
            model: "deepseek-flash".to_string(),
            request_model: None,
            pricing_model: None,
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            input_token_semantics: 1,
            input_cost_usd: "0.00001".to_string(),
            output_cost_usd: "0.00004".to_string(),
            cache_read_cost_usd: "0".to_string(),
            cache_creation_cost_usd: "0".to_string(),
            total_cost_usd: "0.00005".to_string(),
            latency_ms: 1000,
            first_token_ms: None,
            duration_ms: None,
            status_code: 200,
            error_message: None,
            session_id: None,
            provider_type: None,
            is_streaming: true,
            cost_multiplier: "1.0".to_string(),
            created_at,
            data_source: "kimicode_session".to_string(),
        }
    }

    fn rollup(date: &str) -> SnapshotRollup {
        SnapshotRollup {
            date: date.to_string(),
            app_type: "kimi".to_string(),
            provider_id: "p".to_string(),
            model: "deepseek-flash".to_string(),
            request_model: String::new(),
            pricing_model: String::new(),
            request_count: 1,
            success_count: 1,
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            input_token_semantics: 1,
            total_cost_usd: "0.00005".to_string(),
            avg_latency_ms: 1000,
        }
    }

    #[test]
    fn peer_rows_are_aggregated_but_rollups_only_cover_local_rows() -> Result<(), AppError> {
        let db = Database::memory()?;
        let old = 1_699_000_000i64; // far beyond the 30-day retention window
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs
                    (request_id, provider_id, app_type, model, input_tokens, output_tokens,
                     latency_ms, status_code, created_at, data_source)
                 VALUES ('local-old', 'p', 'kimi', 'deepseek-flash', 10, 20, 900, 200, ?1, 'kimicode_session')",
                [old],
            )?;
        }
        let peer = PeerDownload {
            device_id: "peer-b".to_string(),
            device_name: "Peer B".to_string(),
            snapshot_sha256: "hash-details".to_string(),
            etag: None,
            snapshot: UsageSnapshot {
                format: FORMAT.to_string(),
                version: VERSION,
                device_id: "peer-b".to_string(),
                device_name: "Peer B".to_string(),
                generated_at: 0,
                details: vec![detail("peer-old", old + 10)],
                rollups: vec![],
            },
            detail_cutoff_date: Some(local_date(old + 10)),
        };
        assert_eq!(import_peer(&db, &peer)?, (1, 0));

        let summary = db.get_usage_summary(None, None, None, None, None)?;
        assert_eq!(summary.total_requests, 2);

        db.rollup_and_prune(30)?;
        {
            let conn = lock_conn!(db.conn);
            let local_rollup: i64 = conn.query_row(
                "SELECT COALESCE(SUM(request_count), 0) FROM usage_daily_rollups
                 WHERE source_device_id = ''",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(local_rollup, 1);
            let peer_rollup: i64 = conn.query_row(
                "SELECT COUNT(*) FROM usage_daily_rollups WHERE source_device_id <> ''",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(peer_rollup, 0);
            // The peer detail must survive: its rollup has not arrived yet.
            let peer_details: i64 = conn.query_row(
                "SELECT COUNT(*) FROM proxy_request_logs WHERE source_device_id = 'peer-b'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(peer_details, 1);
        }
        assert_eq!(
            db.get_usage_summary(None, None, None, None, None)?
                .total_requests,
            2
        );

        // Now the peer archives the same day: the detail is replaced by its rollup.
        let archived = PeerDownload {
            device_id: "peer-b".to_string(),
            device_name: "Peer B".to_string(),
            snapshot_sha256: "hash-rollups".to_string(),
            etag: None,
            snapshot: UsageSnapshot {
                format: FORMAT.to_string(),
                version: VERSION,
                device_id: "peer-b".to_string(),
                device_name: "Peer B".to_string(),
                generated_at: 0,
                details: vec![],
                rollups: vec![rollup(&local_date(old + 10))],
            },
            detail_cutoff_date: None,
        };
        assert_eq!(import_peer(&db, &archived)?, (0, 1));
        {
            let conn = lock_conn!(db.conn);
            let peer_details: i64 = conn.query_row(
                "SELECT COUNT(*) FROM proxy_request_logs WHERE source_device_id = 'peer-b'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(peer_details, 0);
        }
        assert_eq!(
            db.get_usage_summary(None, None, None, None, None)?
                .total_requests,
            2
        );

        // Re-sending the old detail after archival must not double count.
        assert_eq!(import_peer(&db, &peer)?, (0, 0));
        assert_eq!(
            db.get_usage_summary(None, None, None, None, None)?
                .total_requests,
            2
        );
        Ok(())
    }
}
