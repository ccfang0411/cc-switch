//! Kimi Code 2.x stores session metadata separately from its append-only wire log.
//! Read context/user and loop events, not mirrored UI events or LLM request snapshots.
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::blocks::{
    assign_turn_ids, thinking_block, tool_call_block, tool_result_block, ToolSource,
};
use super::utils::{extract_text, for_each_jsonl_value, parse_timestamp_to_ms, truncate_summary};
use crate::session_manager::model::{SessionBlock, ToolStatus};
use crate::session_manager::{SessionMessage, SessionMeta};

pub fn session_roots() -> Vec<PathBuf> {
    vec![crate::kimi_config::data_dir().join("sessions")]
}

pub fn scan_sessions() -> Vec<SessionMeta> {
    scan_root(&session_roots()[0])
}

fn scan_root(root: &Path) -> Vec<SessionMeta> {
    let mut sessions = Vec::new();
    let Ok(workspaces) = std::fs::read_dir(root) else {
        return sessions;
    };
    for workspace in workspaces.flatten() {
        // Never follow directory symlinks while discovering session files.
        if !workspace.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(workspace.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            if let Some(meta) = parse_meta(&entry.path()) {
                sessions.push(meta);
            }
        }
    }
    sessions
}

fn parse_meta(dir: &Path) -> Option<SessionMeta> {
    let state: Value = serde_json::from_slice(&std::fs::read(dir.join("state.json")).ok()?).ok()?;
    let id = state.get("id")?.as_str()?;
    if dir.file_name()?.to_str()? != id {
        return None;
    }
    let wire = dir.join("agents/main/wire.jsonl");
    if !wire.is_file() {
        return None;
    }
    let home = crate::kimi_config::data_dir();
    let binary = crate::config::get_home_dir().join(".kimi-code/bin/kimi");
    let executable = if binary.is_file() {
        binary.to_string_lossy().into_owned()
    } else {
        "kimi".to_string()
    };
    let quote = crate::session_manager::terminal::shell_escape;
    Some(SessionMeta {
        provider_id: "kimicode".into(),
        session_id: id.into(),
        title: state
            .get("title")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(|s| truncate_summary(s, 80)),
        summary: state
            .get("lastPrompt")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(|s| truncate_summary(s, 160)),
        project_dir: state.get("cwd").and_then(Value::as_str).map(str::to_string),
        created_at: state.get("createdAt").and_then(parse_timestamp_to_ms),
        last_active_at: state.get("updatedAt").and_then(parse_timestamp_to_ms),
        source_path: Some(wire.to_string_lossy().into_owned()),
        resume_command: Some(format!(
            "KIMI_CODE_HOME={} {} --session {}",
            quote(&home.to_string_lossy()),
            quote(&executable),
            quote(id)
        )),
    })
}

fn flush(messages: &mut Vec<SessionMessage>, pending: &mut Option<SessionMessage>) {
    if let Some(message) = pending.take().filter(|m| !m.is_empty()) {
        // Recompute the text projection after appending streamed content blocks.
        messages.push(SessionMessage::from_blocks(
            message.role,
            message.ts,
            message.blocks,
        ));
    }
}

pub fn load_messages(path: &Path) -> Result<Vec<SessionMessage>, String> {
    let mut messages = Vec::new();
    let mut pending: Option<SessionMessage> = None;
    for_each_jsonl_value(path, |span, value| {
        let ts = value.get("time").and_then(parse_timestamp_to_ms);
        match value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "context.append_message" => {
                let Some(message) = value.get("message") else {
                    return Ok(());
                };
                let role = message
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !matches!(role, "user" | "system") {
                    return Ok(());
                }
                flush(&mut messages, &mut pending);
                let text = message.get("content").map(extract_text).unwrap_or_default();
                if text.is_empty() {
                    return Ok(());
                }
                let mut item =
                    SessionMessage::from_blocks(role, ts, vec![SessionBlock::text(text)]);
                item.id = message
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                item.injected = role == "system"
                    || message
                        .pointer("/origin/kind")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind != "user");
                messages.push(item);
            }
            "context.append_loop_event" => {
                let Some(event) = value.get("event") else {
                    return Ok(());
                };
                match event
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                {
                    "step.begin" | "step.end" => flush(&mut messages, &mut pending),
                    "content.part" => {
                        let Some(part) = event.get("part") else {
                            return Ok(());
                        };
                        let text = part
                            .get("text")
                            .or_else(|| part.get("think"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let block = match part.get("type").and_then(Value::as_str) {
                            Some("text") => SessionBlock::text(text),
                            Some("think") | Some("thinking") => {
                                thinking_block(text, None, None, || {
                                    Some(span.content_ref(if part.get("think").is_some() {
                                        "/event/part/think"
                                    } else {
                                        "/event/part/text"
                                    }))
                                })
                            }
                            _ => return Ok(()),
                        };
                        pending
                            .get_or_insert_with(|| {
                                SessionMessage::from_blocks("assistant", ts, vec![])
                            })
                            .blocks
                            .push(block);
                    }
                    "tool.call" => {
                        let input = event.get("args").unwrap_or(&Value::Null);
                        let block = tool_call_block(
                            ToolSource::Generic,
                            event
                                .get("toolCallId")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                            event
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown"),
                            input,
                            || Some(span.content_ref("/event/args")),
                        );
                        pending
                            .get_or_insert_with(|| {
                                SessionMessage::from_blocks("assistant", ts, vec![])
                            })
                            .blocks
                            .push(block);
                        flush(&mut messages, &mut pending);
                    }
                    "tool.result" => {
                        flush(&mut messages, &mut pending);
                        let output = event.pointer("/result/output");
                        let text = output.map(extract_text).unwrap_or_default();
                        let failed = event
                            .pointer("/result/isError")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                            || event
                                .pointer("/result/error")
                                .is_some_and(|error| !error.is_null());
                        let block = tool_result_block(
                            event
                                .get("toolCallId")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                            if failed {
                                ToolStatus::Error
                            } else {
                                ToolStatus::Success
                            },
                            &text,
                            || Some(span.content_ref("/event/result/output")),
                        );
                        messages.push(SessionMessage::from_blocks("tool", ts, vec![block]));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    })?;
    flush(&mut messages, &mut pending);
    assign_turn_ids(&mut messages);
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn kimi_scans_metadata_and_quotes_resume_command() {
        let temp = tempdir().unwrap();
        let dir = temp.path().join("wd_project/session_test-id");
        std::fs::create_dir_all(dir.join("agents/main")).unwrap();
        std::fs::write(dir.join("agents/main/wire.jsonl"), "").unwrap();
        std::fs::write(dir.join("state.json"), r#"{"id":"session_test-id","cwd":"/tmp/project","title":"Kimi history","lastPrompt":"hello","createdAt":1780000000000,"updatedAt":1780000001000}"#).unwrap();
        let sessions = scan_root(temp.path());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].provider_id, "kimicode");
        assert_eq!(sessions[0].title.as_deref(), Some("Kimi history"));
        assert_eq!(sessions[0].last_active_at, Some(1780000001000));
        assert!(sessions[0]
            .resume_command
            .as_ref()
            .unwrap()
            .contains("--session 'session_test-id'"));
        std::fs::write(dir.join("state.json"), r#"{"id":"different-id"}"#).unwrap();
        assert!(scan_root(temp.path()).is_empty());
    }

    #[test]
    fn kimi_reads_wire_without_mirrored_messages_and_keeps_tool_refs() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("wire.jsonl");
        let output = "x".repeat(1500);
        let events = vec![
            serde_json::json!({"type":"context.append_message","time":1780000000000_i64,"message":{"id":"u1","role":"user","content":[{"type":"text","text":"hello"}],"origin":{"kind":"user"}}}),
            serde_json::json!({"type":"agent.message.appended","message":{"message":{"role":"user","content":[{"type":"text","text":"hello"}]}}}),
            serde_json::json!({"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"think","think":"thinking"}}}),
            serde_json::json!({"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"text","text":"checking"}}}),
            serde_json::json!({"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"call-1","name":"Shell","args":{"command":"pwd"}}}),
            serde_json::json!({"type":"context.append_loop_event","event":{"type":"tool.result","toolCallId":"call-1","result":{"output":output}}}),
            serde_json::json!({"type":"agent.message.appended","message":{"message":{"role":"assistant","content":[{"type":"text","text":"checking"}]}}}),
            serde_json::json!({"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"text","text":"done"}}}),
        ];
        std::fs::write(
            &path,
            events
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n{partial",
        )
        .unwrap();
        let messages = load_messages(&path).unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].content, "hello");
        assert_eq!(messages[1].blocks.len(), 3);
        assert!(
            matches!(&messages[1].blocks[0], SessionBlock::Thinking { text, .. } if text == "thinking")
        );
        let source = crate::session_manager::content::ValidatedSource {
            provider_id: "kimicode".into(),
            raw: path.to_string_lossy().into_owned(),
            location: crate::session_manager::content::SourceLocation::Path {
                path: path.clone(),
                root: temp.path().to_path_buf(),
            },
        };
        if let SessionBlock::ToolResult {
            full: Some(reference),
            ..
        } = &messages[2].blocks[0]
        {
            assert_eq!(
                crate::session_manager::content::resolve_content_ref(&source, reference).unwrap(),
                output
            );
        } else {
            panic!("Missing long tool output reference");
        }
        assert_eq!(messages[3].content, "done");
        assert_eq!(messages[3].turn_id.as_deref(), Some("t1"));
        assert!(
            matches!(&messages[2].blocks[0], SessionBlock::ToolResult { call_id, truncated: true, full: Some(_), .. } if call_id == "call-1")
        );
    }

    #[test]
    #[ignore = "read-only inspection of locally installed Kimi history"]
    fn kimi_local_history_smoke() {
        let root = std::env::var("CC_SWITCH_KIMI_HISTORY_ROOT")
            .expect("Set CC_SWITCH_KIMI_HISTORY_ROOT to the local sessions directory");
        let sessions = scan_root(Path::new(&root));
        assert!(!sessions.is_empty());
        let mut readable = 0;
        for meta in &sessions {
            if !load_messages(Path::new(meta.source_path.as_ref().unwrap()))
                .unwrap()
                .is_empty()
            {
                readable += 1;
            }
        }
        assert!(readable > 0);
        eprintln!("Kimi sessions: {}, readable: {readable}", sessions.len());
    }
}
