//! Kimi Code direct-mode session usage importer.
//!
//! Kimi Code writes one `usage.record` per model request into
//! `<data_dir>/sessions/<workspace>/<session>/agents/<agent>/wire.jsonl`.
//! Direct-mode requests (official `kimi-code/*` or cc-switch's
//! `cc-switch:<provider>/*`) never reach the local proxy, so without this
//! importer their tokens are missing from the dashboard. Proxy-mode requests
//! (`cc-switch:proxy/*`) are already logged per request and must be skipped.
//!
//! Event semantics (verified against local Kimi 2.x wire logs):
//! - `usageScope: "turn"` is one model request. `"session"` is a cumulative
//!   snapshot of the whole session and would double count if imported.
//! - `usage.inputOther` is fresh input (cache reads/writes are separate), which
//!   matches Anthropic's `input_tokens` semantics.

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::proxy::usage::calculator::CostCalculator;
use crate::proxy::usage::parser::TokenUsage;
use crate::services::session_usage::{
    estimated_latency_ms, load_sync_cursors, metadata_modified_nanos, update_sync_state_on_conn,
    SessionSyncResult,
};
use crate::services::sql_helpers::INPUT_TOKEN_SEMANTICS_FRESH;
use crate::services::usage_stats::{find_model_pricing, should_skip_session_insert, DedupKey};
use rust_decimal::Decimal;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

const APP_TYPE: &str = "kimicode";
const DATA_SOURCE: &str = "kimicode_session";
const PROVIDER_PLACEHOLDER: &str = "_kimicode_session";
const UNKNOWN_MODEL: &str = "unknown";
const MAX_WIRE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_MODEL_LABEL_BYTES: usize = 512;
const MIN_SQLITE_UNIX_SECONDS: i64 = -62_167_219_200;
const MAX_SQLITE_UNIX_SECONDS: i64 = 253_402_300_799;

#[derive(Debug)]
struct WireFile {
    session_id: String,
    agent: String,
    path: PathBuf,
}

#[derive(Debug)]
struct KimiUsageRecord {
    request_id: String,
    provider_id: String,
    model: String,
    request_model: String,
    input_tokens: u32,
    output_tokens: u32,
    cache_read_tokens: u32,
    cache_creation_tokens: u32,
    latency_ms: i64,
    created_at: i64,
    session_id: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct UsageCounters {
    input: u32,
    output: u32,
    cache_read: u32,
    cache_creation: u32,
}

impl UsageCounters {
    fn is_zero(self) -> bool {
        self.input == 0 && self.output == 0 && self.cache_read == 0 && self.cache_creation == 0
    }
}

/// Import usage from every Kimi Code session discoverable by the session
/// browser's current root and layout rules.
pub fn sync_kimicode_usage(db: &Database) -> Result<SessionSyncResult, AppError> {
    let roots = crate::session_manager::providers::kimicode::session_roots();
    Ok(sync_kimicode_roots(db, &roots))
}

fn sync_kimicode_roots(db: &Database, roots: &[PathBuf]) -> SessionSyncResult {
    if let Err(error) = backfill_provider_ids(db) {
        return SessionSyncResult {
            errors: vec![format!("回填 Kimi Code 供应商标识失败: {error}")],
            ..Default::default()
        };
    }

    let mut files = Vec::new();
    for root in roots {
        collect_wire_files(root, &mut files);
    }

    let mut result = SessionSyncResult {
        files_scanned: files.len().min(u32::MAX as usize) as u32,
        ..Default::default()
    };

    let cursors = match load_sync_cursors(db) {
        Ok(cursors) => cursors,
        Err(error) => {
            result
                .errors
                .push(format!("预取 Kimi Code 同步游标失败: {error}"));
            return result;
        }
    };

    for file in &files {
        match sync_single_wire_file(db, file, &cursors) {
            Ok(file_result) => result.merge(file_result),
            Err(error) => {
                let message = format!("{}: {error}", file.path.display());
                log::warn!("[KIMI-SYNC] 会话文件解析失败: {message}");
                result.errors.push(message);
            }
        }
    }

    if result.imported > 0 {
        log::info!(
            "[KIMI-SYNC] 同步完成: 导入 {} 条, 跳过 {} 条, 扫描 {} 个文件",
            result.imported,
            result.skipped,
            result.files_scanned
        );
    }
    result
}

/// Rows imported before provider attribution stored the placeholder
/// `_kimicode_session`. Derive the real key from the preserved alias so the
/// dashboard resolves the provider name on the next sync.
fn backfill_provider_ids(db: &Database) -> Result<(), AppError> {
    let conn = lock_conn!(db.conn);
    conn.execute(
        "UPDATE proxy_request_logs
         SET provider_id = substr(request_model, 11, instr(substr(request_model, 11), '/') - 1)
         WHERE data_source = 'kimicode_session'
           AND provider_id = '_kimicode_session'
           AND request_model LIKE 'cc-switch:%/%'
           AND instr(substr(request_model, 11), '/') > 1
           AND substr(request_model, 11, instr(substr(request_model, 11), '/') - 1) <> 'proxy'",
        [],
    )
    .map_err(|error| AppError::Database(format!("回填 Kimi Code 供应商标识失败: {error}")))?;
    Ok(())
}

/// Discover `<root>/<workspace>/<session>/agents/<agent>/wire.jsonl` files.
/// Symlinks are never followed. When `state.json` carries an `id` it must match
/// the directory name; legacy v1 sessions without an `id` are still accepted so
/// their historical usage is not lost.
fn collect_wire_files(root: &Path, files: &mut Vec<WireFile>) {
    let Ok(workspaces) = fs::read_dir(root) else {
        return;
    };
    for workspace in workspaces.flatten() {
        if !is_real_dir(&workspace) {
            continue;
        }
        let Ok(sessions) = fs::read_dir(workspace.path()) else {
            continue;
        };
        for session in sessions.flatten() {
            if !is_real_dir(&session) {
                continue;
            }
            let session_dir = session.path();
            let Some(session_id) = session_dir
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            if !state_matches(&session_dir, &session_id) {
                continue;
            }
            let Ok(agents) = fs::read_dir(session_dir.join("agents")) else {
                continue;
            };
            for agent in agents.flatten() {
                if !is_real_dir(&agent) {
                    continue;
                }
                let agent_name = agent.file_name().to_string_lossy().into_owned();
                let wire = agent.path().join("wire.jsonl");
                let Ok(metadata) = fs::symlink_metadata(&wire) else {
                    continue;
                };
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    continue;
                }
                files.push(WireFile {
                    session_id: session_id.clone(),
                    agent: agent_name,
                    path: wire,
                });
            }
        }
    }
}

fn is_real_dir(entry: &fs::DirEntry) -> bool {
    entry
        .metadata()
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

fn state_matches(session_dir: &Path, session_id: &str) -> bool {
    let Ok(bytes) = fs::read(session_dir.join("state.json")) else {
        // Legacy session directories may predate state.json; the wire file is
        // still the authoritative source of usage.
        return true;
    };
    let Ok(state) = serde_json::from_slice::<Value>(&bytes) else {
        return true;
    };
    match state.get("id").and_then(Value::as_str) {
        Some(id) => id == session_id,
        None => true,
    }
}

fn sync_single_wire_file(
    db: &Database,
    file: &WireFile,
    cursors: &std::collections::HashMap<String, crate::services::session_usage::SyncCursor>,
) -> Result<SessionSyncResult, AppError> {
    let file_path = file.path.to_string_lossy().to_string();
    let metadata = fs::metadata(&file.path)
        .map_err(|error| AppError::Config(format!("无法读取 Kimi Code 会话文件元数据: {error}")))?;
    let modified = metadata_modified_nanos(&metadata);
    let last_modified = cursors
        .get(&file_path)
        .map_or(0, |cursor| cursor.last_modified);
    if modified <= last_modified {
        return Ok(SessionSyncResult::default());
    }
    if metadata.len() > MAX_WIRE_BYTES {
        log::warn!(
            "[KIMI-SYNC] 会话文件超过 {} MiB，跳过: {}",
            MAX_WIRE_BYTES / 1024 / 1024,
            file.path.display()
        );
        let conn = lock_conn!(db.conn);
        update_sync_state_on_conn(&conn, &file_path, modified, metadata.len() as i64)?;
        return Ok(SessionSyncResult::default());
    }

    let content = fs::read_to_string(&file.path)
        .map_err(|error| AppError::Config(format!("无法读取 Kimi Code 会话文件: {error}")))?;
    // A trailing partial line means the CLI is still writing. Do not advance the
    // cursor, so the next sync re-reads the completed line.
    let complete = content.is_empty() || content.ends_with('\n');
    let records = parse_wire_usage(&file.session_id, &file.agent, &content);

    let mut result = SessionSyncResult::default();
    let mut insert_failed = false;
    {
        let conn = lock_conn!(db.conn);
        let tx = conn.unchecked_transaction().map_err(|error| {
            AppError::Database(format!("启动 Kimi Code 用量导入事务失败: {error}"))
        })?;
        for record in &records {
            match insert_record(&tx, record) {
                Ok(true) => result.imported = result.imported.saturating_add(1),
                Ok(false) => result.skipped = result.skipped.saturating_add(1),
                Err(error) => {
                    insert_failed = true;
                    log::warn!("[KIMI-SYNC] 插入失败 ({}): {error}", record.request_id);
                    result.skipped = result.skipped.saturating_add(1);
                }
            }
        }
        if complete && !insert_failed {
            update_sync_state_on_conn(&tx, &file_path, modified, content.lines().count() as i64)?;
        }
        tx.commit().map_err(|error| {
            AppError::Database(format!("提交 Kimi Code 用量导入事务失败: {error}"))
        })?;
    }

    if !complete {
        result.deferred_files = 1;
    }
    Ok(result)
}

fn parse_wire_usage(session_id: &str, agent: &str, content: &str) -> Vec<KimiUsageRecord> {
    let mut records = Vec::new();
    // (start_ms, model_alias, upstream_model) of the most recent llm.request
    let mut pending: Option<(i64, String, String)> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "llm.request" => {
                let Some(time) = value.get("time").and_then(Value::as_i64) else {
                    continue;
                };
                let alias = value
                    .get("modelAlias")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let model = value
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                pending = Some((time, alias, model));
            }
            "usage.record" => {
                if value.get("usageScope").and_then(Value::as_str) == Some("session") {
                    continue;
                }
                let Some(usage) = value.get("usage") else {
                    continue;
                };
                let alias = value
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim();
                if alias.is_empty() || is_proxy_alias(alias) {
                    continue;
                }
                let Some(time_ms) = value.get("time").and_then(Value::as_i64) else {
                    continue;
                };
                let counters = parse_usage(usage);
                if counters.is_zero() {
                    continue;
                }

                let (model, latency_ms) = match pending.take() {
                    Some((start_ms, pending_alias, pending_model))
                        if pending_alias.is_empty() || pending_alias == alias =>
                    {
                        let model = nonempty_label(&pending_model)
                            .or_else(|| model_from_alias(alias))
                            .unwrap_or_else(|| UNKNOWN_MODEL.to_string());
                        (model, estimated_latency_ms(start_ms, time_ms).unwrap_or(0))
                    }
                    _ => (
                        model_from_alias(alias).unwrap_or_else(|| UNKNOWN_MODEL.to_string()),
                        0,
                    ),
                };

                let created_at =
                    (time_ms / 1000).clamp(MIN_SQLITE_UNIX_SECONDS, MAX_SQLITE_UNIX_SECONDS);
                records.push(KimiUsageRecord {
                    request_id: request_id(session_id, agent, alias, time_ms, counters),
                    provider_id: provider_id_for_alias(alias),
                    model,
                    request_model: truncate_label(alias),
                    input_tokens: counters.input,
                    output_tokens: counters.output,
                    cache_read_tokens: counters.cache_read,
                    cache_creation_tokens: counters.cache_creation,
                    latency_ms,
                    created_at,
                    session_id: session_id.to_string(),
                });
            }
            _ => {}
        }
    }

    records
}

fn parse_usage(value: &Value) -> UsageCounters {
    let count = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(u32::MAX as u64) as u32
    };
    UsageCounters {
        input: count("inputOther"),
        output: count("output"),
        cache_read: count("inputCacheRead"),
        cache_creation: count("inputCacheCreation"),
    }
}

fn is_proxy_alias(alias: &str) -> bool {
    let alias = alias.trim();
    alias == "cc-switch:proxy"
        || alias.starts_with("cc-switch:proxy/")
        || alias.starts_with("ccs-kimi-")
}

/// `cc-switch:<provider>/<model>` aliases carry the concrete provider key, so
/// attribute the row to that provider and let the dashboard resolve its name.
/// Native `kimi-code/*` sessions keep the session placeholder because they do
/// not have a cc-switch provider row.
fn provider_id_for_alias(alias: &str) -> String {
    if let Some(rest) = alias.trim().strip_prefix("cc-switch:") {
        if let Some((provider, _model)) = rest.split_once('/') {
            let provider = provider.trim();
            if !provider.is_empty() && provider != "proxy" {
                return truncate_label(provider);
            }
        }
    }
    PROVIDER_PLACEHOLDER.to_string()
}

fn model_from_alias(alias: &str) -> Option<String> {
    let model = alias.rsplit_once('/').map_or(alias, |(_, model)| model);
    nonempty_label(model)
}

fn nonempty_label(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn truncate_label(value: &str) -> String {
    if value.len() <= MAX_MODEL_LABEL_BYTES {
        return value.to_string();
    }
    let mut end = MAX_MODEL_LABEL_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn request_id(
    session_id: &str,
    agent: &str,
    alias: &str,
    time_ms: i64,
    counters: UsageCounters,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"kimicode-session-usage-v2");
    for part in [session_id, agent, alias] {
        hasher.update(part.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(time_ms.to_be_bytes());
    for value in [
        counters.input,
        counters.output,
        counters.cache_read,
        counters.cache_creation,
    ] {
        hasher.update(value.to_be_bytes());
    }
    format!("kimicode_session:{session_id}:{:x}", hasher.finalize())
}

fn ledger_seen(conn: &rusqlite::Connection, request_id: &str) -> Result<bool, AppError> {
    conn.prepare_cached(
        "SELECT EXISTS(
             SELECT 1 FROM session_usage_dedup
             WHERE data_source = ?1 AND request_id = ?2
         )",
    )
    .and_then(|mut stmt| {
        stmt.query_row(rusqlite::params![DATA_SOURCE, request_id], |row| {
            row.get::<_, bool>(0)
        })
    })
    .map_err(|error| AppError::Database(format!("查询 Kimi Code 用量去重账本失败: {error}")))
}

fn remember_in_ledger(conn: &rusqlite::Connection, request_id: &str) -> Result<(), AppError> {
    conn.execute(
        "INSERT OR IGNORE INTO session_usage_dedup
         (data_source, request_id, semantic_id, has_entry_id)
         VALUES (?1, ?2, ?2, 1)",
        rusqlite::params![DATA_SOURCE, request_id],
    )
    .map(|_| ())
    .map_err(|error| AppError::Database(format!("写入 Kimi Code 用量去重账本失败: {error}")))
}

fn insert_record(conn: &rusqlite::Connection, record: &KimiUsageRecord) -> Result<bool, AppError> {
    // The detail table is pruned after 30 days but this ledger is not. Without
    // it, re-reading an appended wire file would re-insert already-rolled-up
    // rows and inflate the daily totals.
    if ledger_seen(conn, &record.request_id)? {
        return Ok(false);
    }

    let usage = TokenUsage {
        input_tokens: record.input_tokens,
        output_tokens: record.output_tokens,
        cache_read_tokens: record.cache_read_tokens,
        cache_creation_tokens: record.cache_creation_tokens,
        cache_creation_1h_tokens: 0,
        model: Some(record.model.clone()),
        message_id: None,
    };
    let dedup_key = DedupKey {
        app_type: APP_TYPE,
        model: &record.model,
        input_tokens: record.input_tokens,
        output_tokens: record.output_tokens,
        cache_read_tokens: record.cache_read_tokens,
        cache_creation_tokens: record.cache_creation_tokens,
        created_at: record.created_at,
    };
    if should_skip_session_insert(conn, &record.request_id, &dedup_key)? {
        remember_in_ledger(conn, &record.request_id)?;
        return Ok(false);
    }

    let costs = find_model_pricing(conn, &record.model)
        .map(|pricing| CostCalculator::calculate_for_app(APP_TYPE, &usage, &pricing, Decimal::ONE));
    let (input_cost, output_cost, cache_read_cost, cache_creation_cost, total_cost) = costs.map_or(
        (
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
        ),
        |cost| {
            (
                cost.input_cost,
                cost.output_cost,
                cost.cache_read_cost,
                cost.cache_creation_cost,
                cost.total_cost,
            )
        },
    );

    let changed = conn
        .execute(
            "INSERT OR IGNORE INTO proxy_request_logs (
            request_id, provider_id, app_type, model, request_model, pricing_model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_token_semantics,
            input_cost_usd, output_cost_usd, cache_read_cost_usd,
            cache_creation_cost_usd, total_cost_usd,
            latency_ms, first_token_ms, status_code, error_message, session_id,
            provider_type, is_streaming, cost_multiplier, created_at, data_source
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
            ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26
        )",
            rusqlite::params![
                record.request_id,
                record.provider_id,
                APP_TYPE,
                record.model,
                record.request_model,
                record.model,
                record.input_tokens,
                record.output_tokens,
                record.cache_read_tokens,
                record.cache_creation_tokens,
                INPUT_TOKEN_SEMANTICS_FRESH,
                input_cost.to_string(),
                output_cost.to_string(),
                cache_read_cost.to_string(),
                cache_creation_cost.to_string(),
                total_cost.to_string(),
                record.latency_ms,
                Option::<i64>::None,
                200i64,
                Option::<String>::None,
                record.session_id,
                Some("session_log"),
                1i64,
                "1.0",
                record.created_at,
                DATA_SOURCE,
            ],
        )
        .map_err(|error| AppError::Database(format!("插入 Kimi Code 会话用量失败: {error}")))?;
    remember_in_ledger(conn, &record.request_id)?;
    Ok(changed > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use serde_json::json;
    use std::io::Write;
    use tempfile::tempdir;

    const SESSION: &str = "session_demo";
    const WORKSPACE: &str = "wd_demo";

    fn write_session(root: &Path, agent: &str, lines: &[String]) -> PathBuf {
        let session_dir = root.join(WORKSPACE).join(SESSION);
        fs::create_dir_all(session_dir.join("agents").join(agent)).unwrap();
        fs::write(
            session_dir.join("state.json"),
            json!({"id": SESSION, "cwd": "/tmp/demo"}).to_string(),
        )
        .unwrap();
        let wire = session_dir.join("agents").join(agent).join("wire.jsonl");
        let mut file = fs::File::create(&wire).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        wire
    }

    fn llm_request(alias: &str, model: &str, time: i64) -> String {
        json!({
            "type": "llm.request",
            "agentId": "main",
            "provider": "anthropic",
            "model": model,
            "modelAlias": alias,
            "time": time
        })
        .to_string()
    }

    fn usage_record(alias: &str, scope: &str, time: i64) -> String {
        json!({
            "type": "usage.record",
            "agentId": "main",
            "model": alias,
            "usage": {
                "inputOther": 100,
                "output": 20,
                "inputCacheRead": 50,
                "inputCacheCreation": 0
            },
            "usageScope": scope,
            "time": time
        })
        .to_string()
    }

    fn seed_pricing(db: &Database, model: &str) -> Result<(), AppError> {
        let conn = lock_conn!(db.conn);
        conn.execute(
            "INSERT OR REPLACE INTO model_pricing (
                model_id, display_name, input_cost_per_million, output_cost_per_million,
                cache_read_cost_per_million, cache_creation_cost_per_million
             ) VALUES (?1, ?1, '1', '2', '0.1', '0')",
            [model],
        )?;
        Ok(())
    }

    #[test]
    fn imports_turn_usage_and_skips_session_and_proxy_records() -> Result<(), AppError> {
        let temp = tempdir().unwrap();
        let root = temp.path().join("sessions");
        write_session(
            &root,
            "main",
            &[
                llm_request(
                    "cc-switch:deepseek/deepseek-flash",
                    "deepseek-flash",
                    1_000_000,
                ),
                usage_record("cc-switch:deepseek/deepseek-flash", "turn", 1_001_500),
                usage_record("cc-switch:deepseek/deepseek-flash", "session", 1_002_000),
                usage_record("cc-switch:proxy/gpt-6-luna", "turn", 1_003_000),
                "{partial".to_string(),
            ],
        );
        let db = Database::memory().unwrap();
        seed_pricing(&db, "deepseek-flash")?;

        let result = sync_kimicode_roots(&db, &[root]);
        assert_eq!(result.imported, 1);
        assert!(result.errors.is_empty());

        let conn = lock_conn!(db.conn);
        let row = conn
            .query_row(
                "SELECT provider_id, model, request_model, input_tokens, output_tokens,
                        cache_read_tokens, cache_creation_tokens, input_token_semantics,
                        input_cost_usd, output_cost_usd, cache_read_cost_usd,
                        cache_creation_cost_usd, total_cost_usd, latency_ms, session_id,
                        data_source
                 FROM proxy_request_logs WHERE request_id LIKE 'kimicode_session:%'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, String>(10)?,
                        row.get::<_, String>(11)?,
                        row.get::<_, String>(12)?,
                        row.get::<_, i64>(13)?,
                        row.get::<_, String>(14)?,
                        row.get::<_, String>(15)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row.0, "deepseek");
        assert_eq!(row.1, "deepseek-flash");
        assert_eq!(row.2, "cc-switch:deepseek/deepseek-flash");
        assert_eq!(row.3, 100);
        assert_eq!(row.4, 20);
        assert_eq!(row.5, 50);
        assert_eq!(row.6, 0);
        assert_eq!(row.7, INPUT_TOKEN_SEMANTICS_FRESH);
        assert_eq!(row.8, "0.0001");
        assert_eq!(row.9, "0.00004");
        assert_eq!(row.10, "0.000005");
        assert_eq!(row.11, "0");
        assert_eq!(row.12, "0.000145");
        assert_eq!(row.13, 1500);
        assert_eq!(row.14, SESSION);
        assert_eq!(row.15, DATA_SOURCE);
        Ok(())
    }

    #[test]
    fn imports_subagent_files_and_is_idempotent() -> Result<(), AppError> {
        let temp = tempdir().unwrap();
        let root = temp.path().join("sessions");
        write_session(
            &root,
            "main",
            &[
                llm_request("kimi-code/k3", "k3", 2_000_000),
                usage_record("kimi-code/k3", "turn", 2_001_000),
            ],
        );
        write_session(
            &root,
            "agent-1",
            &[usage_record("kimi-code/k3", "turn", 2_002_000)],
        );
        let db = Database::memory().unwrap();
        seed_pricing(&db, "k3")?;

        let first = sync_kimicode_roots(&db, &[root.clone()]);
        assert_eq!(first.imported, 2);
        let second = sync_kimicode_roots(&db, &[root]);
        assert_eq!(second.imported, 0);

        let conn = lock_conn!(db.conn);
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM proxy_request_logs WHERE data_source = ?1",
                [DATA_SOURCE],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
        Ok(())
    }

    #[test]
    fn incremental_sync_picks_up_appended_usage() -> Result<(), AppError> {
        let temp = tempdir().unwrap();
        let root = temp.path().join("sessions");
        let wire = write_session(
            &root,
            "main",
            &[usage_record(
                "cc-switch:deepseek/deepseek-flash",
                "turn",
                3_000_000,
            )],
        );
        let db = Database::memory().unwrap();
        assert_eq!(sync_kimicode_roots(&db, &[root.clone()]).imported, 1);

        {
            let mut file = fs::OpenOptions::new().append(true).open(&wire).unwrap();
            writeln!(
                file,
                "{}",
                usage_record("cc-switch:deepseek/deepseek-flash", "turn", 3_001_000)
            )
            .unwrap();
        }
        let result = sync_kimicode_roots(&db, &[root]);
        assert_eq!(result.imported, 1);

        let conn = lock_conn!(db.conn);
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM proxy_request_logs WHERE data_source = ?1",
                [DATA_SOURCE],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
        Ok(())
    }

    #[test]
    fn ledger_prevents_reimport_after_detail_prune() -> Result<(), AppError> {
        let temp = tempdir().unwrap();
        let root = temp.path().join("sessions");
        let wire = write_session(
            &root,
            "main",
            &[usage_record("kimi-code/k3", "turn", 7_000_000)],
        );
        let db = Database::memory().unwrap();
        assert_eq!(sync_kimicode_roots(&db, &[root.clone()]).imported, 1);

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "DELETE FROM proxy_request_logs WHERE data_source = ?1",
                [DATA_SOURCE],
            )?;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        {
            let mut file = fs::OpenOptions::new().append(true).open(&wire).unwrap();
            writeln!(file).unwrap();
        }

        let result = sync_kimicode_roots(&db, &[root]);
        assert_eq!(result.imported, 0);
        let conn = lock_conn!(db.conn);
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM proxy_request_logs WHERE data_source = ?1",
            [DATA_SOURCE],
            |row| row.get(0),
        )?;
        assert_eq!(count, 0);
        Ok(())
    }

    #[test]
    fn backfills_provider_ids_for_rows_imported_before_attribution() -> Result<(), AppError> {
        let db = Database::memory().unwrap();
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    latency_ms, status_code, created_at, data_source
                 ) VALUES
                 ('old-1', '_kimicode_session', 'kimicode', 'deepseek-flash',
                  'cc-switch:deepseek/deepseek-flash', 1, 1, 0, 0, 0, 200, 1, 'kimicode_session'),
                 ('old-2', '_kimicode_session', 'kimicode', 'gpt-6-luna',
                  'cc-switch:proxy/gpt-6-luna', 1, 1, 0, 0, 0, 200, 1, 'kimicode_session')",
                [],
            )?;
        }
        backfill_provider_ids(&db)?;

        let conn = lock_conn!(db.conn);
        let rows: Vec<(String, String)> = conn
            .prepare("SELECT request_id, provider_id FROM proxy_request_logs ORDER BY request_id")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        assert_eq!(rows[0], ("old-1".to_string(), "deepseek".to_string()));
        assert_eq!(
            rows[1],
            ("old-2".to_string(), "_kimicode_session".to_string())
        );
        Ok(())
    }

    #[test]
    fn prefers_the_upstream_model_from_llm_request() -> Result<(), AppError> {
        let temp = tempdir().unwrap();
        let root = temp.path().join("sessions");
        write_session(
            &root,
            "main",
            &[
                llm_request("cc-switch:vendor/friendly", "deepseek-v3", 4_000_000),
                usage_record("cc-switch:vendor/friendly", "turn", 4_001_000),
            ],
        );
        let db = Database::memory().unwrap();
        seed_pricing(&db, "deepseek-v3")?;

        assert_eq!(sync_kimicode_roots(&db, &[root]).imported, 1);
        let conn = lock_conn!(db.conn);
        let (provider_id, model, request_model): (String, String, String) = conn
            .query_row(
                "SELECT provider_id, model, request_model FROM proxy_request_logs
                 WHERE data_source = ?1",
                [DATA_SOURCE],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(provider_id, "vendor");
        assert_eq!(model, "deepseek-v3");
        assert_eq!(request_model, "cc-switch:vendor/friendly");
        Ok(())
    }

    #[test]
    fn keeps_tokens_when_pricing_is_missing() -> Result<(), AppError> {
        let temp = tempdir().unwrap();
        let root = temp.path().join("sessions");
        write_session(
            &root,
            "main",
            &[usage_record("kimi-code/kimi-for-coding", "turn", 5_000_000)],
        );
        let db = Database::memory().unwrap();

        assert_eq!(sync_kimicode_roots(&db, &[root]).imported, 1);
        let conn = lock_conn!(db.conn);
        let (provider_id, model, total_cost): (String, String, String) = conn
            .query_row(
                "SELECT provider_id, model, total_cost_usd FROM proxy_request_logs
                 WHERE data_source = ?1",
                [DATA_SOURCE],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(provider_id, PROVIDER_PLACEHOLDER);
        assert_eq!(model, "kimi-for-coding");
        assert_eq!(total_cost, "0");
        Ok(())
    }

    #[test]
    fn imports_legacy_session_without_state_id() -> Result<(), AppError> {
        let temp = tempdir().unwrap();
        let root = temp.path().join("sessions");
        let session_dir = root.join(WORKSPACE).join(SESSION);
        fs::create_dir_all(session_dir.join("agents/main")).unwrap();
        fs::write(session_dir.join("state.json"), "{}\n").unwrap();
        fs::write(
            session_dir.join("agents/main/wire.jsonl"),
            format!("{}\n", usage_record("kimi-code/k3", "turn", 6_000_000)),
        )
        .unwrap();

        let db = Database::memory().unwrap();
        assert_eq!(sync_kimicode_roots(&db, &[root]).imported, 1);
        Ok(())
    }

    #[test]
    #[ignore = "read-only validation against a copied database and the local Kimi history"]
    fn kimi_local_usage_smoke() -> Result<(), AppError> {
        let test_home = std::env::var("CC_SWITCH_TEST_HOME")
            .expect("set CC_SWITCH_TEST_HOME to an isolated copy of the config dir");
        assert!(
            !test_home.trim().is_empty(),
            "CC_SWITCH_TEST_HOME must not be empty"
        );
        let db = Database::init()?;
        let root = std::env::var("CC_SWITCH_KIMI_HISTORY_ROOT")
            .expect("set CC_SWITCH_KIMI_HISTORY_ROOT to the local sessions directory");

        let before = {
            let conn = lock_conn!(db.conn);
            conn.query_row(
                "SELECT COUNT(*) FROM proxy_request_logs WHERE data_source = ?1",
                [DATA_SOURCE],
                |row| row.get::<_, i64>(0),
            )?
        };
        let result = sync_kimicode_roots(&db, &[PathBuf::from(root)]);
        let after = {
            let conn = lock_conn!(db.conn);
            conn.query_row(
                "SELECT COUNT(*) FROM proxy_request_logs WHERE data_source = ?1",
                [DATA_SOURCE],
                |row| row.get::<_, i64>(0),
            )?
        };

        eprintln!(
            "Kimi usage smoke: imported={} skipped={} errors={} rows={} -> {}",
            result.imported,
            result.skipped,
            result.errors.len(),
            before,
            after
        );
        for error in &result.errors {
            eprintln!("  error: {error}");
        }
        assert!(after >= before);
        assert!(after > 0);
        Ok(())
    }
}
