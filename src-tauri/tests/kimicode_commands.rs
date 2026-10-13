use cc_switch_lib::{AppType, Prompt, PromptService, Provider, ProviderService};
use serde_json::json;
use std::fs;
use std::path::PathBuf;

#[path = "support.rs"]
mod support;
use support::{create_test_state, ensure_test_home, reset_test_fs, test_mutex};

fn kimi_provider(id: &str) -> Provider {
    Provider::with_id(
        id.into(),
        "Test".into(),
        json!({
            "name": "Test",
            "type": "kimi",
            "base_url": "https://api.example.com/coding/v1",
            "api_key": "test-key",
            "model": {"model": "k3", "max_context_size": 1048576, "display_name": "K3"}
        }),
        None,
    )
}

fn kimi_config_path() -> PathBuf {
    ensure_test_home().join(".kimi-code/config.toml")
}

fn read_default_model() -> String {
    let text = fs::read_to_string(kimi_config_path()).unwrap();
    let document = text.parse::<toml_edit::DocumentMut>().unwrap();
    document["default_model"].as_str().unwrap().to_string()
}

#[test]
fn failed_kimicode_prompt_writes_preserve_state_and_can_be_retried() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    let state = create_test_state().unwrap();
    let path = ensure_test_home().join(".kimi-code/AGENTS.md");
    let active: Prompt = serde_json::from_value(json!({
        "id":"active", "name":"Active", "content":"original", "enabled":true
    }))
    .unwrap();
    state.db.save_prompt("kimicode", &active).unwrap();
    // A directory at the file path deterministically rejects the atomic rename.
    fs::create_dir_all(&path).unwrap();
    let mut edited = active.clone();
    edited.content = "updated".into();
    assert!(
        PromptService::upsert_prompt(&state, AppType::KimiCode, &active.id, edited.clone())
            .is_err()
    );
    let mut disabled = active.clone();
    disabled.enabled = false;
    assert!(
        PromptService::upsert_prompt(&state, AppType::KimiCode, &active.id, disabled.clone())
            .is_err()
    );
    let stored = &state.db.get_prompts("kimicode").unwrap()[&active.id];
    assert!(stored.enabled);
    assert_eq!(stored.content, "original");
    fs::remove_dir(&path).unwrap();
    fs::write(&path, "original").unwrap();
    PromptService::upsert_prompt(&state, AppType::KimiCode, &active.id, disabled).unwrap();
    assert!(!state.db.get_prompts("kimicode").unwrap()[&active.id].enabled);
    assert_eq!(fs::read_to_string(&path).unwrap(), "");
    PromptService::upsert_prompt(&state, AppType::KimiCode, &active.id, edited).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "updated");
    assert!(state.db.get_prompts("kimicode").unwrap()[&active.id].enabled);
}

#[test]
fn kimicode_provider_write_failure_rolls_back_the_database_row() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    let state = create_test_state().unwrap();
    let path = kimi_config_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Kimi Code's own lock is honored: the add fails before anything is committed.
    let lock = PathBuf::from(format!("{}.lock", path.display()));
    fs::create_dir(&lock).unwrap();
    assert!(
        ProviderService::add(&state, AppType::KimiCode, kimi_provider("official"), true).is_err()
    );
    assert!(state.db.get_all_providers("kimicode").unwrap().is_empty());
    assert!(!path.exists());
    fs::remove_dir(&lock).unwrap();
    ProviderService::add(&state, AppType::KimiCode, kimi_provider("official"), true).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("[providers.\"cc-switch:official\"]"));
    assert!(text.contains("[models.\"cc-switch:official/k3\"]"));
    assert_eq!(read_default_model(), "cc-switch:official/k3");
}

#[test]
fn adding_a_kimicode_provider_rejects_a_key_already_in_use() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    let state = create_test_state().unwrap();
    let path = kimi_config_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let native = "default_model = \"cc-switch:handwritten/k3\"\n\n[providers.\"cc-switch:handwritten\"]\ntype = \"kimi\"\nbase_url = \"https://api.example.com/coding/v1\"\napi_key = \"mine\"\n\n[models.\"cc-switch:handwritten/k3\"]\nprovider = \"cc-switch:handwritten\"\nmodel = \"k3\"\nmax_context_size = 1048576\n";
    fs::write(&path, native).unwrap();

    // Live `cc-switch:` nodes are never overwritten by an add.
    assert!(ProviderService::add(
        &state,
        AppType::KimiCode,
        kimi_provider("handwritten"),
        true
    )
    .is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), native);
    assert!(state.db.get_all_providers("kimicode").unwrap().is_empty());

    // Neither is a row that only lives in the database.
    ProviderService::add(&state, AppType::KimiCode, kimi_provider("fresh"), false).unwrap();
    assert!(!fs::read_to_string(&path)
        .unwrap()
        .contains("cc-switch:fresh"));
    let mut renamed = kimi_provider("fresh");
    renamed.name = "Other".into();
    assert!(ProviderService::add(&state, AppType::KimiCode, renamed, true).is_err());
    assert_eq!(
        state
            .db
            .get_provider_by_id("fresh", "kimicode")
            .unwrap()
            .unwrap()
            .name,
        "Test"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), native);
}

#[test]
fn kimicode_managed_entries_are_never_listed_or_modified() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    let state = create_test_state().unwrap();
    let path = kimi_config_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let native = "# Kimi Code configuration\ndefault_model = \"managed:kimi-code/k3\"\n\n# OAuth login, do not touch\n[providers.\"managed:kimi-code\"]\ntype = \"kimi\"\nbase_url = \"https://api.kimi.com/coding/v1\"\napi_key = \"\"\n\n[models.\"managed:kimi-code/k3\"]\nprovider = \"managed:kimi-code\"\nmodel = \"k3\"\nmax_context_size = 1048576\ncapabilities = [\"thinking\", \"image_in\", \"tool_use\"]\ndisplay_name = \"K3\"\n";
    fs::write(&path, native).unwrap();

    // OAuth login artifacts are not cc-switch providers.
    let listed = ProviderService::list(&state, AppType::KimiCode).unwrap();
    assert!(listed.is_empty());
    assert_eq!(fs::read_to_string(&path).unwrap(), native);

    // Adding/switching our own provider leaves the managed entry byte-identical.
    ProviderService::add(&state, AppType::KimiCode, kimi_provider("official"), true).unwrap();
    let written = fs::read_to_string(&path).unwrap();
    assert!(written.contains("# OAuth login, do not touch"));
    assert!(written.contains("[providers.\"managed:kimi-code\"]"));
    assert!(written.contains("[models.\"managed:kimi-code/k3\"]"));
    assert!(written.contains("capabilities = [\"thinking\", \"image_in\", \"tool_use\"]"));
    assert_eq!(read_default_model(), "cc-switch:official/k3");
    let listed = ProviderService::list(&state, AppType::KimiCode).unwrap();
    assert!(listed.contains_key("official"));
    assert!(!listed.contains_key("managed:kimi-code"));

    // Deleting our provider must not remove the managed entries.
    let mut document = fs::read_to_string(&path)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    document["default_model"] = toml_edit::value("managed:kimi-code/k3");
    fs::write(&path, document.to_string()).unwrap();
    ProviderService::delete(&state, AppType::KimiCode, "official").unwrap();
    let after = fs::read_to_string(&path).unwrap();
    assert!(after.contains("[providers.\"managed:kimi-code\"]"));
    assert!(after.contains("[models.\"managed:kimi-code/k3\"]"));
    assert!(!after.contains("cc-switch:official"));
    assert!(state.db.get_all_providers("kimicode").unwrap().is_empty());
}

#[test]
fn concurrent_kimicode_adds_with_one_key_leave_exactly_one_winner() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    let state = create_test_state().unwrap();
    let successes = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..16)
            .map(|index| {
                let state = &state;
                scope.spawn(move || {
                    let mut provider = kimi_provider("same");
                    provider.name = format!("Racer {index}");
                    ProviderService::add(state, AppType::KimiCode, provider, false)
                        .is_ok()
                        .then_some(index)
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(successes.len(), 1);
    assert_eq!(
        state
            .db
            .get_provider_by_id("same", "kimicode")
            .unwrap()
            .unwrap()
            .name,
        format!("Racer {}", successes[0])
    );
}

#[test]
fn kimicode_switch_points_default_model_at_the_selected_provider() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    let state = create_test_state().unwrap();
    let path = kimi_config_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "[providers.\"managed:kimi-code\"]\ntype = \"kimi\"\nbase_url = \"https://api.kimi.com/coding/v1\"\napi_key = \"\"\n\n[models.\"managed:kimi-code/k3\"]\nprovider = \"managed:kimi-code\"\nmodel = \"k3\"\nmax_context_size = 1048576\n",
    )
    .unwrap();

    ProviderService::add(&state, AppType::KimiCode, kimi_provider("first"), true).unwrap();
    assert_eq!(read_default_model(), "cc-switch:first/k3");
    ProviderService::add(&state, AppType::KimiCode, kimi_provider("second"), true).unwrap();
    assert_eq!(read_default_model(), "cc-switch:second/k3");

    ProviderService::switch(&state, AppType::KimiCode, "first").unwrap();
    assert_eq!(read_default_model(), "cc-switch:first/k3");

    // The live default model's owner cannot be removed from the live config.
    assert!(ProviderService::remove_from_live_config(&state, AppType::KimiCode, "first").is_err());
    ProviderService::switch(&state, AppType::KimiCode, "second").unwrap();
    ProviderService::remove_from_live_config(&state, AppType::KimiCode, "first").unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("cc-switch:first"));
    assert_eq!(read_default_model(), "cc-switch:second/k3");
    assert!(state
        .db
        .get_provider_by_id("first", "kimicode")
        .unwrap()
        .is_some());
}

#[test]
fn managed_accounts_stay_in_database_and_cannot_be_written_as_direct_credentials() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    let state = create_test_state().unwrap();
    for kind in ["github_copilot", "codex_oauth", "xai_oauth"] {
        let mut provider = kimi_provider(kind);
        provider.settings_config["api_key"] = json!("MANAGED_AUTH");
        provider.meta = Some(serde_json::from_value(json!({
            "providerType": kind,
            "authBinding": {"source": "managed_account", "authProvider": kind, "accountId": "test-account"}
        })).unwrap());
        ProviderService::add(&state, AppType::KimiCode, provider.clone(), true).unwrap();
        assert!(!kimi_config_path().exists());
        let listed = ProviderService::list(&state, AppType::KimiCode).unwrap();
        assert_eq!(
            listed[kind].meta.as_ref().unwrap().live_config_managed,
            Some(false)
        );
        assert!(ProviderService::switch(&state, AppType::KimiCode, kind).is_err());
        assert!(!kimi_config_path().exists());
    }
}

#[test]
fn existing_kimi_subscription_rows_gain_quota_queries_without_live_config_rewrites() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    let state = create_test_state().unwrap();
    let mut provider = kimi_provider("subscription");
    provider.settings_config["base_url"] = json!("https://api.kimi.com/coding/v1");
    state.db.save_provider("kimicode", &provider).unwrap();
    let rows = ProviderService::list(&state, AppType::KimiCode).unwrap();
    let script = rows["subscription"]
        .meta
        .as_ref()
        .unwrap()
        .usage_script
        .as_ref()
        .unwrap();
    assert!(script.enabled);
    assert_eq!(script.template_type.as_deref(), Some("token_plan"));
    assert!(!kimi_config_path().exists());
    let persisted = state.db.get_all_providers("kimicode").unwrap();
    assert!(persisted["subscription"].has_usage_script_enabled());
}
