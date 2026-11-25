//! Read Kimi Code's session metadata and committed main-agent wire history.
use super::{blocks, utils};
use crate::kimi_session::RequestIndex;
use crate::session_manager::model::{ContentRef, MessageMeta, SessionBlock, ToolStatus};
use crate::session_manager::{SessionMessage, SessionMeta};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub mod resume;

pub(crate) fn session_roots() -> Vec<PathBuf> {
    vec![crate::kimi_config::data_dir().join("sessions")]
}

pub fn scan_sessions() -> Vec<SessionMeta> {
    scan(&crate::kimi_config::data_dir())
}

fn scan(home: &Path) -> Vec<SessionMeta> {
    let mut sessions = Vec::new();
    let Ok(workspaces) = fs::read_dir(home.join("sessions")) else {
        return sessions;
    };
    for workspace in workspaces.flatten() {
        if !workspace.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(entries) = fs::read_dir(workspace.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let wire = entry.path().join("agents/main/wire.jsonl");
            let id = entry.file_name().to_string_lossy().into_owned();
            let Ok(dir) = session_dir_from_source(&home.join("sessions"), &wire, &id) else {
                continue;
            };
            let state = [dir.join("state.json"), dir.join("session-meta/state.json")]
                .into_iter()
                .find_map(|path| {
                    let canonical = path.canonicalize().ok()?;
                    if !canonical.starts_with(&dir) || !canonical.is_file() {
                        return None;
                    }
                    fs::read(canonical)
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                });
            let Some(state) = state else {
                continue;
            };
            if state.get("archived").and_then(Value::as_bool) == Some(true)
                || state
                    .pointer("/custom/session_kind")
                    .and_then(Value::as_str)
                    == Some("child")
            {
                continue;
            }
            let command = resume_command(home, &id);
            sessions.push(SessionMeta {
                provider_id: "kimi".into(),
                session_id: id,
                title: state
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                summary: state
                    .get("lastPrompt")
                    .and_then(Value::as_str)
                    .map(|text| text.chars().take(200).collect()),
                project_dir: state
                    .get("cwd")
                    .or_else(|| state.get("workDir"))
                    .or_else(|| state.pointer("/custom/cwd"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                created_at: state
                    .get("createdAt")
                    .and_then(utils::parse_timestamp_to_ms),
                last_active_at: state
                    .get("updatedAt")
                    .and_then(utils::parse_timestamp_to_ms),
                source_path: Some(wire.to_string_lossy().into_owned()),
                resume_command: command,
            });
        }
    }
    sessions
}

fn resume_command(home: &Path, id: &str) -> Option<String> {
    if !is_valid_session_id(id) {
        return None;
    }
    #[cfg(windows)]
    let command = format!(
        "$env:KIMI_CODE_HOME = '{}'; kimi --session {id}",
        home.to_string_lossy().replace('\'', "''")
    );
    #[cfg(not(windows))]
    let command = format!(
        "env KIMI_CODE_HOME={} kimi --session {id}",
        crate::session_manager::terminal::shell_escape(&home.to_string_lossy())
    );
    Some(command)
}

/// Resolve branch switches before projecting messages, so undone/forked turns stay hidden.
struct BranchSegment {
    start: usize,
    end: usize,
    base: Option<(String, usize)>,
}

fn active_records(records: &[(utils::JsonlSpan, Value)]) -> Result<Vec<usize>, String> {
    let switches: Vec<_> = records
        .iter()
        .enumerate()
        .filter(|(_, (_, record))| record["type"] == "agent.switched")
        .collect();
    let mut active = Vec::new();
    let mut segments = std::collections::HashMap::new();
    let mut branch = "main".to_string();
    let mut start = 0;
    let mut base = None;
    for (index, (_, record)) in switches {
        segments.insert(
            branch,
            BranchSegment {
                start,
                end: index,
                base,
            },
        );
        branch = record
            .get("branch")
            .and_then(Value::as_str)
            .ok_or("Invalid Kimi branch")?
            .into();
        base = Some((
            record
                .pointer("/base/branch")
                .and_then(Value::as_str)
                .ok_or("Invalid Kimi branch base")?
                .into(),
            record
                .pointer("/base/line")
                .and_then(Value::as_u64)
                .ok_or("Invalid Kimi branch line")? as usize,
        ));
        start = index + 1;
    }
    segments.insert(
        branch.clone(),
        BranchSegment {
            start,
            end: records.len(),
            base,
        },
    );
    let mut chain = Vec::new();
    let mut limit = records.len();
    let mut visited = std::collections::HashSet::new();
    loop {
        if !visited.insert(branch.clone()) {
            return Err("Kimi branch chain contains a cycle".into());
        }
        let segment = segments.get(&branch).ok_or("Unknown Kimi branch")?;
        chain.push((segment.start, segment.end.min(limit)));
        if let Some((parent, line)) = &segment.base {
            branch = parent.clone();
            limit = *line;
        } else {
            break;
        }
    }
    for (start, end) in chain.into_iter().rev() {
        active.extend(start..end);
    }
    Ok(active)
}

pub fn load_messages(path: &Path) -> Result<Vec<SessionMessage>, String> {
    let mut records = Vec::new();
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut lines = utils::LineSpans::new(std::io::BufReader::new(file));
    while let Some(line) = lines.next_line().map_err(|error| error.to_string())? {
        // Keep physical line positions for agent.switched base.line, including malformed tails.
        records.push((
            line.span,
            serde_json::from_slice::<Value>(line.bytes).unwrap_or(Value::Null),
        ));
    }
    let active = active_records(&records)?;
    let mut requests = RequestIndex::default();
    for index in &active {
        requests.observe(&records[*index].1);
    }
    // Newer files include the committed human projection. Prefer it to the old context stream.
    let has_human = active.iter().any(|index| {
        matches!(
            records[*index].1["type"].as_str(),
            Some("agent.message.appended" | "human.agent.message.appended")
        )
    });
    let mut messages = Vec::new();
    for index in active {
        let (span, record) = &records[index];
        let kind = record["type"].as_str().unwrap_or_default();
        let (entry, pointer) = if has_human
            && matches!(
                kind,
                "agent.message.appended" | "human.agent.message.appended"
            ) {
            (&record["message"], "/message/message")
        } else if !has_human && kind == "context.append_message" {
            (record, "/message")
        } else {
            continue;
        };
        let Some(raw) = record.pointer(pointer) else {
            continue;
        };
        let Some(role @ ("user" | "assistant" | "tool" | "system")) = raw["role"].as_str() else {
            continue;
        };
        let mut message_blocks = Vec::new();
        let reference = |pointer: String| {
            Some(ContentRef::Jsonl {
                offset: span.offset,
                len: span.len,
                pointer,
            })
        };
        if role == "tool" {
            let text = utils::extract_text(&raw["content"]);
            message_blocks.push(blocks::tool_result_block(
                raw["toolCallId"].as_str().unwrap_or_default(),
                ToolStatus::Success,
                &text,
                || reference(format!("{pointer}/content")),
            ));
        } else {
            for (i, part) in raw["content"].as_array().into_iter().flatten().enumerate() {
                match part["type"].as_str() {
                    Some("text") => {
                        if let Some(text) = part["text"].as_str() {
                            message_blocks.push(SessionBlock::text(text));
                        }
                    }
                    Some("think") => message_blocks.push(blocks::thinking_block(
                        part["think"].as_str().unwrap_or_default(),
                        None,
                        None,
                        || reference(format!("{pointer}/content/{i}/think")),
                    )),
                    _ => {}
                }
            }
            for (i, call) in raw["toolCalls"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                let input = blocks::parse_arguments(&call["arguments"]);
                message_blocks.push(blocks::tool_call_block(
                    blocks::ToolSource::Generic,
                    call["id"].as_str().unwrap_or_default(),
                    call["name"].as_str().unwrap_or_default(),
                    &input,
                    || reference(format!("{pointer}/toolCalls/{i}/arguments")),
                ));
            }
        }
        if message_blocks.is_empty() {
            continue;
        }
        let mut message = SessionMessage::from_blocks(
            role,
            record.get("time").and_then(utils::parse_timestamp_to_ms),
            message_blocks,
        );
        let meta = &entry["meta"];
        message.injected = role == "system"
            || matches!(
                meta.pointer("/origin/kind")
                    .or_else(|| raw.pointer("/origin/kind"))
                    .and_then(Value::as_str),
                Some("injection" | "system_trigger" | "retry" | "compaction_summary")
            );
        if let Some(usage) = meta.get("usage") {
            message.meta = Some(MessageMeta {
                model: requests.model(meta).map(str::to_owned),
                input_tokens: usage["inputOther"].as_u64(),
                output_tokens: usage["output"].as_u64(),
                cache_read_tokens: usage["inputCacheRead"].as_u64(),
                cache_write_tokens: usage["inputCacheCreation"].as_u64(),
                duration_ms: requests
                    .get(meta)
                    .and_then(|info| (info.latency_ms > 0).then_some(info.latency_ms as u64)),
                ..Default::default()
            });
        }
        messages.push(message);
    }
    blocks::assign_turn_ids(&mut messages);
    Ok(messages)
}

fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// Mirror Kimi Code's native delete: remove `sessions/<workspace>/<id>/`, then append the
/// `{sessionId, deleted}` tombstone to `session_index.jsonl` and drop an `.index-dirty` mark
/// so a running Kimi reconciles its index. Kimi also compacts stale index entries on its own,
/// so the index updates are best-effort once the directory is gone.
fn session_dir_from_source(root: &Path, path: &Path, session_id: &str) -> Result<PathBuf, String> {
    if !is_valid_session_id(session_id) {
        return Err("Invalid Kimi session ID".into());
    }
    let session_dir = path
        .strip_prefix(root)
        .ok()
        .filter(|rel| {
            let parts: Vec<_> = rel.components().map(|c| c.as_os_str()).collect();
            parts.len() == 5 && parts[2..] == ["agents", "main", "wire.jsonl"]
        })
        .and_then(|_| path.ancestors().nth(3))
        .ok_or_else(|| format!("Unexpected Kimi session source: {}", path.display()))?;
    if session_dir.file_name().and_then(|name| name.to_str()) != Some(session_id) {
        return Err(format!(
            "Kimi session directory does not match session ID: {}",
            session_dir.display()
        ));
    }
    if !fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file()) {
        return Err("Kimi session source must be a regular file".into());
    }
    let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
    let relative = session_dir
        .strip_prefix(root)
        .map_err(|error| error.to_string())?;
    if relative
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err("Invalid Kimi session path".into());
    }
    let canonical_dir = session_dir
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let canonical_wire = path.canonicalize().map_err(|error| error.to_string())?;
    if canonical_dir != canonical_root.join(relative)
        || canonical_wire != canonical_dir.join("agents/main/wire.jsonl")
    {
        return Err("Kimi session source resolves outside its expected directory".into());
    }
    Ok(canonical_dir)
}

pub fn delete_session(root: &Path, path: &Path, session_id: &str) -> Result<bool, String> {
    let session_dir = session_dir_from_source(root, path, session_id)?;
    fs::remove_dir_all(&session_dir).map_err(|e| {
        format!(
            "Failed to delete Kimi session directory {}: {e}",
            session_dir.display()
        )
    })?;

    if let Err(e) = append_index_tombstone(root, session_id) {
        log::warn!("Failed to record Kimi session deletion in session_index.jsonl: {e}");
    }
    mark_index_dirty(root, session_id);
    Ok(true)
}

fn mark_index_dirty(root: &Path, session_id: &str) {
    let mark = root.join(".index-dirty").join(format!(
        "{session_id}.{}",
        chrono::Utc::now().timestamp_millis()
    ));
    if let Err(e) =
        fs::create_dir_all(root.join(".index-dirty")).and_then(|_| fs::write(&mark, b""))
    {
        log::warn!("Failed to mark Kimi session index dirty: {e}");
    }
}

pub fn rename_session(
    root: &Path,
    path: &Path,
    session_id: &str,
    title: &str,
) -> Result<bool, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("Session title cannot be empty".into());
    }
    let session_dir = session_dir_from_source(root, path, session_id)?;
    let state_path = [
        session_dir.join("state.json"),
        session_dir.join("session-meta/state.json"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .ok_or("Kimi session metadata was not found")?;
    let validated_state = state_path
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !validated_state.starts_with(&session_dir) {
        return Err("Kimi session metadata is outside its session directory".into());
    }
    let mut state: Value =
        serde_json::from_slice(&fs::read(&state_path).map_err(|error| error.to_string())?)
            .map_err(|error| format!("Invalid Kimi session metadata: {error}"))?;
    if !state.is_object()
        || state
            .get("id")
            .is_some_and(|id| id.as_str() != Some(session_id))
    {
        return Err("Kimi session metadata does not match the session ID".into());
    }
    state["title"] = Value::String(title.into());
    state["titleKind"] = Value::String("custom".into());
    state["isCustomTitle"] = Value::Bool(true);
    let bytes = serde_json::to_vec(&state).map_err(|error| error.to_string())?;
    crate::config::atomic_write_private(&state_path, &bytes).map_err(|error| error.to_string())?;
    mark_index_dirty(root, session_id);
    Ok(true)
}

fn append_index_tombstone(root: &Path, session_id: &str) -> std::io::Result<()> {
    use std::io::Write;
    let Some(home) = root.parent() else {
        return Ok(());
    };
    let line = serde_json::json!({ "sessionId": session_id, "deleted": true });
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join("session_index.jsonl"))?;
    writeln!(file, "{line}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn kimi_sessions_use_native_state_and_committed_messages_once() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("sessions/wd_project_123/session-1");
        fs::create_dir_all(dir.join("agents/main")).unwrap();
        fs::write(dir.join("state.json"), json!({"title":"Project","cwd":"/project","createdAt":1700000000000i64,"updatedAt":1700000001000i64}).to_string()).unwrap();
        let wire = dir.join("agents/main/wire.jsonl");
        let lines = [
            json!({"type":"metadata","protocol_version":"1.5"}),
            json!({"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"duplicate"}]}}),
            json!({"type":"agent.message.appended","message":{"message":{"role":"user","content":[{"type":"text","text":"Fix this"}]}}}),
            json!({"type":"agent.message.appended","message":{"message":{"role":"assistant","content":[{"type":"text","text":"Fixed"}],"toolCalls":[]},"meta":{"model":{"provider":"custom","model":"vendor/model"},"usage":{"inputOther":10,"output":20,"inputCacheRead":30,"inputCacheCreation":0}}}}),
        ];
        fs::write(
            &wire,
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let sessions = scan(home.path());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].project_dir.as_deref(), Some("/project"));
        let messages = load_messages(&wire).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "Fix this");
        assert_eq!(
            messages[1].meta.as_ref().unwrap().model.as_deref(),
            Some("vendor/model")
        );
    }

    #[test]
    fn kimi_branch_switch_omits_abandoned_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wire.jsonl");
        let message = |text| json!({"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":text}]}});
        let lines = [
            message("keep"),
            message("undone"),
            json!({"type":"agent.switched","branch":"fork","base":{"branch":"main","line":1}}),
            message("replacement"),
        ];
        fs::write(
            &path,
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let messages = load_messages(&path).unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>(),
            vec!["keep", "replacement"]
        );
    }

    #[test]
    fn kimi_session_reader_resolves_model_from_matching_historical_request() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wire.jsonl");
        let lines = [
            json!({"type":"context.append_loop_event","event":{"type":"step.begin","uuid":"step","turnId":"0","step":1}}),
            json!({"type":"llm.request","agentId":"main","kind":"loop","turnStep":"0.1","model":"actual-model"}),
            json!({"type":"llm.request","kind":"compaction","model":"summary-model"}),
            json!({"type":"agent.message.appended","message":{"message":{"role":"assistant","content":[{"type":"text","text":"Result"}]},"meta":{"messageId":"response","model":{"model":"agent-loop","provider":"agent-loop"},"usage":{"inputOther":10,"output":900}}}}),
            json!({"type":"context.append_loop_event","event":{"type":"step.end","uuid":"step","messageId":"response","llmFirstTokenLatencyMs":1000,"llmStreamDurationMs":9000}}),
        ];
        fs::write(
            &path,
            lines
                .iter()
                .map(|line| format!("{line}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let messages = load_messages(&path).unwrap();
        assert_eq!(messages.len(), 1);
        let meta = messages[0].meta.as_ref().unwrap();
        assert_eq!(meta.model.as_deref(), Some("actual-model"));
        assert_eq!(meta.duration_ms, Some(10_000));
        assert_eq!(meta.output_tokens, Some(900));
    }

    #[test]
    fn kimi_delete_removes_session_dir_and_records_tombstone() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("sessions");
        let dir = root.join("wd_project_123/session_abc-1");
        let sibling = root.join("wd_project_123/session_keep");
        fs::create_dir_all(dir.join("agents/main")).unwrap();
        fs::create_dir_all(sibling.join("agents/main")).unwrap();
        let wire = dir.join("agents/main/wire.jsonl");
        fs::write(
            &wire, "{}
",
        )
        .unwrap();
        fs::write(
            sibling.join("agents/main/wire.jsonl"),
            "{}
",
        )
        .unwrap();
        fs::write(
            home.path().join("session_index.jsonl"),
            "{\"sessionId\":\"session_abc-1\",\"sessionDir\":\"x\",\"workDir\":\"/p\"}
",
        )
        .unwrap();

        assert!(delete_session(&root, &wire, "session_abc-1").unwrap());

        assert!(!dir.exists());
        assert!(sibling.exists());
        let index = fs::read_to_string(home.path().join("session_index.jsonl")).unwrap();
        let last: Value = serde_json::from_str(index.lines().last().unwrap()).unwrap();
        assert_eq!(last, json!({"sessionId":"session_abc-1","deleted":true}));
        let marks: Vec<_> = fs::read_dir(root.join(".index-dirty"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(marks.len(), 1);
        assert!(marks[0].starts_with("session_abc-1."));
    }

    #[test]
    fn kimi_rename_preserves_metadata_and_wire_for_both_state_locations() {
        for relative_state in ["state.json", "session-meta/state.json"] {
            let home = tempfile::tempdir().unwrap();
            let root = home.path().join("sessions");
            let dir = root.join("workspace/session-1");
            fs::create_dir_all(dir.join("agents/main")).unwrap();
            let wire = dir.join("agents/main/wire.jsonl");
            fs::write(&wire, "conversation stays unchanged\n").unwrap();
            let state = dir.join(relative_state);
            fs::create_dir_all(state.parent().unwrap()).unwrap();
            fs::write(
                &state,
                json!({"id":"session-1","title":"Old","custom":{"cwd":"/project"},"updatedAt":100})
                    .to_string(),
            )
            .unwrap();
            assert!(rename_session(&root, &wire, "session-1", "  新标题  ").unwrap());
            let saved: Value = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
            assert_eq!(saved["title"], "新标题");
            assert_eq!(saved["titleKind"], "custom");
            assert_eq!(saved["isCustomTitle"], true);
            assert_eq!(saved["custom"]["cwd"], "/project");
            assert_eq!(saved["updatedAt"], 100);
            assert_eq!(
                fs::read_to_string(&wire).unwrap(),
                "conversation stays unchanged\n"
            );
            assert_eq!(fs::read_dir(root.join(".index-dirty")).unwrap().count(), 1);
            assert!(!home.path().join("session_index.jsonl").exists());
        }
    }

    #[test]
    fn kimi_rename_rejects_empty_title_and_mismatched_metadata() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("sessions");
        let dir = root.join("workspace/session-1");
        fs::create_dir_all(dir.join("agents/main")).unwrap();
        let wire = dir.join("agents/main/wire.jsonl");
        fs::write(&wire, "").unwrap();
        let state = dir.join("state.json");
        let original = json!({"id":"session-2","title":"Old"}).to_string();
        fs::write(&state, &original).unwrap();
        assert!(rename_session(&root, &wire, "session-1", "   ").is_err());
        assert!(rename_session(&root, &wire, "session-1", "New").is_err());
        assert!(rename_session(&root, &wire, "../session-1", "New").is_err());
        assert!(rename_session(&root.join("workspace"), &wire, "session-1", "New").is_err());
        assert_eq!(fs::read_to_string(state).unwrap(), original);
        assert!(!root.join(".index-dirty").exists());
    }

    #[cfg(unix)]
    #[test]
    fn kimi_session_operations_never_follow_nested_symlinks() {
        for relative in [
            "workspace",
            "workspace/session-1",
            "workspace/session-1/agents",
            "workspace/session-1/agents/main",
            "workspace/session-1/agents/main/wire.jsonl",
        ] {
            let home = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let root = home.path().join("sessions");
            let outside_dir = outside.path().join("workspace/session-1");
            fs::create_dir_all(outside_dir.join("agents/main")).unwrap();
            fs::write(outside_dir.join("agents/main/wire.jsonl"), "unchanged\n").unwrap();
            fs::write(outside_dir.join("state.json"), "{\"title\":\"Original\"}").unwrap();
            let link = root.join(relative);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(outside.path().join(relative), &link).unwrap();
            let wire = root.join("workspace/session-1/agents/main/wire.jsonl");
            assert!(
                rename_session(&root, &wire, "session-1", "New").is_err(),
                "{relative}"
            );
            assert!(
                delete_session(&root, &wire, "session-1").is_err(),
                "{relative}"
            );
            assert!(scan(home.path()).is_empty(), "{relative}");
            assert_eq!(
                fs::read_to_string(outside_dir.join("agents/main/wire.jsonl")).unwrap(),
                "unchanged\n"
            );
            assert_eq!(
                fs::read_to_string(outside_dir.join("state.json")).unwrap(),
                "{\"title\":\"Original\"}"
            );
            assert!(!home.path().join("session_index.jsonl").exists());
            assert!(!root.join(".index-dirty").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn kimi_rename_rejects_metadata_symlink_outside_the_session() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = home.path().join("sessions");
        let dir = root.join("workspace/session-1");
        fs::create_dir_all(dir.join("agents/main")).unwrap();
        let wire = dir.join("agents/main/wire.jsonl");
        fs::write(&wire, "unchanged\n").unwrap();
        let state = outside.path().join("state.json");
        fs::write(&state, "{\"title\":\"Outside\"}").unwrap();
        std::os::unix::fs::symlink(&state, dir.join("state.json")).unwrap();
        assert!(rename_session(&root, &wire, "session-1", "New").is_err());
        assert!(scan(home.path()).is_empty());
        assert_eq!(
            fs::read_to_string(&state).unwrap(),
            "{\"title\":\"Outside\"}"
        );
        assert!(!root.join(".index-dirty").exists());
    }

    #[test]
    fn kimi_delete_rejects_unexpected_layout_or_mismatched_id() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("sessions");
        let dir = root.join("wd_project_123/session_abc");
        fs::create_dir_all(dir.join("agents/sub")).unwrap();
        let sub_wire = dir.join("agents/sub/wire.jsonl");
        fs::write(
            &sub_wire, "{}
",
        )
        .unwrap();
        fs::create_dir_all(dir.join("agents/main")).unwrap();
        let wire = dir.join("agents/main/wire.jsonl");
        fs::write(
            &wire, "{}
",
        )
        .unwrap();

        assert!(delete_session(&root, &sub_wire, "session_abc").is_err());
        assert!(delete_session(&root, &wire, "session_other").is_err());
        assert!(delete_session(&root, &wire, "../session_abc").is_err());
        assert!(delete_session(&root.join("wd_project_123"), &wire, "session_abc").is_err());
        assert!(dir.exists());
        assert!(!home.path().join("session_index.jsonl").exists());
    }
}
