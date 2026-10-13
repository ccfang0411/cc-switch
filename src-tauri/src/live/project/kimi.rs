//! Kimi Code 的投影：模式机要写进 `config.toml` 的东西——保留的代理 provider 表、
//! 模型表和 `default_model` 指针；退出代理时直连供应商和其余供应商的表按库里的行
//! 一并写回。
//!
//! 路由 / 聚合模式写一张保留 provider 表 `[providers."cc-switch:proxy"]`（客户端协议固定
//! `type = "anthropic"`，地址指向本地代理，Key 是占位符），`default_model` 指向它的模型表；
//! 用户的 `cc-switch:<id>` 表在代理模式下不改写（additive 写入被抑制，见
//! `services::provider`）。退出时代理表整组删掉，`default_model` 指回直连供应商的模型。

use std::path::Path;

use serde_json::{json, Value};
use toml_edit::{DocumentMut, Item, Table, TableLike};

use crate::app_config::AppType;
use crate::error::AppError;
use crate::kimi_config::{
    json_to_toml, MODEL_FIELDS, PROVIDER_FIELDS, PROXY_PLACEHOLDER, RESERVED_PROXY_TABLE,
};
use crate::live::patch::toml::TomlDocPatch;
use crate::live::patch::{KeyPath, LiveWriteError};
use crate::mode::stack::{self, Member};
use crate::provider::Provider;

/// 代理契约的客户端协议：Kimi Code 对代理讲 Anthropic Messages（`POST /kimicode/v1/messages`），
/// 上行协议由转发层按目标行的 `type` 选。
const PROXY_PROVIDER_TYPE: &str = "anthropic";

/// 供应商行中一个模型的发布配置（保留逐模型选项）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KimiModel {
    /// `[models."<provider>/<alias>"]` 的表键后缀：行里显式 `alias`，没有是模型 id
    /// （含 `/` 的模型 id 必须有显式 alias，`kimi_config::validate_provider` 保证，
    /// 所以 alias 不会含 `/`）。
    pub alias: String,
    /// 发往上游的模型 id。
    pub upstream: String,
    pub max_context_size: Option<u64>,
    pub display_name: Option<String>,
    options: Value,
}

impl KimiModel {
    /// 从供应商行（`settings_config`）取模型；行里没有模型（过不了校验的行）时是 `None`。
    pub fn of(settings: &Value) -> Option<Self> {
        Self::of_model(settings.get("model")?)
    }

    pub fn all_of(settings: &Value) -> Vec<Self> {
        crate::kimi_config::provider_models(settings)
            .into_iter()
            .filter_map(Self::of_model)
            .collect()
    }

    fn of_model(model: &Value) -> Option<Self> {
        let upstream = model
            .get("model")
            .and_then(Value::as_str)?
            .trim()
            .to_string();
        if upstream.is_empty() {
            return None;
        }
        let alias = model
            .get("alias")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|alias| !alias.is_empty())
            .unwrap_or(&upstream)
            .to_string();
        if alias.contains('/') {
            return None;
        }
        Some(Self {
            options: model.clone(),
            alias,
            upstream,
            max_context_size: model.get("max_context_size").and_then(Value::as_u64),
            display_name: model
                .get("display_name")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }
}

/// 模式机的一次写入：要写的 provider 表、模型表和 `default_model` 目标值。直接实现
/// `TomlDocPatch`，应用时整组删掉保留的代理表（`cc-switch:proxy` 及其模型表）、清掉
/// 同一供应商名下的陈旧模型表，再写目标。
#[derive(Debug, Clone, Default)]
pub struct KimiProjection {
    /// 要写的 provider 表（完整 key，如 `cc-switch:proxy`、`cc-switch:<id>`）。
    providers: Vec<(String, Table)>,
    /// 要写的模型表（完整 key，含所属 provider 前缀）。
    models: Vec<(String, Table)>,
    /// `default_model` 目标值；`None` 不动（还指着刚删掉的代理表时一并删掉）。
    default_model: Option<String>,
    /// 契约摘要材料（只有代理契约会算契约，见 `mode::contract::kimi`）。
    contract: Value,
}

impl KimiProjection {
    /// 代理契约：保留 provider 表 + 发布的模型表 + `default_model` 指针。
    /// `members` 是发布的 Stack 成员（路由模式为空）：路由模式发布目标那家的模型
    /// （别名沿用、model 是上游 id）；聚合模式每个成员的模型以 `ccs-kimi-<key>--<alias>`
    /// 发布，`default_model` 指向默认那家的编码 id。
    pub fn proxy(route: &Provider, base_url: &str, members: &[Member]) -> Result<Self, AppError> {
        let route_model = KimiModel::of(&route.settings_config).ok_or_else(|| {
            AppError::localized(
                "provider.kimicode.model.missing",
                format!("Kimi Code 供应商 {} 的配置缺少模型", route.id),
                format!("Kimi Code provider {} is missing the model", route.id),
            )
        })?;

        let mut provider = Table::new();
        provider.insert("name", toml_edit::value("CC Switch"));
        provider.insert("type", toml_edit::value(PROXY_PROVIDER_TYPE));
        provider.insert("base_url", toml_edit::value(base_url));
        provider.insert("api_key", toml_edit::value(PROXY_PLACEHOLDER));

        let published: Vec<(KimiModel, String, String, Option<String>)> = if members.is_empty() {
            KimiModel::all_of(&route.settings_config)
                .into_iter()
                .map(|model| {
                    let alias = model.alias.clone();
                    let upstream = model.upstream.clone();
                    let name = model.display_name.clone();
                    (model, alias, upstream, name)
                })
                .collect()
        } else {
            members
                .iter()
                .flat_map(|member| {
                    KimiModel::all_of(&member.provider.settings_config)
                        .into_iter()
                        .map(|model| {
                            let id =
                                stack::encode(&AppType::KimiCode, &member.key, &model.alias, false);
                            let name = stack::display_name(
                                model.display_name.as_deref().unwrap_or(&model.alias),
                                &member.provider.name,
                            );
                            (model, id.clone(), id, Some(name))
                        })
                })
                .collect()
        };
        let default = if members.is_empty() {
            route_model.alias.clone()
        } else {
            let route_key = members
                .iter()
                .find(|member| member.route)
                .map(|member| member.key.as_str())
                .unwrap_or_default();
            stack::encode(&AppType::KimiCode, route_key, &route_model.alias, false)
        };

        let mut models = Vec::with_capacity(published.len());
        for (mut model, alias, upstream, display_name) in published {
            model.upstream = upstream;
            model.display_name = display_name;
            let table = model_table(RESERVED_PROXY_TABLE, &model);
            models.push((format!("{RESERVED_PROXY_TABLE}/{alias}"), table));
        }
        let default_model = format!("{RESERVED_PROXY_TABLE}/{default}");
        let contract = json!({
            "provider": table_text(&provider),
            "models": models
                .iter()
                .map(|(key, table)| json!([key, table_text(table)]))
                .collect::<Vec<_>>(),
            "default_model": default_model,
        });
        Ok(Self {
            providers: vec![(RESERVED_PROXY_TABLE.to_string(), provider)],
            models,
            default_model: Some(default_model),
            contract,
        })
    }

    /// 直连写回：删掉代理表；有直连供应商时写回它的表并把 `default_model` 指回去。
    /// `None`（没有直连供应商，或它的行被占位符污染）时只删代理表和悬空的指针。
    pub fn direct(provider: Option<&Provider>) -> Result<Self, AppError> {
        let Some(provider) = provider else {
            return Ok(Self::default());
        };
        let model = KimiModel::of(&provider.settings_config).ok_or_else(|| {
            AppError::localized(
                "provider.kimicode.model.missing",
                format!("Kimi Code 供应商 {} 的配置缺少模型", provider.id),
                format!("Kimi Code provider {} is missing the model", provider.id),
            )
        })?;
        let provider_key = format!("cc-switch:{}", provider.id);
        let table = provider_table(&provider.settings_config);
        let models = KimiModel::all_of(&provider.settings_config)
            .iter()
            .map(|entry| {
                (
                    format!("{provider_key}/{}", entry.alias),
                    model_table(&provider_key, entry),
                )
            })
            .collect();
        Ok(Self {
            providers: vec![(provider_key, table)],
            models,
            default_model: Some(format!("cc-switch:{}/{}", provider.id, model.alias)),
            contract: Value::Null,
        })
    }

    /// 退出代理时把库里的行都按行写回：代理模式下 additive 写入被抑制，live 里的表
    /// 可能是旧的（编辑过、删除过）。行的模型解析不出来就跳过（校验本来就拦着）。
    pub fn restore_additive_rows(mut self, providers: impl IntoIterator<Item = Provider>) -> Self {
        for provider in providers {
            if crate::proxy::providers::kimi::managed_auth(&provider)
                || provider.meta.as_ref().and_then(|m| m.live_config_managed) != Some(true)
                || crate::services::ProxyService::config_has_proxy_placeholder(
                    &AppType::KimiCode,
                    &provider.settings_config,
                )
                || crate::kimi_config::validate_provider(&provider.id, &provider.settings_config)
                    .is_err()
            {
                continue;
            }
            let Some(_) = KimiModel::of(&provider.settings_config) else {
                log::warn!(
                    "写回 Kimi Code 直连配置：供应商 {} 的行里没有模型，跳过",
                    provider.id
                );
                continue;
            };
            let provider_key = format!("cc-switch:{}", provider.id);
            self.providers.retain(|(key, _)| key != &provider_key);
            self.providers.push((
                provider_key.clone(),
                provider_table(&provider.settings_config),
            ));
            let prefix = format!("{provider_key}/");
            self.models.retain(|(key, _)| !key.starts_with(&prefix));
            for entry in KimiModel::all_of(&provider.settings_config) {
                let model_key = format!("{provider_key}/{}", entry.alias);
                if !self.models.iter().any(|(key, _)| key == &model_key) {
                    self.models
                        .push((model_key, model_table(&provider_key, &entry)));
                }
            }
        }
        self
    }

    /// 契约摘要用的规范形式。
    pub fn to_value(&self) -> Value {
        self.contract.clone()
    }

    fn apply_to(&self, path: &Path, doc: &mut DocumentMut) -> Result<(), LiveWriteError> {
        let root = doc.as_table_mut();

        if let Some(providers) = table_mut(path, root, "providers", false)? {
            providers.remove(RESERVED_PROXY_TABLE);
        }
        if let Some(models) = table_mut(path, root, "models", false)? {
            let targets: Vec<&str> = self.models.iter().map(|(key, _)| key.as_str()).collect();
            // 保留的代理表和同一供应商名下、不在目标里的模型表一并删掉（改名留下的
            // 陈旧别名、上一次代理发布的表）。用户手写共享前缀的表会被当成陈旧表
            // 删掉——和 `kimi_config::write_model` 的语义一致。
            let prefixes: Vec<String> = std::iter::once(format!("{RESERVED_PROXY_TABLE}/"))
                .chain(
                    self.providers
                        .iter()
                        .map(|(key, _)| format!("{key}/"))
                        .collect::<Vec<_>>(),
                )
                .collect();
            let doomed: Vec<String> = models
                .iter()
                .filter(|(key, _)| {
                    !targets.contains(key)
                        && prefixes
                            .iter()
                            .any(|prefix| key.starts_with(prefix.as_str()))
                })
                .map(|(key, _)| key.to_string())
                .collect();
            for key in doomed {
                models.remove(&key);
            }
        }

        if !self.providers.is_empty() {
            let providers = table_mut(path, root, "providers", true)?.expect("created");
            for (key, table) in &self.providers {
                insert_table(providers, key, table);
            }
        }
        if !self.models.is_empty() {
            let models = table_mut(path, root, "models", true)?.expect("created");
            for (key, table) in &self.models {
                insert_table(models, key, table);
            }
        }

        match &self.default_model {
            Some(default) => match root.get_mut("default_model") {
                Some(Item::Value(slot)) => {
                    let decor = slot.decor().clone();
                    *slot = default.as_str().into();
                    *slot.decor_mut() = decor;
                }
                _ => {
                    root.insert("default_model", toml_edit::value(default));
                }
            },
            None => {
                // 没给目标值时，`default_model` 不能指着刚删掉的代理表。
                let dangling = root
                    .get("default_model")
                    .and_then(Item::as_str)
                    .is_some_and(|current| {
                        current.starts_with(&format!("{RESERVED_PROXY_TABLE}/"))
                    });
                if dangling {
                    root.remove("default_model");
                }
            }
        }
        Ok(())
    }
}

impl TomlDocPatch for KimiProjection {
    fn apply_to(&self, path: &Path, doc: &mut DocumentMut) -> Result<(), LiveWriteError> {
        Self::apply_to(self, path, doc)
    }
}

/// 供应商行的 provider 表（只含 CC Switch 管理的字段）。
fn provider_table(settings: &Value) -> Table {
    let mut table = Table::new();
    for field in PROVIDER_FIELDS {
        if let Some(value) = settings.get(field).and_then(json_to_toml) {
            table.insert(field, Item::Value(value));
        }
    }
    table
}

/// 供应商行的模型表（`provider` 指向自己的表）。
fn model_table(provider_key: &str, model: &KimiModel) -> Table {
    let mut table = Table::new();
    for field in MODEL_FIELDS {
        if let Some(value) = model.options.get(field).and_then(json_to_toml) {
            table.insert(field, Item::Value(value));
        }
    }
    table.insert("provider", toml_edit::value(provider_key));
    table.insert("model", toml_edit::value(model.upstream.as_str()));
    if let Some(size) = model.max_context_size {
        table.insert("max_context_size", toml_edit::value(size as i64));
    }
    if let Some(name) = model.display_name.as_deref() {
        table.insert("display_name", toml_edit::value(name));
    }
    table
}

/// 摘要用的表文本（键顺序即写入顺序）。
fn table_text(table: &Table) -> String {
    let mut doc = DocumentMut::new();
    for (key, item) in table.iter() {
        doc.insert(key, item.clone());
    }
    doc.to_string()
}

/// 原位替换或插入一张表：已有时沿用原来的排版和位置（见 Grok 的同款处理）。
fn insert_table(tables: &mut dyn TableLike, key: &str, table: &Table) {
    match tables.get_mut(key) {
        Some(slot) => {
            let mut table = table.clone();
            if let Item::Table(old) = slot {
                *table.decor_mut() = old.decor().clone();
                if let Some(position) = old.position() {
                    table.set_position(position);
                }
            }
            *slot = Item::Table(table);
        }
        None => {
            tables.insert(key, Item::Table(table.clone()));
        }
    }
}

/// 根下的表；`create` 时缺失就建（隐式表，只输出子表头，不输出 `[providers]` / `[models]`）。
fn table_mut<'a>(
    path: &Path,
    root: &'a mut Table,
    key: &str,
    create: bool,
) -> Result<Option<&'a mut dyn TableLike>, LiveWriteError> {
    if !root.contains_key(key) {
        if !create {
            return Ok(None);
        }
        let mut table = Table::new();
        table.set_implicit(true);
        root.insert(key, Item::Table(table));
    }
    root.get_mut(key)
        .and_then(Item::as_table_like_mut)
        .map(Some)
        .ok_or_else(|| LiveWriteError::Shape {
            path: path.to_path_buf(),
            key_path: KeyPath::new(&[key]),
            expected: "表",
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::patch::LivePatch;

    fn row() -> Value {
        json!({
            "type": "kimi",
            "base_url": "https://api.kimi.com/coding/v1",
            "api_key": "sk-a",
            "model": {"model": "k3", "max_context_size": 1048576, "display_name": "K3"},
        })
    }

    fn route() -> Provider {
        Provider::with_id("a".to_string(), "A".to_string(), row(), None)
    }

    fn apply(patch: &KimiProjection, pre: &str) -> String {
        String::from_utf8(
            patch
                .apply(Path::new("config.toml"), Some(pre.as_bytes()))
                .unwrap(),
        )
        .unwrap()
    }

    const USER_LIVE: &str = "# mine\ndefault_model = \"cc-switch:a/k3\"\n\n[providers.\"cc-switch:a\"]\ntype = \"kimi\"\nbase_url = \"https://a.example/v1\"\napi_key = \"sk-a\"\n\n[providers.\"managed:kimi-code\"]\ntype = \"kimi\"\nbase_url = \"https://api.kimi.com/coding/v1\"\napi_key = \"\"\n\n[models.\"cc-switch:a/k3\"]\nprovider = \"cc-switch:a\"\nmodel = \"k3\"\nmax_context_size = 1048576\n";

    #[test]
    fn kimi_multiple_models_keep_context_and_effort_in_all_projections() {
        let mut provider = route();
        provider.settings_config["model"]["support_efforts"] = json!(["low", "high", "ultra"]);
        provider.settings_config["model"]["default_effort"] = json!("ultra");
        provider.settings_config["models"] = json!([
            provider.settings_config["model"].clone(),
            {"model":"other", "max_context_size":123456,
             "support_efforts":["low","max"], "default_effort":"max",
             "capabilities":["thinking","tool_use"]}
        ]);
        let routed = KimiProjection::proxy(&provider, "http://localhost/kimicode", &[]).unwrap();
        let text = apply(&routed, USER_LIVE);
        let doc: DocumentMut = text.parse().unwrap();
        assert_eq!(
            doc["models"]["cc-switch:proxy/other"]["max_context_size"].as_integer(),
            Some(123456)
        );
        assert_eq!(
            doc["models"]["cc-switch:proxy/k3"]["default_effort"].as_str(),
            Some("ultra")
        );
        assert_eq!(
            doc["models"]["cc-switch:proxy/other"]["default_effort"].as_str(),
            Some("max")
        );
        let member = Member {
            provider: provider.clone(),
            key: "a".into(),
            route: true,
            model_ids: stack::kimi_model_ids("a", &provider),
        };
        let stacked =
            KimiProjection::proxy(&provider, "http://localhost/kimicode", &[member]).unwrap();
        let stack_text = apply(&stacked, USER_LIVE);
        let stack_doc: DocumentMut = stack_text.parse().unwrap();
        assert_eq!(
            stack_doc["models"]["cc-switch:proxy/ccs-kimi-a--other"]["model"].as_str(),
            Some("ccs-kimi-a--other")
        );
        let restored = KimiProjection::direct(Some(&provider)).unwrap();
        let restored_text = apply(&restored, &text);
        let restored_doc: DocumentMut = restored_text.parse().unwrap();
        assert_eq!(
            restored_doc["models"]["cc-switch:a/other"]["default_effort"].as_str(),
            Some("max")
        );
        assert!(restored_doc["models"]
            .get("cc-switch:proxy/other")
            .is_none());
    }

    #[test]
    fn proxy_contract_writes_the_reserved_table_and_keeps_user_tables() {
        let projection =
            KimiProjection::proxy(&route(), "http://127.0.0.1:15721/kimicode", &[]).unwrap();
        let out = apply(&projection, USER_LIVE);
        let doc: DocumentMut = out.parse().unwrap();
        let proxy = &doc["providers"]["cc-switch:proxy"];
        assert_eq!(proxy["type"].as_str(), Some("anthropic"));
        assert_eq!(
            proxy["base_url"].as_str(),
            Some("http://127.0.0.1:15721/kimicode")
        );
        assert_eq!(proxy["api_key"].as_str(), Some(PROXY_PLACEHOLDER));
        assert_eq!(doc["default_model"].as_str(), Some("cc-switch:proxy/k3"));
        let model = &doc["models"]["cc-switch:proxy/k3"];
        assert_eq!(model["provider"].as_str(), Some("cc-switch:proxy"));
        assert_eq!(model["model"].as_str(), Some("k3"));
        assert_eq!(model["max_context_size"].as_integer(), Some(1048576));
        // 用户表和 managed 表不动。
        assert_eq!(
            doc["providers"]["cc-switch:a"]["api_key"].as_str(),
            Some("sk-a")
        );
        assert!(doc["providers"].get("managed:kimi-code").is_some());
        assert!(out.starts_with("# mine\n"), "{out}");
    }

    #[test]
    fn reentering_replaces_stale_proxy_model_tables() {
        let first = apply(
            &KimiProjection::proxy(&route(), "http://127.0.0.1:15721/kimicode", &[]).unwrap(),
            USER_LIVE,
        );
        let mut renamed = route();
        renamed.settings_config["model"]["model"] = json!("k4");
        let out = apply(
            &KimiProjection::proxy(&renamed, "http://127.0.0.1:15721/kimicode", &[]).unwrap(),
            &first,
        );
        let doc: DocumentMut = out.parse().unwrap();
        assert!(doc["models"].get("cc-switch:proxy/k3").is_none(), "{out}");
        assert_eq!(
            doc["models"]["cc-switch:proxy/k4"]["model"].as_str(),
            Some("k4")
        );
        assert_eq!(doc["default_model"].as_str(), Some("cc-switch:proxy/k4"));
    }

    #[test]
    fn exit_restores_the_additive_rows_and_removes_the_proxy_table() {
        let entered = apply(
            &KimiProjection::proxy(&route(), "http://127.0.0.1:15721/kimicode", &[]).unwrap(),
            USER_LIVE,
        );
        // 代理模式下 additive 写入被抑制：行已经改了，live 里的表还是旧的。
        let mut edited = route();
        edited.settings_config["model"]["model"] = json!("k4");
        let projection = KimiProjection::direct(Some(&edited))
            .unwrap()
            .restore_additive_rows([edited.clone()]);
        let out = apply(&projection, &entered);
        let doc: DocumentMut = out.parse().unwrap();
        assert!(doc["providers"].get("cc-switch:proxy").is_none(), "{out}");
        assert!(doc["models"].get("cc-switch:proxy/k3").is_none(), "{out}");
        assert_eq!(doc["default_model"].as_str(), Some("cc-switch:a/k4"));
        assert_eq!(
            doc["models"]["cc-switch:a/k4"]["model"].as_str(),
            Some("k4")
        );
        // 陈旧别名删掉；managed 表不动。
        assert!(doc["models"].get("cc-switch:a/k3").is_none(), "{out}");
        assert!(doc["providers"].get("managed:kimi-code").is_some());
    }

    #[test]
    fn exit_without_a_direct_provider_drops_the_dangling_default() {
        let entered = apply(
            &KimiProjection::proxy(&route(), "http://127.0.0.1:15721/kimicode", &[]).unwrap(),
            USER_LIVE,
        );
        let out = apply(&KimiProjection::direct(None).unwrap(), &entered);
        let doc: DocumentMut = out.parse().unwrap();
        assert!(doc["providers"].get("cc-switch:proxy").is_none(), "{out}");
        assert!(doc.get("default_model").is_none(), "{out}");
        // 用户自己的表不碰。
        assert!(doc["providers"].get("cc-switch:a").is_some());
    }

    #[test]
    fn contract_material_changes_with_the_route_and_the_proxy_address() {
        let a = KimiProjection::proxy(&route(), "http://127.0.0.1:15721/kimicode", &[]).unwrap();
        let same = KimiProjection::proxy(&route(), "http://127.0.0.1:15721/kimicode", &[]).unwrap();
        assert_eq!(a.to_value(), same.to_value());
        let other_url =
            KimiProjection::proxy(&route(), "http://127.0.0.1:15722/kimicode", &[]).unwrap();
        assert_ne!(a.to_value(), other_url.to_value());
        let mut renamed = route();
        renamed.settings_config["model"]["model"] = json!("k4");
        let other_route =
            KimiProjection::proxy(&renamed, "http://127.0.0.1:15721/kimicode", &[]).unwrap();
        assert_ne!(a.to_value(), other_route.to_value());
    }
}
