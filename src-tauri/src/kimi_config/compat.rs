use crate::error::AppError;
use serde_json::{json, Value};

pub(crate) const RESERVED_PROXY_TABLE: &str = "cc-switch:proxy";
pub(crate) const PROXY_PLACEHOLDER: &str = "PROXY_MANAGED";
const LEGACY_NAMESPACE: &str = "cc_switch_legacy_namespace";

pub(crate) fn stored_id(native_id: &str) -> &str {
    native_id.strip_prefix("cc-switch:").unwrap_or(native_id)
}

pub(crate) fn canonical_config(id: &str, config: &Value) -> Result<Value, AppError> {
    if config.get("provider").is_some() {
        return Ok(config.clone());
    }
    let mut result = config
        .as_object()
        .cloned()
        .ok_or_else(|| AppError::Config("Invalid Kimi Code provider configuration".into()))?;
    let mut provider = serde_json::Map::new();
    for field in [
        "type",
        "base_url",
        "api_key",
        "api_key_env",
        "env",
        "custom_headers",
        "oauth",
    ] {
        if let Some(value) = result.remove(field) {
            provider.insert(field.into(), value);
        }
    }
    if provider.is_empty() {
        return Err(AppError::Config(
            "Invalid legacy Kimi Code provider configuration".into(),
        ));
    }
    let default = result.remove("model");
    let additional = result.remove("models");
    let mut models = serde_json::Map::new();
    let mut selected = None;
    let mut entries = Vec::new();
    if let Some(model) = default {
        entries.push(model);
    }
    match additional {
        None => {}
        Some(Value::Array(additional)) => entries.extend(additional),
        Some(_) => {
            return Err(AppError::Config(
                "Legacy Kimi Code models must be an array".into(),
            ))
        }
    }
    for mut model in entries {
        let alias = model_alias(&model)
            .filter(|alias| !alias.contains('/'))
            .ok_or_else(|| {
                AppError::Config("Legacy Kimi Code model needs an unambiguous alias".into())
            })?
            .to_string();
        let key = format!("cc-switch:{id}/{alias}");
        let fields = model
            .as_object_mut()
            .ok_or_else(|| AppError::Config("Invalid Kimi Code model".into()))?;
        fields.insert("provider".into(), json!(id));
        if let Some(existing) = models.get(&key) {
            if existing != &model {
                return Err(AppError::Config(format!(
                    "Conflicting legacy Kimi Code model alias '{alias}'"
                )));
            }
            continue;
        }
        selected.get_or_insert_with(|| key.clone());
        models.insert(key, model);
    }
    result.insert("provider".into(), Value::Object(provider));
    result.insert("models".into(), Value::Object(models));
    result.insert(LEGACY_NAMESPACE.into(), json!(true));
    if let Some(selected) = selected {
        result.insert("default_model".into(), json!(selected));
    }
    Ok(Value::Object(result))
}

pub(crate) fn native_id(
    document: &Value,
    id: &str,
    config: Option<&Value>,
) -> Result<String, AppError> {
    let legacy = format!("cc-switch:{id}");
    let providers = document.get("providers");
    let plain_exists = providers.and_then(|providers| providers.get(id)).is_some();
    let legacy_exists = providers
        .and_then(|providers| providers.get(&legacy))
        .is_some();
    if plain_exists && legacy_exists {
        return Err(AppError::Config(format!(
            "Kimi Code provider '{id}' has conflicting native and legacy entries"
        )));
    }
    if legacy_exists
        || (!plain_exists
            && config
                .and_then(|config| config.get(LEGACY_NAMESPACE))
                .and_then(Value::as_bool)
                == Some(true))
    {
        Ok(legacy)
    } else {
        Ok(id.to_string())
    }
}

pub(crate) fn native_config(id: &str, native_id: &str, config: &Value) -> Result<Value, AppError> {
    let mut config = canonical_config(id, config)?;
    if let Some(models) = config.get_mut("models").and_then(Value::as_object_mut) {
        for model in models.values_mut() {
            let fields = model
                .as_object_mut()
                .ok_or_else(|| AppError::Config("Invalid Kimi Code model".into()))?;
            if fields.get("provider").is_some_and(|provider| {
                provider.as_str() != Some(id) && provider.as_str() != Some(native_id)
            }) {
                return Err(AppError::InvalidInput(
                    "Kimi Code model belongs to another provider".into(),
                ));
            }
            fields.insert("provider".into(), json!(native_id));
        }
    }
    Ok(config)
}

pub(crate) fn snapshot(document: &Value, native_id: &str) -> Value {
    let id = stored_id(native_id);
    let models: serde_json::Map<String, Value> = document
        .get("models")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, model)| model.get("provider").and_then(Value::as_str) == Some(native_id))
        .map(|(key, model)| {
            let mut model = model.clone();
            model["provider"] = json!(id);
            (key.clone(), model)
        })
        .collect();
    let mut config = json!({"provider": document["providers"][native_id], "models": models});
    if id != native_id {
        config[LEGACY_NAMESPACE] = json!(true);
    }
    if let Some(selected) = document
        .get("default_model")
        .and_then(Value::as_str)
        .filter(|selected| config["models"].get(*selected).is_some())
    {
        config["default_model"] = json!(selected);
    }
    config
}

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

pub(crate) fn provider_models(config: &Value) -> Vec<&Value> {
    if let Some(models) = config.get("models").and_then(Value::as_object) {
        let preferred = config
            .get("default_model")
            .and_then(Value::as_str)
            .and_then(|key| models.get(key));
        return preferred
            .into_iter()
            .chain(
                models.values().filter(|model| {
                    preferred.is_none_or(|preferred| !std::ptr::eq(*model, preferred))
                }),
            )
            .collect();
    }
    let mut result = Vec::new();
    if let Some(model) = config.get("model") {
        result.push(model);
    }
    if let Some(models) = config.get("models").and_then(Value::as_array) {
        for model in models {
            if !result
                .iter()
                .any(|current| model_alias(current) == model_alias(model))
            {
                result.push(model);
            }
        }
    }
    result
}

pub(crate) fn json_to_toml(value: &Value) -> Option<toml_edit::Value> {
    match value {
        Value::Null => None,
        Value::Bool(value) => Some((*value).into()),
        Value::String(value) => Some(value.clone().into()),
        Value::Number(value) => value
            .as_i64()
            .map(Into::into)
            .or_else(|| value.as_f64().map(Into::into)),
        Value::Array(values) => {
            let mut array = toml_edit::Array::new();
            for value in values {
                array.push(json_to_toml(value)?);
            }
            Some(toml_edit::Value::Array(array))
        }
        Value::Object(values) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, value) in values {
                table.insert(key, json_to_toml(value)?);
            }
            Some(toml_edit::Value::InlineTable(table))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_models_preserve_default_effort_unknown_fields_and_native_aliases() {
        let model = json!({"model":"vendor/model", "alias":"friendly", "max_context_size":1048576, "default_effort":"max", "overrides":{"future":true}});
        let original = json!({"type":"openai", "base_url":"https://example.test/v1", "api_key":"test-key", "custom_headers":{"X-Team":"test"}, "model":model, "models":[model, {"model":"other", "max_context_size":1000}], "future_setting":{"keep":true}});
        let config = canonical_config("vendor", &original).unwrap();
        assert_eq!(
            config["provider"]["custom_headers"],
            original["custom_headers"]
        );
        assert_eq!(config["models"].as_object().unwrap().len(), 2);
        assert_eq!(config["default_model"], "cc-switch:vendor/friendly");
        assert_eq!(
            config["models"]["cc-switch:vendor/friendly"]["overrides"]["future"],
            true
        );
        assert_eq!(
            config["models"]["cc-switch:vendor/friendly"]["default_effort"],
            "max"
        );
        assert_eq!(config["future_setting"], original["future_setting"]);
        assert_eq!(canonical_config("vendor", &config).unwrap(), config);
    }

    #[test]
    fn conflicting_aliases_or_ownership_are_rejected_without_mutating_input() {
        let original = json!({"type":"openai", "model":{"model":"a", "alias":"same"}, "models":[{"model":"b", "alias":"same"}]});
        assert!(canonical_config("vendor", &original).is_err());
        let foreign = json!({"provider":{"type":"openai"}, "models":{"key":{"provider":"other","model":"m"}}});
        assert!(native_config("vendor", "cc-switch:vendor", &foreign).is_err());
        assert_eq!(foreign["models"]["key"]["provider"], "other");
    }

    #[test]
    fn existing_namespace_and_aliases_survive_snapshot_and_rewrite() {
        let document = json!({"default_model":"arbitrary/alias", "providers":{"cc-switch:vendor":{"type":"openai","api_key":"test"},"managed:kimi-code":{"type":"kimi","oauth":{"key":"login"}}}, "models":{"arbitrary/alias":{"provider":"cc-switch:vendor","model":"upstream","max_context_size":1000}}});
        let config = snapshot(&document, "cc-switch:vendor");
        assert_eq!(config["models"]["arbitrary/alias"]["provider"], "vendor");
        assert_eq!(
            native_id(&document, "vendor", Some(&config)).unwrap(),
            "cc-switch:vendor"
        );
        assert_eq!(
            native_config("vendor", "cc-switch:vendor", &config).unwrap()["models"],
            document["models"]
        );
        let mut collision = document.clone();
        collision["providers"]["vendor"] = json!({"type":"openai"});
        assert!(native_id(&collision, "vendor", Some(&config)).is_err());
    }
}
