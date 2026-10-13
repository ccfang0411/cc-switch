//! Kimi Code CLI and desktop share this file. CC Switch providers live under the
//! `cc-switch:` namespace; switching means pointing `default_model` at one of them.
//! Entries prefixed `managed:` are written by Kimi Code's own login flow and are
//! never listed, modified, or deleted here.
use crate::config::{atomic_write_private, get_home_dir};
use crate::error::AppError;
use indexmap::IndexMap;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use toml_edit::{DocumentMut, Item, Table};

const CC_PREFIX: &str = "cc-switch:";
/// Reserved provider id: the routing/stack mode contract lives in
/// `[providers."cc-switch:proxy"]`. It is a mode artifact, never a provider: listings
/// skip it and user providers cannot take the key.
pub(crate) const RESERVED_PROXY_KEY: &str = "proxy";
pub(crate) const RESERVED_PROXY_TABLE: &str = "cc-switch:proxy";
/// The placeholder API key the proxy contract carries (same marker as the other apps).
pub(crate) const PROXY_PLACEHOLDER: &str = "PROXY_MANAGED";
const PROVIDER_TYPES: [&str; 6] = [
    "kimi",
    "anthropic",
    "openai",
    "openai_responses",
    "google-genai",
    "vertexai",
];
/// Fields CC Switch manages inside a `[providers."cc-switch:<key>"]` table. Unknown
/// keys in our namespace survive rewrites; other namespaces are never touched.
pub(crate) const PROVIDER_FIELDS: [&str; 6] = [
    "type",
    "base_url",
    "api_key",
    "api_key_env",
    "env",
    "custom_headers",
];
/// Fields CC Switch manages inside a `[models."cc-switch:<key>/<alias>"]` table;
/// `provider` is always rewritten to the owning provider key.
pub(crate) const MODEL_FIELDS: [&str; 8] = [
    "model",
    "max_context_size",
    "capabilities",
    "display_name",
    "max_output_size",
    "support_efforts",
    "default_effort",
    "provider",
];

pub(crate) fn data_dir() -> PathBuf {
    if let Some(custom) = crate::settings::get_kimicode_override_dir() {
        return custom;
    }
    explicit_data_dir(std::env::var("KIMI_CODE_HOME").ok().as_deref())
        .unwrap_or_else(|| get_home_dir().join(".kimi-code"))
}

fn explicit_data_dir(kimi_code_home: Option<&str>) -> Option<PathBuf> {
    let path = kimi_code_home?.trim();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

// Callers hold their feature lock across the native write and database commit.
pub(crate) fn write_and_commit<T>(
    path: &Path,
    write: impl FnOnce() -> Result<(), AppError>,
    commit: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    let previous = match fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(AppError::io(path, error)),
    };
    write()?;
    match commit() {
        Ok(result) => Ok(result),
        Err(error) => {
            let rollback = match previous {
                Some(bytes) => atomic_write_private(path, &bytes),
                None => fs::remove_file(path).map_err(|error| AppError::io(path, error)),
            };
            if let Err(rollback_error) = rollback {
                return Err(AppError::Message(format!(
                    "Config update failed ({error}); restoring {} also failed: {rollback_error}",
                    path.display()
                )));
            }
            Err(error)
        }
    }
}

pub(crate) fn config_path() -> PathBuf {
    data_dir().join("config.toml")
}

fn read(path: &Path) -> Result<DocumentMut, AppError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(AppError::Message(format!(
                "Cannot read Kimi Code configuration: {e}"
            )))
        }
    };
    if text.trim().is_empty() {
        return Ok(DocumentMut::new());
    }
    text.parse::<DocumentMut>()
        .map_err(|_| AppError::Config("Invalid Kimi Code TOML configuration".into()))
}

fn toml_to_json(item: &Item) -> Value {
    match item {
        Item::None => Value::Null,
        Item::Value(value) => match value {
            toml_edit::Value::String(s) => Value::String(s.value().clone()),
            toml_edit::Value::Integer(i) => Value::from(*i.value()),
            toml_edit::Value::Float(f) => Value::from(*f.value()),
            toml_edit::Value::Boolean(b) => Value::from(*b.value()),
            toml_edit::Value::Datetime(dt) => Value::String(dt.value().to_string()),
            toml_edit::Value::Array(array) => Value::Array(
                array
                    .iter()
                    .map(|value| toml_to_json(&Item::Value(value.clone())))
                    .collect(),
            ),
            toml_edit::Value::InlineTable(table) => Value::Object(
                table
                    .iter()
                    .map(|(key, value)| {
                        (key.to_string(), toml_to_json(&Item::Value(value.clone())))
                    })
                    .collect(),
            ),
        },
        Item::Table(table) => Value::Object(
            table
                .iter()
                .map(|(key, item)| (key.to_string(), toml_to_json(item)))
                .collect(),
        ),
        Item::ArrayOfTables(_) => Value::Null,
    }
}

/// Null and objects with nested structure that TOML cannot inline are skipped by
/// returning `None`; callers treat them as absent.
pub(crate) fn json_to_toml(value: &Value) -> Option<toml_edit::Value> {
    match value {
        Value::Null => None,
        Value::Bool(b) => Some(toml_edit::Value::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(toml_edit::Value::from(i))
            } else {
                n.as_f64().map(toml_edit::Value::from)
            }
        }
        Value::String(s) => Some(toml_edit::Value::from(s.clone())),
        Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(json_to_toml(item)?);
            }
            Some(toml_edit::Value::Array(array))
        }
        Value::Object(map) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, value) in map {
                table.insert(key, json_to_toml(value)?);
            }
            Some(toml_edit::Value::InlineTable(table))
        }
    }
}

fn table_mut<'a>(item: &'a mut Item, what: &str) -> Result<&'a mut Table, AppError> {
    item.as_table_mut()
        .ok_or_else(|| AppError::Config(format!("Invalid Kimi Code {what} configuration")))
}

/// All `cc-switch:` providers, keyed by their bare id, with their models folded in.
pub(crate) fn get_providers() -> Result<IndexMap<String, Value>, AppError> {
    let document = read(&config_path())?;
    let mut providers = IndexMap::new();
    let Some(table) = document.get("providers") else {
        return Ok(providers);
    };
    let table = table
        .as_table()
        .ok_or_else(|| AppError::Config("Invalid Kimi Code providers configuration".into()))?;
    for (key, item) in table.iter() {
        let Some(id) = key.strip_prefix(CC_PREFIX) else {
            continue;
        };
        // The reserved proxy table is a routing/stack-mode artifact, not a provider.
        if id == RESERVED_PROXY_KEY {
            continue;
        }
        let Some(provider) = item.as_table() else {
            continue;
        };
        providers.insert(id.to_string(), provider_to_json(&document, key, provider));
    }
    Ok(providers)
}

fn provider_to_json(document: &DocumentMut, key: &str, provider: &Table) -> Value {
    let mut config = serde_json::Map::new();
    for (field, item) in provider.iter() {
        config.insert(field.to_string(), toml_to_json(item));
    }
    if let Some(all) = document.get("models").and_then(Item::as_table) {
        let prefix = format!("{key}/");
        let selected = document.get("default_model").and_then(Item::as_str);
        let mut models = Vec::new();
        let mut default = None;
        for (model_key, item) in all.iter() {
            let Some(alias) = model_key.strip_prefix(&prefix) else {
                continue;
            };
            let Some(table) = item.as_table() else {
                continue;
            };
            let mut fields = serde_json::Map::new();
            for (field, item) in table.iter() {
                if field != "provider" {
                    fields.insert(field.to_string(), toml_to_json(item));
                }
            }
            if fields.get("model").and_then(Value::as_str) != Some(alias) {
                fields.insert("alias".into(), Value::String(alias.to_string()));
            }
            let model = Value::Object(fields);
            if default.is_none() || Some(model_key) == selected {
                default = Some(model.clone());
            }
            models.push(model);
        }
        if let Some(model) = default {
            config.insert("model".into(), model);
        }
        if models.len() > 1 {
            config.insert("models".into(), Value::Array(models));
        }
    }
    Value::Object(config)
}

/// Any `[providers]` node under this key, including hand-written `cc-switch:` entries.
/// The reserved proxy key never counts: it is written by routing/stack mode, and
/// `validate_provider` keeps user providers from taking it.
pub(crate) fn provider_key_exists(id: &str) -> Result<bool, AppError> {
    if id == RESERVED_PROXY_KEY {
        return Ok(false);
    }
    let document = read(&config_path())?;
    Ok(document
        .get("providers")
        .and_then(Item::as_table)
        .is_some_and(|providers| providers.contains_key(&format!("{CC_PREFIX}{id}"))))
}

/// The provider the live `default_model` points at (`cc-switch:<id>/...`); the reserved
/// proxy pointer and unmanaged (`managed:` etc.) selections yield `None`. Fallback for
/// the direct-mode pointer when neither the device settings nor the DB have one.
pub(crate) fn default_provider_id() -> Option<String> {
    let document = read(&config_path()).ok()?;
    document
        .get("default_model")
        .and_then(Item::as_str)
        .and_then(|selected| selected.strip_prefix(CC_PREFIX))
        .and_then(|rest| rest.split('/').next())
        .filter(|id| !id.is_empty() && *id != RESERVED_PROXY_KEY)
        .map(str::to_string)
}

/// The live config currently carries the routing/stack-mode proxy contract.
pub(crate) fn live_has_proxy_placeholder() -> bool {
    read(&config_path())
        .ok()
        .and_then(|document| {
            document
                .get("providers")
                .and_then(Item::as_table)
                .and_then(|providers| providers.get(RESERVED_PROXY_TABLE))
                .and_then(Item::as_table)
                .and_then(|proxy| proxy.get("api_key"))
                .and_then(Item::as_str)
                .map(str::to_string)
        })
        .as_deref()
        == Some(PROXY_PLACEHOLDER)
}

/// The Kimi Code alias for the managed model: the explicit `alias` when present
/// (required when the upstream model id contains '/'), else the model id itself.
pub(crate) fn model_alias(model: &Value) -> Option<&str> {
    model
        .get("alias")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|alias| !alias.is_empty())
        .or_else(|| {
            model
                .get("model")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
        })
}

/// The singular model remains the default for backwards compatibility.
pub(crate) fn provider_models(config: &Value) -> Vec<&Value> {
    let mut result = Vec::new();
    if let Some(model) = config.get("model") {
        result.push(model);
    }
    if let Some(models) = config.get("models").and_then(Value::as_array) {
        for model in models {
            if !result.iter().any(|m| model_alias(m) == model_alias(model)) {
                result.push(model);
            }
        }
    }
    result
}

fn validate_model(model: &Value) -> Result<(), AppError> {
    if model
        .get("model")
        .and_then(Value::as_str)
        .is_none_or(|id| id.trim().is_empty())
    {
        return Err(AppError::InvalidInput("Enter a Kimi Code model id".into()));
    }
    if model_alias(model).is_none_or(|alias| alias.contains('/')) {
        return Err(AppError::InvalidInput(
            "A model id containing '/' needs an explicit alias without '/'".into(),
        ));
    }
    if model
        .get("max_context_size")
        .and_then(Value::as_u64)
        .is_none_or(|n| n == 0)
    {
        return Err(AppError::InvalidInput(
            "The Kimi Code model needs a positive max_context_size".into(),
        ));
    }
    if let Some(effort) = model.get("default_effort").and_then(Value::as_str) {
        if let Some(levels) = model.get("support_efforts").and_then(Value::as_array) {
            if !levels.iter().any(|level| level.as_str() == Some(effort)) {
                return Err(AppError::InvalidInput(
                    "The default reasoning effort must be supported by this model".into(),
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_provider(id: &str, config: &Value) -> Result<(), AppError> {
    if id == RESERVED_PROXY_KEY {
        return Err(AppError::InvalidInput(format!(
            "Kimi Code provider key '{id}' is reserved"
        )));
    }
    if id.is_empty()
        || id
            .chars()
            .any(|c| !c.is_ascii_alphanumeric() && !matches!(c, '-' | '_'))
    {
        return Err(AppError::InvalidInput(
            "Kimi Code provider key must contain letters, digits, '-' or '_'".into(),
        ));
    }
    if !PROVIDER_TYPES.contains(
        &config
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    ) {
        return Err(AppError::InvalidInput(
            "Select a supported Kimi Code provider type".into(),
        ));
    }
    let base_url = config
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !url::Url::parse(base_url)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
    {
        return Err(AppError::InvalidInput(
            "Enter a valid Kimi Code endpoint URL".into(),
        ));
    }
    let api_key = config
        .get("api_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty());
    let api_key_env = config
        .get("api_key_env")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty());
    if api_key.is_some() == api_key_env.is_some() {
        return Err(AppError::InvalidInput(
            "Set exactly one of api_key and api_key_env".into(),
        ));
    }
    let default = config
        .get("model")
        .ok_or_else(|| AppError::InvalidInput("Enter a Kimi Code model id".into()))?;
    validate_model(default)?;
    if let Some(value) = config.get("models") {
        let models = value
            .as_array()
            .ok_or_else(|| AppError::InvalidInput("Kimi Code models must be a list".into()))?;
        let mut aliases = std::collections::HashSet::new();
        for model in models {
            validate_model(model)?;
            if !aliases.insert(model_alias(model).unwrap_or_default()) {
                return Err(AppError::InvalidInput(
                    "Duplicate Kimi Code model alias".into(),
                ));
            }
        }
    }
    Ok(())
}

// Same atomic directory lock protocol the other additive apps use for native files.
struct ConfigLock(PathBuf);
impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

fn update(path: &Path, id: &str, provider: Option<Value>) -> Result<(), AppError> {
    write_entry(path, id, provider, false)
}

/// `create_only` rejects a key that already exists, checked while holding the lock.
fn write_entry(
    path: &Path,
    id: &str,
    provider: Option<Value>,
    create_only: bool,
) -> Result<(), AppError> {
    fs::create_dir_all(path.parent().expect("Kimi Code configuration directory"))
        .map_err(|e| AppError::Message(format!("Cannot create Kimi Code directory: {e}")))?;
    let path = if path.exists() {
        fs::canonicalize(path).map_err(|e| AppError::Message(e.to_string()))?
    } else {
        path.to_path_buf()
    };
    let lock_path = PathBuf::from(format!("{}.lock", path.display()));
    if let Err(error) = fs::create_dir(&lock_path) {
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(AppError::io(&lock_path, error));
        }
        let stale = fs::metadata(&lock_path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|mtime| mtime.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(10));
        if !stale {
            return Err(AppError::Conflict(
                "Kimi Code configuration is busy; retry after Kimi Code finishes saving".into(),
            ));
        }
        fs::remove_dir(&lock_path).map_err(|e| AppError::io(&lock_path, e))?;
        fs::create_dir(&lock_path).map_err(|e| AppError::io(&lock_path, e))?;
    }
    let _lock = ConfigLock(lock_path);
    let mut document = read(&path)?;
    let provider_key = format!("{CC_PREFIX}{id}");
    if let Some(alias) = document
        .get("default_model")
        .and_then(Item::as_str)
        .and_then(|selected| selected.strip_prefix(&format!("{provider_key}/")))
    {
        if !provider.as_ref().is_some_and(|config| {
            provider_models(config)
                .iter()
                .any(|model| model_alias(model) == Some(alias))
        }) {
            return Err(AppError::InvalidInput(
                "Select another default model in Kimi Code before removing this model or provider"
                    .into(),
            ));
        }
    }
    let providers = table_mut(
        document
            .entry("providers")
            .or_insert_with(|| Item::Table(Table::new())),
        "providers",
    )?;
    if create_only && providers.contains_key(&provider_key) {
        return Err(AppError::InvalidInput(format!(
            "Kimi Code provider key '{id}' already exists"
        )));
    }
    match provider {
        Some(config) => {
            let table = table_mut(
                providers
                    .entry(&provider_key)
                    .or_insert_with(|| Item::Table(Table::new())),
                "providers",
            )?;
            for field in PROVIDER_FIELDS {
                match config.get(field).and_then(json_to_toml) {
                    Some(value) => table.insert(field, Item::Value(value)),
                    None => table.remove(field),
                };
            }
            write_models(&mut document, &provider_key, Some(&config))?;
            // Switching means pointing `default_model` at this provider's model.
            if let Some(alias) = config.get("model").and_then(model_alias) {
                document["default_model"] = toml_edit::value(format!("{provider_key}/{alias}"));
            }
        }
        None => {
            providers.remove(&provider_key);
            write_models(&mut document, &provider_key, None)?;
        }
    }
    atomic_write_private(&path, document.to_string().as_bytes())
}

/// Project the provider's `model` object onto `[models."cc-switch:<key>/<alias>"]`,
/// dropping stale aliases of the same provider. `None` removes them all.
fn write_models(
    document: &mut DocumentMut,
    provider_key: &str,
    config: Option<&Value>,
) -> Result<(), AppError> {
    let models = config.map(provider_models).unwrap_or_default();
    let prefix = format!("{provider_key}/");
    let desired: Vec<String> = models
        .iter()
        .filter_map(|m| model_alias(m))
        .map(|alias| format!("{provider_key}/{alias}"))
        .collect();
    let stale: Vec<String> = document
        .get("models")
        .and_then(Item::as_table)
        .map(|table| {
            table
                .iter()
                .map(|(key, _)| key.to_string())
                .filter(|key| key.starts_with(&prefix) && !desired.contains(key))
                .collect()
        })
        .unwrap_or_default();
    if stale.is_empty() && models.is_empty() {
        return Ok(());
    }
    let all = table_mut(
        document
            .entry("models")
            .or_insert_with(|| Item::Table(Table::new())),
        "models",
    )?;
    for key in stale {
        all.remove(&key);
    }
    for model in models {
        let Some(alias) = model_alias(model) else {
            continue;
        };
        let model_key = format!("{provider_key}/{alias}");
        let table = table_mut(
            all.entry(&model_key)
                .or_insert_with(|| Item::Table(Table::new())),
            "models",
        )?;
        for field in MODEL_FIELDS {
            let value = if field == "provider" {
                Some(Value::String(provider_key.to_string()))
            } else {
                model.get(field).cloned()
            };
            match value.as_ref().and_then(json_to_toml) {
                Some(value) => table.insert(field, Item::Value(value)),
                None => table.remove(field),
            };
        }
    }
    Ok(())
}

pub(crate) fn set_provider(id: &str, config: Value) -> Result<(), AppError> {
    validate_provider(id, &config)?;
    update(&config_path(), id, Some(config))
}

pub(crate) fn add_provider(id: &str, config: Value) -> Result<(), AppError> {
    validate_provider(id, &config)?;
    write_entry(&config_path(), id, Some(config), true)
}

pub(crate) fn remove_provider(id: &str) -> Result<(), AppError> {
    if id == RESERVED_PROXY_KEY {
        return Err(AppError::InvalidInput(
            "The proxy provider is reserved".into(),
        ));
    }
    update(&config_path(), id, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn provider() -> Value {
        json!({
            "type": "kimi",
            "base_url": "https://api.kimi.com/coding/v1",
            "api_key": "test-key",
            "model": {"model": "k3", "max_context_size": 1048576, "display_name": "K3"},
        })
    }

    #[test]
    fn kimi_multiple_models_round_trip_and_switch_default_without_losing_members() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let mut config = provider();
        config["model"]["support_efforts"] = json!(["low", "ultra"]);
        config["model"]["default_effort"] = json!("ultra");
        let extra = json!({"model":"other", "max_context_size":123456, "support_efforts":["low","high"], "default_effort":"high"});
        config["models"] = json!([config["model"].clone(), extra.clone()]);
        validate_provider("a", &config).unwrap();
        update(&path, "a", Some(config.clone())).unwrap();
        let doc = read(&path).unwrap();
        let loaded = provider_to_json(
            &doc,
            "cc-switch:a",
            doc["providers"]["cc-switch:a"].as_table().unwrap(),
        );
        assert_eq!(loaded["models"].as_array().unwrap().len(), 2);
        assert_eq!(loaded["model"]["default_effort"], "ultra");
        config["model"] = extra;
        update(&path, "a", Some(config)).unwrap();
        let doc = read(&path).unwrap();
        assert_eq!(doc["default_model"].as_str(), Some("cc-switch:a/other"));
        assert!(doc["models"].get("cc-switch:a/k3").is_some());
    }

    #[test]
    fn native_data_directory_honors_blank_override() {
        assert_eq!(explicit_data_dir(Some(" /data ")), Some("/data".into()));
        assert_eq!(explicit_data_dir(Some(" \t")), None);
        assert_eq!(explicit_data_dir(None), None);
    }

    #[test]
    #[cfg(unix)]
    fn reclaims_a_stale_native_lock_but_preserves_an_active_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let lock = path.with_extension("toml.lock");
        fs::create_dir(&lock).unwrap();
        assert!(update(&path, "test", Some(provider())).is_err());
        assert!(lock.exists());
        fs::File::open(&lock)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(11))
            .unwrap();
        update(&path, "test", Some(provider())).unwrap();
        assert!(!lock.exists());
        let document = read(&path).unwrap();
        assert_eq!(
            document["providers"]["cc-switch:test"]["type"].as_str(),
            Some("kimi")
        );
    }

    #[test]
    fn create_only_writes_never_replace_a_node_that_appeared_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "[providers.\"cc-switch:raced\"]\ntype = \"kimi\"\nbase_url = \"https://theirs.example.com\"\napi_key = \"theirs\"\n";
        fs::write(&path, text).unwrap();
        assert!(write_entry(&path, "raced", Some(provider()), true).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        assert!(!path.with_extension("toml.lock").exists());
        write_entry(&path, "fresh", Some(provider()), true).unwrap();
        let document = read(&path).unwrap();
        assert_eq!(
            document["providers"]["cc-switch:fresh"]["api_key"].as_str(),
            Some("test-key")
        );
    }

    #[test]
    fn additive_changes_preserve_managed_and_unrelated_entries_and_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "# Kimi Code configuration\ndefault_model = \"managed:kimi-code/k3\"\n\n# OAuth login, do not touch\n[providers.\"managed:kimi-code\"]\ntype = \"kimi\"\nbase_url = \"https://api.kimi.com/coding/v1\"\napi_key = \"\"\n\n[providers.\"mine\"]\ntype = \"openai\"\nbase_url = \"https://api.example.com/v1\"\napi_key = \"keep\"\n\n[models.\"managed:kimi-code/k3\"]\nprovider = \"managed:kimi-code\"\nmodel = \"k3\"\nmax_context_size = 1048576\n\n[models.\"mine/gpt\"]\nprovider = \"mine\"\nmodel = \"gpt-5\"\nmax_context_size = 400000\n";
        fs::write(&path, text).unwrap();
        update(&path, "official", Some(provider())).unwrap();
        let written = fs::read_to_string(&path).unwrap();
        for fragment in [
            "# Kimi Code configuration",
            "# OAuth login, do not touch",
            "[providers.\"managed:kimi-code\"]",
            "[providers.\"mine\"]",
            "provider = \"managed:kimi-code\"",
            "provider = \"mine\"",
        ] {
            assert!(written.contains(fragment), "missing {fragment}");
        }
        let document = read(&path).unwrap();
        assert_eq!(
            document["default_model"].as_str(),
            Some("cc-switch:official/k3")
        );
        assert_eq!(
            document["providers"]["cc-switch:official"]["base_url"].as_str(),
            Some("https://api.kimi.com/coding/v1")
        );
        assert_eq!(
            document["models"]["cc-switch:official/k3"]["provider"].as_str(),
            Some("cc-switch:official")
        );
        assert_eq!(
            document["models"]["cc-switch:official/k3"]["max_context_size"].as_integer(),
            Some(1048576)
        );
        // Removing our entry restores the managed namespace untouched. Repoint
        // default_model first: removing the owner of the live default is refused.
        fs::write(&path, &written).unwrap();
        let mut document = read(&path).unwrap();
        document["default_model"] = toml_edit::value("managed:kimi-code/k3");
        atomic_write_private(&path, document.to_string().as_bytes()).unwrap();
        update(&path, "official", None).unwrap();
        let document = read(&path).unwrap();
        assert!(document["providers"].get("cc-switch:official").is_none());
        assert!(document["models"].get("cc-switch:official/k3").is_none());
        assert!(document["providers"].get("managed:kimi-code").is_some());
        assert!(document["models"].get("managed:kimi-code/k3").is_some());
    }

    #[test]
    fn managed_providers_are_never_listed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[providers.\"managed:kimi-code\"]\ntype = \"kimi\"\nbase_url = \"https://api.kimi.com/coding/v1\"\napi_key = \"\"\n[providers.\"cc-switch:ours\"]\ntype = \"kimi\"\nbase_url = \"https://api.kimi.com/coding/v1\"\napi_key = \"key\"\n\n[models.\"managed:kimi-code/k3\"]\nprovider = \"managed:kimi-code\"\nmodel = \"k3\"\nmax_context_size = 1048576\n[models.\"cc-switch:ours/k3\"]\nprovider = \"cc-switch:ours\"\nmodel = \"k3\"\nmax_context_size = 1048576\n",
        )
        .unwrap();
        let document = read(&path).unwrap();
        let providers = document["providers"].as_table().unwrap();
        assert!(providers.contains_key("managed:kimi-code"));
        assert!(providers.contains_key("cc-switch:ours"));
        let listed: Vec<String> = providers
            .iter()
            .filter_map(|(key, _)| key.strip_prefix(CC_PREFIX).map(str::to_string))
            .collect();
        assert_eq!(listed, vec!["ours"]);
        let json = provider_to_json(
            &document,
            "cc-switch:ours",
            providers.get("cc-switch:ours").unwrap().as_table().unwrap(),
        );
        assert_eq!(json["api_key"], "key");
        assert_eq!(json["model"]["model"], "k3");
        assert_eq!(json["model"]["max_context_size"], 1048576);
    }

    #[test]
    fn default_model_switch_and_orphan_guard() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        update(&path, "official", Some(provider())).unwrap();
        assert_eq!(
            read(&path).unwrap()["default_model"].as_str(),
            Some("cc-switch:official/k3")
        );
        // Editing while keeping the same model id leaves the selection alone.
        let mut renamed = provider();
        renamed["model"]["display_name"] = json!("K3 (updated)");
        update(&path, "official", Some(renamed)).unwrap();
        let document = read(&path).unwrap();
        assert_eq!(
            document["default_model"].as_str(),
            Some("cc-switch:official/k3")
        );
        assert_eq!(
            document["models"]["cc-switch:official/k3"]["display_name"].as_str(),
            Some("K3 (updated)")
        );
        // Renaming the model would orphan the live default_model: refused.
        let mut other_model = provider();
        other_model["model"]["model"] = json!("k4");
        update(&path, "official", Some(other_model.clone())).unwrap_err();
        assert!(read(&path).unwrap()["models"]
            .get("cc-switch:official/k4")
            .is_none());
        // Removing the provider while it owns default_model is refused either way.
        update(&path, "official", None).unwrap_err();
        let mut document = read(&path).unwrap();
        document["default_model"] = toml_edit::value("managed:kimi-code/k3");
        atomic_write_private(&path, document.to_string().as_bytes()).unwrap();
        // With the default elsewhere, the rename goes through and drops the stale alias.
        update(&path, "official", Some(other_model)).unwrap();
        let mut document = read(&path).unwrap();
        assert_eq!(
            document["default_model"].as_str(),
            Some("cc-switch:official/k4")
        );
        assert!(document["models"].get("cc-switch:official/k3").is_none());
        document["default_model"] = toml_edit::value("managed:kimi-code/k3");
        atomic_write_private(&path, document.to_string().as_bytes()).unwrap();
        update(&path, "official", None).unwrap();
        assert!(read(&path).unwrap()["providers"]
            .get("cc-switch:official")
            .is_none());
    }

    #[test]
    fn provider_validation() {
        validate_provider("official", &provider()).unwrap();
        for id in ["", "has space", "has:colon", "中文"] {
            assert!(validate_provider(id, &provider()).is_err(), "{id}");
        }
        for (field, value) in [
            ("type", json!("unsupported")),
            ("base_url", json!("not a url")),
            ("base_url", json!("ftp://example.com")),
        ] {
            let mut broken = provider();
            broken[field] = value;
            assert!(validate_provider("official", &broken).is_err(), "{field}");
        }
        let mut both = provider();
        both["api_key_env"] = json!("KIMI_API_KEY");
        assert!(validate_provider("official", &both).is_err());
        let mut neither = provider();
        neither.as_object_mut().unwrap().remove("api_key");
        assert!(validate_provider("official", &neither).is_err());
        let mut env_only = neither.clone();
        env_only["api_key_env"] = json!("KIMI_API_KEY");
        validate_provider("official", &env_only).unwrap();
        let mut no_model = provider();
        no_model.as_object_mut().unwrap().remove("model");
        assert!(validate_provider("official", &no_model).is_err());
        for bad_id in ["", "has/slash"] {
            let mut bad = provider();
            bad["model"]["model"] = json!(bad_id);
            assert!(validate_provider("official", &bad).is_err(), "{bad_id}");
        }
        let mut no_context = provider();
        no_context["model"]
            .as_object_mut()
            .unwrap()
            .remove("max_context_size");
        assert!(validate_provider("official", &no_context).is_err());
    }

    #[test]
    fn slashed_model_id_requires_and_round_trips_an_alias() {
        // Upstream ids like "deepseek/deepseek-v4.1-flash" are legal, but the
        // `[models."<provider>/<alias>"]` key cannot contain '/'.
        let mut bad = provider();
        bad["model"] = json!({"model": "deepseek/deepseek-v4.1-flash", "max_context_size": 128000});
        assert!(validate_provider("relay", &bad).is_err());
        bad["model"]["alias"] = json!("deepseek-v4.1-flash");
        validate_provider("relay", &bad).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        update(&path, "relay", Some(bad)).unwrap();
        let document = read(&path).unwrap();
        assert_eq!(
            document["default_model"].as_str(),
            Some("cc-switch:relay/deepseek-v4.1-flash")
        );
        let model = &document["models"]["cc-switch:relay/deepseek-v4.1-flash"];
        assert_eq!(
            model["model"].as_str(),
            Some("deepseek/deepseek-v4.1-flash")
        );
        assert!(model.get("alias").is_none());
        // Read-back carries the alias so the next save targets the same table.
        let json = provider_to_json(
            &document,
            "cc-switch:relay",
            document["providers"]["cc-switch:relay"].as_table().unwrap(),
        );
        assert_eq!(json["model"]["model"], "deepseek/deepseek-v4.1-flash");
        assert_eq!(json["model"]["alias"], "deepseek-v4.1-flash");
        // Aliased model does not trip the orphan guard on save or delete.
        update(&path, "relay", Some(json)).unwrap();
        let mut document = read(&path).unwrap();
        document["default_model"] = toml_edit::value("managed:kimi-code/k3");
        atomic_write_private(&path, document.to_string().as_bytes()).unwrap();
        update(&path, "relay", None).unwrap();
        assert!(read(&path).unwrap()["providers"]
            .get("cc-switch:relay")
            .is_none());
    }

    #[test]
    fn env_sub_table_and_headers_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = provider();
        config["env"] = json!({"KIMI_LOG": "debug"});
        config["custom_headers"] = json!({"X-Tenant": "example"});
        config["model"]["capabilities"] = json!(["thinking", "image_in"]);
        update(&path, "official", Some(config)).unwrap();
        let document = read(&path).unwrap();
        assert_eq!(
            document["providers"]["cc-switch:official"]["env"]["KIMI_LOG"].as_str(),
            Some("debug")
        );
        assert_eq!(
            document["providers"]["cc-switch:official"]["custom_headers"]["X-Tenant"].as_str(),
            Some("example")
        );
        let json = provider_to_json(
            &document,
            "cc-switch:official",
            document["providers"]["cc-switch:official"]
                .as_table()
                .unwrap(),
        );
        assert_eq!(json["env"]["KIMI_LOG"], "debug");
        assert_eq!(json["custom_headers"]["X-Tenant"], "example");
        assert_eq!(json["model"]["capabilities"][0], "thinking");
        assert_eq!(json["model"]["max_context_size"], 1048576);
        assert_eq!(json["model"]["model"], "k3");
        assert!(json["model"].get("provider").is_none());
    }
}
