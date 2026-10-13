//! Kimi Code Provider Adapter
//!
//! Inbound the Kimi Code CLI speaks Anthropic Messages: the routing/stack-mode proxy
//! contract pins the reserved provider table to `type = "anthropic"`, so the client
//! posts to `<proxy>/kimicode/v1/messages`. The upstream protocol follows the target
//! row's own `type`:
//!
//! - `kimi` / `openai`：OpenAI Chat Completions（api.kimi.com/coding/v1 是 OpenAI 兼容）
//! - `anthropic`：透传
//! - `openai_responses`：OpenAI Responses
//! - `google-genai` / `vertexai`：Gemini Native
//!
//! 转换全部复用 Claude 路径的 transform（`transform` / `transform_responses` /
//! `transform_gemini`），这里只做协议选择和凭据提取。

use super::adapter::auth_header_value;
use super::{AuthInfo, AuthStrategy, ProviderAdapter};
use crate::provider::Provider;
use crate::proxy::error::ProxyError;
use serde_json::Value as JsonValue;

/// Kimi Code 供应商行的上游协议（`settings_config.type`），取值和 Claude 路径的
/// `api_format` 一致，好让转换层复用。行里缺 `type` 时按透传处理。
pub fn managed_auth(provider: &Provider) -> bool {
    provider.is_github_copilot() || provider.is_codex_oauth() || provider.is_xai_oauth()
}

pub fn kimi_api_format(provider: &Provider) -> &'static str {
    if managed_auth(provider) {
        return super::get_claude_api_format(provider);
    }
    match provider
        .settings_config
        .get("type")
        .and_then(JsonValue::as_str)
        .unwrap_or("anthropic")
    {
        "kimi" | "openai" => "openai_chat",
        "openai_responses" => "openai_responses",
        "google-genai" | "vertexai" => "gemini_native",
        _ => "anthropic",
    }
}

/// Kimi Code 适配器
pub struct KimiAdapter;

impl KimiAdapter {
    pub fn new() -> Self {
        Self
    }

    /// 行里的 Key：`api_key` 直填，或 `api_key_env` 指名的环境变量（kimi_config 校验
    /// 保证两者恰好其一）。
    fn extract_key(&self, provider: &Provider) -> Option<String> {
        if let Some(key) = provider
            .settings_config
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|key| !key.is_empty())
        {
            return Some(key.to_string());
        }
        provider
            .settings_config
            .get("api_key_env")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .and_then(|name| std::env::var(name).ok())
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
    }
}

impl Default for KimiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAdapter for KimiAdapter {
    fn name(&self) -> &'static str {
        "Kimi"
    }

    fn extract_base_url(&self, provider: &Provider) -> Result<String, ProxyError> {
        if managed_auth(provider) {
            return super::ClaudeAdapter::new().extract_base_url(provider);
        }
        if let Some(url) = provider
            .settings_config
            .get("base_url")
            .or_else(|| provider.settings_config.get("baseURL"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|url| !url.is_empty())
        {
            return Ok(url.trim_end_matches('/').to_string());
        }
        Err(ProxyError::ConfigError(
            "Kimi Code Provider 缺少 base_url 配置".to_string(),
        ))
    }

    fn extract_auth(&self, provider: &Provider) -> Option<AuthInfo> {
        if managed_auth(provider) {
            return super::ClaudeAdapter::new().extract_auth(provider);
        }
        let key = self.extract_key(provider)?;
        // Anthropic 上游：x-api-key，和 Kimi Code 直连时 Anthropic SDK 的线形一致。
        // Gemini Native：google-genai 用 x-goog-api-key；vertexai 的 api_key(_env) 是
        // GCP access token，走 Bearer。其余 OpenAI 系上游一律 Bearer。
        let strategy = match kimi_api_format(provider) {
            "anthropic" => AuthStrategy::Anthropic,
            "gemini_native" => match provider
                .settings_config
                .get("type")
                .and_then(JsonValue::as_str)
            {
                Some("google-genai") => AuthStrategy::Google,
                _ => AuthStrategy::Bearer,
            },
            _ => AuthStrategy::Bearer,
        };
        Some(AuthInfo::new(key, strategy))
    }

    fn build_url(&self, base_url: &str, endpoint: &str) -> String {
        if matches!(
            base_url,
            super::CHATGPT_CODEX_BASE_URL | super::XAI_API_BASE_URL
        ) {
            return super::ClaudeAdapter::new().build_url(base_url, endpoint);
        }
        let base_trimmed = base_url.trim_end_matches('/');
        let endpoint_trimmed = endpoint.trim_start_matches('/');

        // 和 Codex 同一套规则：纯 origin 补 /v1；已含 /v1 或自定义前缀（如
        // api.kimi.com/coding/v1）直接拼接。
        let already_has_v1 = base_trimmed.ends_with("/v1");
        let origin_only = super::codex::is_origin_only_url(base_trimmed);

        let mut url = if already_has_v1 {
            format!("{base_trimmed}/{endpoint_trimmed}")
        } else if origin_only {
            format!("{base_trimmed}/v1/{endpoint_trimmed}")
        } else {
            format!("{base_trimmed}/{endpoint_trimmed}")
        };

        while url.contains("/v1/v1") {
            url = url.replace("/v1/v1", "/v1");
        }

        url
    }

    fn get_auth_headers(
        &self,
        auth: &AuthInfo,
    ) -> Result<Vec<(http::HeaderName, http::HeaderValue)>, ProxyError> {
        if matches!(
            auth.strategy,
            AuthStrategy::GitHubCopilot | AuthStrategy::CodexOAuth | AuthStrategy::XaiOAuth
        ) {
            return super::ClaudeAdapter::new().get_auth_headers(auth);
        }
        match auth.strategy {
            AuthStrategy::Anthropic => Ok(vec![(
                http::HeaderName::from_static("x-api-key"),
                auth_header_value(&auth.api_key)?,
            )]),
            AuthStrategy::Google => Ok(vec![(
                http::HeaderName::from_static("x-goog-api-key"),
                auth_header_value(&auth.api_key)?,
            )]),
            _ => {
                let bearer = format!("Bearer {}", auth.api_key);
                Ok(vec![(
                    http::HeaderName::from_static("authorization"),
                    auth_header_value(&bearer)?,
                )])
            }
        }
    }

    fn needs_transform(&self, provider: &Provider) -> bool {
        super::claude_api_format_needs_transform(kimi_api_format(provider))
    }

    fn transform_request(
        &self,
        body: JsonValue,
        provider: &Provider,
    ) -> Result<JsonValue, ProxyError> {
        super::transform_claude_request_for_api_format(
            body,
            provider,
            kimi_api_format(provider),
            None,
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn provider(config: JsonValue) -> Provider {
        Provider::with_id("test".to_string(), "Test Kimi".to_string(), config, None)
    }

    #[test]
    fn kimi_codex_effort_uses_selected_models_official_tiers() {
        let mut row = provider(json!({
            "model":{"model":"first", "support_efforts":["low","high"], "default_effort":"high"},
            "models":[{"model":"second", "support_efforts":["none","low","ultra"], "default_effort":"ultra"}]
        }));
        row.meta = Some(crate::provider::ProviderMeta {
            provider_type: Some("codex_oauth".into()),
            ..Default::default()
        });
        let mut output = json!({"model":"second", "reasoning":{"effort":"xhigh","summary":"auto"}});
        apply_codex_reasoning_selection(&mut output, &json!({}), &row);
        assert_eq!(output["reasoning"]["effort"], "ultra");
        assert_eq!(output["reasoning"]["summary"], "auto");
        apply_codex_reasoning_selection(
            &mut output,
            &json!({"output_config":{"effort":"low"}}),
            &row,
        );
        assert_eq!(output["reasoning"]["effort"], "low");
        apply_codex_reasoning_selection(
            &mut output,
            &json!({"thinking":{"type":"disabled"}}),
            &row,
        );
        assert_eq!(output["reasoning"]["effort"], "none");
    }

    #[test]
    fn managed_accounts_reuse_claude_auth_and_force_protocol() {
        for (kind, format, strategy) in [
            ("github_copilot", "openai_chat", AuthStrategy::GitHubCopilot),
            ("codex_oauth", "openai_responses", AuthStrategy::CodexOAuth),
            ("xai_oauth", "openai_responses", AuthStrategy::XaiOAuth),
        ] {
            let mut row = provider(
                json!({"type": "anthropic", "base_url": "https://api.githubcopilot.com", "api_key": "must-not-be-used"}),
            );
            row.meta = Some(crate::provider::ProviderMeta {
                provider_type: Some(kind.into()),
                api_format: Some("openai_chat".into()),
                ..Default::default()
            });
            let adapter = KimiAdapter::new();
            assert_eq!(kimi_api_format(&row), format);
            let auth = adapter.extract_auth(&row).unwrap();
            assert_eq!(auth.strategy, strategy);
            if kind != "github_copilot" {
                let base = adapter.extract_base_url(&row).unwrap();
                assert_eq!(
                    adapter.build_url(&base, "/v1/messages"),
                    format!("{base}/responses")
                );
            }
            assert_ne!(auth.api_key, "must-not-be-used");
            assert!(adapter.get_auth_headers(&auth).is_ok());
            if kind == "codex_oauth" {
                assert_eq!(
                    adapter.extract_base_url(&row).unwrap(),
                    super::super::CHATGPT_CODEX_BASE_URL
                );
            }
            if kind == "xai_oauth" {
                assert_eq!(
                    adapter.extract_base_url(&row).unwrap(),
                    super::super::XAI_API_BASE_URL
                );
            }
        }
    }

    #[test]
    fn api_format_follows_the_rows_type() {
        for (row_type, expected) in [
            ("kimi", "openai_chat"),
            ("openai", "openai_chat"),
            ("anthropic", "anthropic"),
            ("openai_responses", "openai_responses"),
            ("google-genai", "gemini_native"),
            ("vertexai", "gemini_native"),
        ] {
            let provider = provider(json!({ "type": row_type }));
            assert_eq!(kimi_api_format(&provider), expected, "{row_type}");
        }
        // 缺 type 的行按透传处理。
        assert_eq!(kimi_api_format(&provider(json!({}))), "anthropic");
    }

    #[test]
    fn extract_reads_snake_case_fields() {
        let adapter = KimiAdapter::new();
        let provider = provider(json!({
            "type": "kimi",
            "base_url": "https://api.kimi.com/coding/v1/",
            "api_key": "sk-kimi"
        }));
        assert_eq!(
            adapter.extract_base_url(&provider).unwrap(),
            "https://api.kimi.com/coding/v1"
        );
        let auth = adapter.extract_auth(&provider).unwrap();
        assert_eq!(auth.api_key, "sk-kimi");
        assert_eq!(auth.strategy, AuthStrategy::Bearer);
        let headers = adapter.get_auth_headers(&auth).unwrap();
        assert_eq!(
            headers[0],
            (
                http::HeaderName::from_static("authorization"),
                http::HeaderValue::from_static("Bearer sk-kimi")
            )
        );
    }

    #[test]
    fn anthropic_rows_send_x_api_key_like_the_cli_does() {
        let adapter = KimiAdapter::new();
        let provider = provider(json!({
            "type": "anthropic",
            "base_url": "https://api.anthropic.com",
            "api_key": "sk-ant"
        }));
        assert!(!adapter.needs_transform(&provider));
        let auth = adapter.extract_auth(&provider).unwrap();
        assert_eq!(auth.strategy, AuthStrategy::Anthropic);
        let headers = adapter.get_auth_headers(&auth).unwrap();
        assert_eq!(
            headers[0],
            (
                http::HeaderName::from_static("x-api-key"),
                http::HeaderValue::from_static("sk-ant")
            )
        );
    }

    #[test]
    fn api_key_env_resolves_from_the_environment() {
        let adapter = KimiAdapter::new();
        let name = "CC_SWITCH_TEST_KIMI_KEY";
        std::env::set_var(name, "sk-from-env");
        let provider = provider(json!({
            "type": "openai",
            "base_url": "https://example.com/v1",
            "api_key_env": name
        }));
        let auth = adapter.extract_auth(&provider).unwrap();
        assert_eq!(auth.api_key, "sk-from-env");
        std::env::remove_var(name);
        assert!(adapter.extract_auth(&provider).is_none());
    }

    #[test]
    fn transform_only_for_non_anthropic_upstreams() {
        let adapter = KimiAdapter::new();
        for (row_type, needs) in [
            ("anthropic", false),
            ("kimi", true),
            ("openai", true),
            ("openai_responses", true),
            ("google-genai", true),
        ] {
            let provider = provider(json!({ "type": row_type }));
            assert_eq!(adapter.needs_transform(&provider), needs, "{row_type}");
        }
    }

    #[test]
    fn build_url_dedupes_v1_and_fills_origin_only() {
        let adapter = KimiAdapter::new();
        assert_eq!(
            adapter.build_url("https://api.kimi.com/coding/v1", "/v1/messages"),
            "https://api.kimi.com/coding/v1/messages"
        );
        assert_eq!(
            adapter.build_url("https://api.openai.com", "/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            adapter.build_url("https://relay.example/anthropic", "/v1/messages"),
            "https://relay.example/anthropic/v1/messages"
        );
    }
}

/// Keep the account catalog's exact reasoning tier after the generic Anthropic bridge.
/// This is scoped to Kimi's selected Codex model, never a global model-name whitelist.
pub(crate) fn apply_codex_reasoning_selection(
    outbound: &mut JsonValue,
    inbound: &JsonValue,
    provider: &Provider,
) {
    if !provider.is_codex_oauth() {
        return;
    }
    let model_id = outbound
        .get("model")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    let models = crate::kimi_config::provider_models(&provider.settings_config);
    let Some(model) = models.into_iter().find(|model| {
        model.get("model").and_then(JsonValue::as_str) == Some(model_id)
            || crate::kimi_config::model_alias(model) == Some(model_id)
    }) else {
        return;
    };
    let Some(levels) = model.get("support_efforts").and_then(JsonValue::as_array) else {
        return;
    };
    let allowed = |effort: &str| levels.iter().any(|level| level.as_str() == Some(effort));
    let explicit = inbound
        .pointer("/output_config/effort")
        .or_else(|| inbound.pointer("/reasoning/effort"))
        .and_then(JsonValue::as_str)
        .filter(|effort| allowed(effort));
    let default = model
        .get("default_effort")
        .and_then(JsonValue::as_str)
        .filter(|effort| allowed(effort));
    let disabled = inbound
        .pointer("/thinking/type")
        .and_then(JsonValue::as_str)
        == Some("disabled");
    let effort = if disabled {
        if allowed("none") {
            Some("none")
        } else {
            levels.first().and_then(JsonValue::as_str)
        }
    } else {
        explicit.or(default)
    };
    if let Some(effort) = effort {
        if !outbound.get("reasoning").is_some_and(JsonValue::is_object) {
            outbound["reasoning"] = serde_json::json!({});
        }
        outbound["reasoning"]["effort"] = JsonValue::String(effort.to_string());
    }
}
