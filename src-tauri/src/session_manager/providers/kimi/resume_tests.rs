use super::*;
use crate::mode::state::{Mode, Pending, PendingTarget};

const PROXY_URL: &str = "http://127.0.0.1:15721/kimicode";

fn fixture_config() -> Value {
    json!({
        "default_model": "friendly alias",
        "providers": {
            "provider": {"type":"openai", "name":"Example", "base_url":"https://provider.example/v1", "api_key":"test-key"},
        },
        "models": {
            "friendly alias": {"provider":"provider", "model":"new-model"},
            "custom/nickname": {"provider":"provider", "model":"other-model", "display_name":"Another model"},
            "orphaned": {"provider":"missing", "model":"old-model"},
            "broken\nalias": {"provider":"provider", "model":"old-model"},
            "proxy/orphaned": {"provider":RESERVED_PROXY_TABLE, "model":"former-route"},
        }
    })
}

fn route_config() -> Value {
    let mut config = fixture_config();
    config["default_model"] = json!("cc-switch:proxy/new-model");
    config["providers"][RESERVED_PROXY_TABLE] = json!({
        "type":"anthropic", "name":"CC Switch", "base_url":PROXY_URL, "api_key":PROXY_PLACEHOLDER,
    });
    config["models"]["cc-switch:proxy/new-model"] =
        json!({"provider":RESERVED_PROXY_TABLE, "model":"new-model"});
    config
}

fn route_live() -> AppLiveState {
    AppLiveState {
        mode: Some(Mode::Proxy),
        attached: true,
        proxy_route: Some("route-provider".into()),
        ..AppLiveState::default()
    }
}

#[test]
fn direct_choices_only_include_real_configured_aliases_and_do_not_return_credentials() {
    let options = configured_models(&fixture_config(), &AppLiveState::default(), None).unwrap();
    assert_eq!(options.mode, ResumeMode::Direct);
    assert_eq!(options.models.len(), 2);
    assert_eq!(options.models[0].alias, "friendly alias");
    assert!(options.models[0].is_default);
    assert_eq!(options.models[0].model, "new-model");
    assert_eq!(options.models[1].alias, "custom/nickname");
    assert_eq!(
        options.models[1].display_name.as_deref(),
        Some("Another model")
    );
    let serialized = serde_json::to_string(&options).unwrap();
    assert!(!serialized.contains("test-key"));
    assert!(!serialized.contains("provider.example"));
    assert!(
        configured_models(&json!({}), &AppLiveState::default(), None)
            .unwrap()
            .models
            .is_empty()
    );
}

#[test]
fn route_and_stack_offer_only_projected_models_and_require_the_active_proxy() {
    let mut config = route_config();
    let mut live = route_live();
    let route = configured_models(&config, &live, Some(PROXY_URL)).unwrap();
    assert_eq!(route.mode, ResumeMode::Route);
    assert_eq!(route.models.len(), 1);
    assert_eq!(route.models[0].alias, "cc-switch:proxy/new-model");
    assert_eq!(route.models[0].provider, "route-provider");
    live.stack.enabled = true;
    config["models"]["cc-switch:proxy/ccs-kimi-b--extra"] = json!({
        "provider":RESERVED_PROXY_TABLE,"model":"ccs-kimi-b--extra","display_name":"Extra · Provider B",
    });
    let stack = configured_models(&config, &live, Some(PROXY_URL)).unwrap();
    assert_eq!(stack.mode, ResumeMode::Stack);
    assert_eq!(stack.models.len(), 2);
    assert_eq!(
        stack.models[1].display_name.as_deref(),
        Some("Extra · Provider B")
    );
    for (state, base) in [
        (AppLiveState::default(), Some(PROXY_URL)),
        (
            AppLiveState {
                attached: false,
                ..live.clone()
            },
            Some(PROXY_URL),
        ),
        (live.clone(), Some("http://127.0.0.1:9/kimicode")),
        (live.clone(), None),
        (
            AppLiveState {
                pending: Some(Pending {
                    op: "route".into(),
                    files: Vec::new(),
                    target: PendingTarget::pointer(None),
                    published: false,
                }),
                ..live.clone()
            },
            Some(PROXY_URL),
        ),
    ] {
        assert_eq!(
            configured_models(&config, &state, base).unwrap_err(),
            ResumeError::ProxyNotReady
        );
    }
    config["default_model"] = json!("friendly alias");
    assert_eq!(
        configured_models(&config, &live, Some(PROXY_URL)).unwrap_err(),
        ResumeError::ProxyNotReady
    );
}

#[test]
fn alias_reuse_and_route_changes_require_a_new_selection() {
    let mut config = route_config();
    let mut live = route_live();
    let options = configured_models(&config, &live, Some(PROXY_URL)).unwrap();
    let selection = ModelSelection {
        alias: options.models[0].alias.clone(),
        revision: options.models[0].revision.clone(),
    };
    let context = |options| ResumeContext {
        home: PathBuf::from("home"),
        cwd: PathBuf::from("cwd"),
        session_id: "id".into(),
        options,
    };
    assert!(context(options).selected_model(&selection).is_ok());
    live.proxy_route = Some("new-route".into());
    assert_eq!(
        context(configured_models(&config, &live, Some(PROXY_URL)).unwrap())
            .selected_model(&selection)
            .unwrap_err(),
        ResumeError::ModelChanged
    );
    config["models"]
        .as_object_mut()
        .unwrap()
        .remove(&selection.alias);
    assert_eq!(
        context(configured_models(&config, &live, Some(PROXY_URL)).unwrap())
            .selected_model(&selection)
            .unwrap_err(),
        ResumeError::ModelChanged
    );
}

fn session_fixture() -> (tempfile::TempDir, ResumeTarget, PathBuf, PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let cwd = home.path().join("user's workspace");
    fs::create_dir(&cwd).unwrap();
    let dir = home.path().join("sessions/wd_fixture/old-id");
    let wire = dir.join("agents/main/wire.jsonl");
    fs::create_dir_all(wire.parent().unwrap()).unwrap();
    fs::write(&wire, b"{\"type\":\"metadata\"}\n").unwrap();
    let state_path = dir.join("state.json");
    fs::write(&state_path, json!({"id":"old-id","cwd":cwd}).to_string()).unwrap();
    let target = ResumeTarget {
        session_id: "old-id".into(),
        source_path: wire.to_str().unwrap().into(),
    };
    (home, target, cwd, state_path)
}

#[test]
fn target_is_read_only_and_bound_to_the_original_session_and_cwd() {
    let (home, mut target, cwd, state_path) = session_fixture();
    let original_state = fs::read(&state_path).unwrap();
    let original_wire = fs::read(&target.source_path).unwrap();
    let (real_home, dir) = read_target(home.path(), &target).unwrap();
    assert_eq!(real_home, home.path().canonicalize().unwrap());
    assert_eq!(dir, cwd);
    assert_eq!(fs::read(&state_path).unwrap(), original_state);
    assert_eq!(fs::read(&target.source_path).unwrap(), original_wire);
    target.session_id = "another-id".into();
    assert_eq!(
        read_target(home.path(), &target),
        Err(ResumeError::InvalidSession)
    );
    target.session_id = "old-id".into();
    fs::remove_dir(&cwd).unwrap();
    assert_eq!(
        read_target(home.path(), &target),
        Err(ResumeError::MissingProject)
    );
    fs::write(&state_path, json!({"id":"wrong-id","cwd":cwd}).to_string()).unwrap();
    assert_eq!(
        read_target(home.path(), &target),
        Err(ResumeError::InvalidSession)
    );
}

#[cfg(unix)]
#[test]
fn target_refuses_linked_metadata_and_wire_files() {
    let (home, target, _, state_path) = session_fixture();
    let moved = home.path().join("outside-state.json");
    fs::rename(&state_path, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &state_path).unwrap();
    assert_eq!(
        read_target(home.path(), &target),
        Err(ResumeError::InvalidSession)
    );
    fs::remove_file(&state_path).unwrap();
    fs::rename(&moved, &state_path).unwrap();
    let moved = home.path().join("outside-wire.jsonl");
    fs::rename(&target.source_path, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &target.source_path).unwrap();
    assert_eq!(
        read_target(home.path(), &target),
        Err(ResumeError::InvalidSession)
    );
}

#[test]
fn reading_resume_mode_never_recovers_or_moves_bad_state() {
    let tmp = tempfile::tempdir().unwrap();
    let store = DeviceStore::at(tmp.path());
    let path = store.state_path();
    fs::write(&path, b"invalid JSON").unwrap();
    assert!(state::load_read_only(&store).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"invalid JSON");
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn copied_posix_command_keeps_alias_home_session_and_cwd_as_literal_arguments() {
    use std::os::unix::fs::PermissionsExt;
    let bin = tempfile::tempdir().unwrap();
    let stub = bin.path().join("kimi");
    fs::write(
        &stub,
        b"#!/bin/sh\nprintf '%s\\0' \"$KIMI_CODE_HOME\" \"$PWD\" \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700)).unwrap();
    let (home, target, _, _) = session_fixture();
    let (home, cwd) = read_target(home.path(), &target).unwrap();
    let alias = "provider/it's $HOME \"literal\"";
    let mut config = fixture_config();
    config["models"][alias] = json!({"provider":"provider","model":"real-upstream"});
    for kind in [ResumeKind::Original, ResumeKind::New] {
        let options = configured_models(&config, &AppLiveState::default(), None).unwrap();
        let revision = options
            .models
            .iter()
            .find(|model| model.alias == alias)
            .unwrap()
            .revision
            .clone();
        let context = ResumeContext {
            home: home.clone(),
            cwd: cwd.clone(),
            session_id: target.session_id.clone(),
            options,
        };
        let ResumeOutcome::Command { command } = context
            .execute(
                ModelSelection {
                    alias: alias.into(),
                    revision,
                },
                kind,
                ResumeAction::Copy,
                "unused",
            )
            .unwrap()
        else {
            panic!("expected a copyable command");
        };
        let path =
            std::env::join_paths([bin.path(), Path::new("/usr/bin"), Path::new("/bin")]).unwrap();
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &command])
            .env("PATH", path)
            .output()
            .unwrap();
        assert!(output.status.success());
        let actual = String::from_utf8(output.stdout).unwrap();
        let actual = actual.split_terminator('\0').collect::<Vec<_>>();
        let alias_arg = format!("--model={alias}");
        let canonical_cwd = cwd.canonicalize().unwrap();
        let mut expected = vec![home.to_str().unwrap(), canonical_cwd.to_str().unwrap()];
        if matches!(kind, ResumeKind::Original) {
            expected.extend(["--session", "old-id"]);
        }
        expected.push(&alias_arg);
        // Some shells retain the original symlink spelling in PWD on Darwin.
        assert_eq!(Path::new(actual[1]).canonicalize().unwrap(), canonical_cwd);
        assert_eq!(actual[0], expected[0]);
        assert_eq!(actual[2..], expected[2..]);
    }
}

#[test]
fn copied_powershell_uses_literal_directory_and_restores_environment() {
    let command = powershell_copy_command(
        r"C:\it's home",
        r"C:\[project]",
        Some("old-id"),
        "provider/o’b's $literal",
    );
    assert!(command.contains(r"Push-Location -LiteralPath 'C:\[project]' -ErrorAction Stop"));
    assert!(command.contains(r"$env:KIMI_CODE_HOME = 'C:\it''s home'"));
    assert!(command.contains("& kimi --session 'old-id' '--model=provider/o’’b''s $literal'"));
    assert!(command.contains("finally { $env:KIMI_CODE_HOME = $oldHome; Pop-Location }"));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
#[ignore = "requires an installed Kimi Code CLI and loopback networking"]
async fn generated_command_resumes_a_real_cli_session_after_old_alias_removal() {
    use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};
    let cli = PathBuf::from(
        std::env::var_os("CC_SWITCH_KIMI_BINARY")
            .expect("set CC_SWITCH_KIMI_BINARY to an installed Kimi executable"),
    )
    .canonicalize()
    .unwrap();
    let home = crate::test_support::TestHome::new();
    let native_home = home.path().join("isolated Kimi home");
    let _env = crate::test_support::EnvVars::new().set("KIMI_CODE_HOME", &native_home);
    fs::create_dir(&native_home).unwrap();
    let cwd = home.path().join("original project's folder");
    fs::create_dir(&cwd).unwrap();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let launcher = bin.join("kimi");
    fs::write(
        &launcher,
        format!(
            "#!/bin/sh\nexec {} \"$@\"\n",
            shell_escape(cli.to_str().unwrap())
        ),
    )
    .unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
    let path =
        std::env::join_paths([bin.as_path(), Path::new("/usr/bin"), Path::new("/bin")]).unwrap();

    let requests = Arc::new(tokio::sync::Mutex::new(Vec::<Value>::new()));
    let capture = requests.clone();
    let server = axum::Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let capture = capture.clone();
        async move {
            let model = body["model"].as_str().unwrap().to_string();
            capture.lock().await.push(body);
            let mut stream = String::new();
            for chunk in [
                json!({"id":"fixture","object":"chat.completion.chunk","model":model,"choices":[{"index":0,"delta":{"role":"assistant","content":format!("reply from {model}")},"finish_reason":null}]}),
                json!({"id":"fixture","object":"chat.completion.chunk","model":model,"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":4,"total_tokens":16}}),
            ] {
                stream.push_str(&format!("data: {chunk}\n\n"));
            }
            stream.push_str("data: [DONE]\n\n");
            ([(axum::http::header::CONTENT_TYPE, "text/event-stream")], stream)
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let serving = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
    let replacement = "provider/new model's alias";
    let config = format!(
        r#"telemetry = false
 default_model = "unchanged-default"
 [providers.fixture]
 type = "openai"
 base_url = "http://{address}/v1"
 api_key = "local-fixture-key"
 [models.unchanged-default]
 provider = "fixture"
 model = "default-upstream"
 max_context_size = 65536
 [models."{replacement}"]
 provider = "fixture"
 model = "replacement-upstream"
 max_context_size = 65536
"#
    );
    let config_path = native_home.join("config.toml");
    fs::write(&config_path, format!("{config}\n[models.retired]\nprovider = \"fixture\"\nmodel = \"old-upstream\"\nmax_context_size = 65536\n")).unwrap();
    let run = |command: String, prompt: &str| {
        let mut child = tokio::process::Command::new("/bin/sh");
        child
            .args([
                "-c",
                &format!(
                    "{command} --output-format stream-json --prompt {}",
                    shell_escape(prompt)
                ),
            ])
            .env_clear()
            .env("PATH", &path)
            .env("HOME", home.path())
            .env("KIMI_CODE_HOME", &native_home)
            .env("KIMI_DISABLE_TELEMETRY", "1")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .current_dir(&cwd)
            .kill_on_drop(true);
        async move {
            let output = tokio::time::timeout(Duration::from_secs(30), child.output())
                .await
                .unwrap()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8(output.stdout).unwrap();
            stdout
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .find(|row| row["type"] == "session.resume_hint")
                .and_then(|row| row["session_id"].as_str().map(str::to_string))
                .expect("CLI resume ID")
        }
    };
    let session_id = run(
        "kimi --model retired".into(),
        "first historical prompt in the fixture",
    )
    .await;
    let session = super::super::scan_sessions()
        .into_iter()
        .find(|item| item.session_id == session_id)
        .expect("CC Switch sees the native session");
    let source_path = session.source_path.unwrap();
    let original = fs::read(&source_path).unwrap();
    fs::write(&config_path, &config).unwrap();
    let app = AppState::new(Arc::new(crate::database::Database::memory().unwrap()));
    let context = prepare(
        &app,
        ResumeTarget {
            session_id: session_id.clone(),
            source_path: source_path.clone(),
        },
    )
    .await
    .unwrap();
    let selected = context
        .options
        .models
        .iter()
        .find(|item| item.alias == replacement)
        .unwrap();
    let selection = ModelSelection {
        alias: selected.alias.clone(),
        revision: selected.revision.clone(),
    };
    assert!(context
        .options
        .models
        .iter()
        .all(|item| item.alias != "retired"));
    let ResumeOutcome::Command { command } = context
        .execute(
            selection,
            ResumeKind::Original,
            ResumeAction::Copy,
            "unused",
        )
        .unwrap()
    else {
        panic!("copy command")
    };
    assert_eq!(fs::read(&source_path).unwrap(), original);
    assert_eq!(fs::read_to_string(&config_path).unwrap(), config);
    assert_eq!(
        run(command, "continue the same fixture session").await,
        session_id
    );
    let requests = requests.lock().await;
    assert_eq!(requests[0]["model"], "old-upstream");
    assert_eq!(requests.last().unwrap()["model"], "replacement-upstream");
    let messages = requests.last().unwrap()["messages"].to_string();
    assert!(messages.contains("first historical prompt in the fixture"));
    assert!(messages.contains("reply from old-upstream"));
    assert_eq!(
        kimi_config::read(&config_path).unwrap()["default_model"],
        "unchanged-default"
    );
    serving.abort();
}
