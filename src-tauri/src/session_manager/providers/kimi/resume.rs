use super::session_dir_from_source;
use crate::app_config::AppType;
use crate::kimi_config::{self, PROXY_PLACEHOLDER, RESERVED_PROXY_TABLE};
use crate::live::engine::DeviceStore;
use crate::mode::state::{self, AppLiveState};
use crate::session_manager::terminal::shell_escape;
use crate::store::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(test)]
#[path = "resume_tests.rs"]
mod tests;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeTarget {
    session_id: String,
    source_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelection {
    alias: String,
    revision: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResumeKind {
    Original,
    New,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResumeAction {
    Open,
    Copy,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ResumeError {
    InvalidSession,
    InvalidConfig,
    ProxyNotReady,
    ModelChanged,
    MissingProject,
    TerminalFailed,
    Internal,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeModel {
    alias: String,
    model: String,
    provider: String,
    display_name: Option<String>,
    is_default: bool,
    revision: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum ResumeMode {
    Direct,
    Route,
    Stack,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeOptions {
    models: Vec<ResumeModel>,
    mode: ResumeMode,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ResumeOutcome {
    Opened,
    Command { command: String },
}

pub(crate) struct ResumeContext {
    home: PathBuf,
    cwd: PathBuf,
    session_id: String,
    pub(crate) options: ResumeOptions,
}

fn text(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| is_plain_arg(s))
}

fn is_plain_arg(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().any(char::is_control)
}

fn read_target(home: &Path, target: &ResumeTarget) -> Result<(PathBuf, PathBuf), ResumeError> {
    let source = Path::new(&target.source_path);
    if target.session_id.starts_with('-') || !is_plain_arg(&target.source_path) {
        return Err(ResumeError::InvalidSession);
    }
    let dir = session_dir_from_source(&home.join("sessions"), source, &target.session_id)
        .map_err(|_| ResumeError::InvalidSession)?;
    let metadata: Value = [dir.join("state.json"), dir.join("session-meta/state.json")]
        .into_iter()
        .find_map(|candidate| {
            if !fs::symlink_metadata(&candidate).ok()?.is_file()
                || candidate.canonicalize().ok()? != candidate
            {
                return None;
            }
            serde_json::from_slice(&fs::read(candidate).ok()?).ok()
        })
        .ok_or(ResumeError::InvalidSession)?;
    if !metadata.is_object()
        || metadata
            .get("id")
            .is_some_and(|id| id.as_str() != Some(&target.session_id))
        || metadata["archived"].as_bool() == Some(true)
        || metadata
            .pointer("/custom/session_kind")
            .and_then(Value::as_str)
            == Some("child")
    {
        return Err(ResumeError::InvalidSession);
    }
    let cwd = metadata
        .get("cwd")
        .or_else(|| metadata.get("workDir"))
        .or_else(|| metadata.pointer("/custom/cwd"))
        .and_then(text)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_dir())
        .ok_or(ResumeError::MissingProject)?;
    let home = home
        .canonicalize()
        .map_err(|_| ResumeError::InvalidConfig)?;
    if !home.to_str().is_some_and(is_plain_arg) {
        return Err(ResumeError::InvalidConfig);
    }
    Ok((home, cwd))
}

fn configured_models(
    document: &Value,
    live: &AppLiveState,
    proxy_base: Option<&str>,
) -> Result<ResumeOptions, ResumeError> {
    if live.pending.is_some() {
        return Err(ResumeError::ProxyNotReady);
    }
    let is_proxy = live.mode_state().is_proxy();
    let default = document.get("default_model").and_then(text);
    let proxy = &document["providers"][RESERVED_PROXY_TABLE];
    if is_proxy {
        if !live.attached
            || live.proxy_route.as_deref().is_none_or(str::is_empty)
            || proxy_base.is_none()
            || proxy["base_url"].as_str().map(|s| s.trim_end_matches('/')) != proxy_base
            || proxy["type"].as_str() != Some("anthropic")
            || proxy["api_key"].as_str() != Some(PROXY_PLACEHOLDER)
            || default.is_some_and(|alias| {
                document["models"]
                    .get(alias)
                    .is_some_and(|model| model["provider"].as_str() != Some(RESERVED_PROXY_TABLE))
            })
        {
            return Err(ResumeError::ProxyNotReady);
        }
    } else if live.attached || !proxy.is_null() {
        return Err(ResumeError::ProxyNotReady);
    }
    let mode = if !is_proxy {
        ResumeMode::Direct
    } else if live.stack.enabled {
        ResumeMode::Stack
    } else {
        ResumeMode::Route
    };
    let proxy_prefix = format!("{RESERVED_PROXY_TABLE}/");
    let models = document["models"].as_object().into_iter().flatten();
    let mut models = models
        .filter_map(|(alias, model)| {
            if !is_plain_arg(alias) || (is_proxy && !alias.starts_with(&proxy_prefix)) {
                return None;
            }
            let provider_id = model.get("provider").and_then(text)?;
            if (provider_id == RESERVED_PROXY_TABLE) != is_proxy {
                return None;
            }
            let provider = document["providers"].get(provider_id)?;
            provider.get("type").and_then(text)?;
            if !is_proxy && provider["api_key"].as_str() == Some(PROXY_PLACEHOLDER) {
                return None;
            }
            let model_id = model.get("model").and_then(text)?;
            let display_name = model.get("display_name").and_then(text).map(str::to_string);
            let provider_label = match mode {
                ResumeMode::Direct => provider.get("name").and_then(text).unwrap_or(provider_id),
                ResumeMode::Route => live.proxy_route.as_deref().unwrap_or(provider_id),
                ResumeMode::Stack => provider.get("name").and_then(text).unwrap_or(provider_id),
            };
            // Bind the click to this routing choice, without returning or hashing API keys.
            let revision = format!(
                "{:x}",
                Sha256::digest(
                    json!([
                        mode,
                        alias,
                        provider_id,
                        model_id,
                        model.get("protocol"),
                        provider.get("type"),
                        provider.get("base_url"),
                        provider.pointer("/env/KIMI_BASE_URL"),
                        provider.pointer("/env/OPENAI_BASE_URL"),
                        provider.pointer("/env/ANTHROPIC_BASE_URL"),
                        provider.pointer("/oauth/key"),
                        provider.pointer("/oauth/oauth_host"),
                        provider.get("api_key_env"),
                        live.proxy_route,
                        live.contract.as_ref().map(|contract| &contract.key),
                    ])
                    .to_string()
                )
            );
            Some(ResumeModel {
                alias: alias.clone(),
                model: model_id.into(),
                provider: provider_label.into(),
                display_name,
                is_default: default == Some(alias.as_str()),
                revision,
            })
        })
        .collect::<Vec<_>>();
    models.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| a.alias.cmp(&b.alias))
    });
    Ok(ResumeOptions { models, mode })
}

pub(crate) async fn prepare(
    state: &AppState,
    target: ResumeTarget,
) -> Result<ResumeContext, ResumeError> {
    let app = AppType::Kimi.as_str();
    let _guard = state.proxy_service.lock_switch_for_app(app).await;
    let (home, cwd, session_id, document, live) = tauri::async_runtime::spawn_blocking(move || {
        let home = kimi_config::data_dir();
        let (home, cwd) = read_target(&home, &target)?;
        let live = state::load_read_only(&DeviceStore::for_device())
            .map_err(|_| ResumeError::ProxyNotReady)?
            .apps
            .remove(app)
            .unwrap_or_default();
        let document =
            kimi_config::read(&home.join("config.toml")).map_err(|_| ResumeError::InvalidConfig)?;
        Ok((home, cwd, target.session_id, document, live))
    })
    .await
    .map_err(|_| ResumeError::Internal)??;
    let proxy_base = if live.mode_state().is_proxy() && live.attached && live.pending.is_none() {
        let status = state
            .proxy_service
            .get_status()
            .await
            .map_err(|_| ResumeError::ProxyNotReady)?;
        if !status.running {
            return Err(ResumeError::ProxyNotReady);
        }
        let (origin, _) = state
            .proxy_service
            .build_proxy_urls()
            .await
            .map_err(|_| ResumeError::ProxyNotReady)?;
        Some(format!("{}/kimicode", origin.trim_end_matches('/')))
    } else {
        None
    };
    let mut options = configured_models(&document, &live, proxy_base.as_deref())?;
    if options.mode == ResumeMode::Route {
        if let Some(route) = live.proxy_route.as_deref().and_then(|id| {
            state
                .db
                .get_provider_by_id(id, &AppType::Kimi)
                .ok()
                .flatten()
        }) {
            for model in &mut options.models {
                model.provider = route.name.clone();
            }
        }
    }
    Ok(ResumeContext {
        home,
        cwd,
        session_id,
        options,
    })
}

impl ResumeContext {
    fn selected_model(&self, selection: &ModelSelection) -> Result<&ResumeModel, ResumeError> {
        self.options
            .models
            .iter()
            .find(|model| model.alias == selection.alias && model.revision == selection.revision)
            .ok_or(ResumeError::ModelChanged)
    }

    pub(crate) fn execute(
        self,
        selection: ModelSelection,
        kind: ResumeKind,
        action: ResumeAction,
        terminal: &str,
    ) -> Result<ResumeOutcome, ResumeError> {
        let model = self.selected_model(&selection)?;
        let id = matches!(kind, ResumeKind::Original).then_some(self.session_id.as_str());
        let home = self.home.to_str().ok_or(ResumeError::InvalidConfig)?;
        let cwd = self.cwd.to_str().ok_or(ResumeError::MissingProject)?;
        match action {
            ResumeAction::Open => {
                if !cfg!(target_os = "macos") {
                    return Err(ResumeError::TerminalFailed);
                }
                let command = posix_command(home, id, &model.alias);
                crate::session_manager::terminal::launch_terminal(
                    terminal,
                    &command,
                    Some(cwd),
                    None,
                )
                .map_err(|_| ResumeError::TerminalFailed)?;
                Ok(ResumeOutcome::Opened)
            }
            ResumeAction::Copy => {
                #[cfg(windows)]
                let command = powershell_copy_command(home, cwd, id, &model.alias);
                #[cfg(not(windows))]
                let command = format!(
                    "cd {} && {}",
                    shell_escape(cwd),
                    posix_command(home, id, &model.alias)
                );
                Ok(ResumeOutcome::Command { command })
            }
        }
    }
}

fn posix_command(home: &str, session_id: Option<&str>, alias: &str) -> String {
    let env = shell_escape(home);
    let model = shell_escape(alias);
    let session = session_id
        .map(|id| format!(" --session {}", shell_escape(id)))
        .unwrap_or_default();
    format!("env KIMI_CODE_HOME={env} kimi{session} --model={model}")
}

#[cfg(any(windows, test))]
fn powershell_copy_command(home: &str, cwd: &str, session_id: Option<&str>, alias: &str) -> String {
    fn quote(value: &str) -> String {
        let mut quoted = String::from("'");
        for ch in value.chars() {
            if matches!(ch, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
                quoted.push(ch);
            }
            quoted.push(ch);
        }
        quoted.push('\'');
        quoted
    }
    let home = quote(home);
    let cwd = quote(cwd);
    let model = quote(&format!("--model={alias}"));
    let session = session_id
        .map(|id| format!(" --session {}", quote(id)))
        .unwrap_or_default();
    format!("& {{ Push-Location -LiteralPath {cwd} -ErrorAction Stop; $oldHome = $env:KIMI_CODE_HOME; try {{ $env:KIMI_CODE_HOME = {home}; & kimi{session} {model} }} finally {{ $env:KIMI_CODE_HOME = $oldHome; Pop-Location }} }}")
}
