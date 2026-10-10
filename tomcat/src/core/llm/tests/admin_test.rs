use std::fs;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use fs2::FileExt;
use serial_test::serial;

use crate::core::llm::thinking_policy::ThinkingFormat;
use crate::core::llm::{
    auth::clear_managed_credentials_for_test, list_model_views, list_model_views_with_prefs,
    list_provider_keys, remove_user_model, remove_user_model_with_config_path, set_provider_key,
    upsert_user_model, Capabilities, DefaultLlmResolver, LlmResolver, LlmScene, ModelCatalog,
    ModelEntryInput, ModelSource, ProviderKeyInput, SharedModelCatalog,
};
use crate::infra::config::AppConfig;
use crate::{resolve_sessions_dir, ModelPrefsStore, SessionEntry, SessionStore, ThinkingLevel};

fn temp_cfg() -> (tempfile::TempDir, AppConfig) {
    let work_dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().into_owned());
    (work_dir, cfg)
}

fn model_prefs(path: &std::path::Path) -> Arc<ModelPrefsStore> {
    Arc::new(
        ModelPrefsStore::load(path.join("model-thinking.json"), ThinkingLevel::Medium)
            .expect("model preferences"),
    )
}

fn custom_claude_input() -> ModelEntryInput {
    ModelEntryInput {
        id: "custom-claude".to_string(),
        model_name: Some("claude-opus-4-6".to_string()),
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        api_key_env: Some("ADMIN_TEST_ANTHROPIC_API_KEY".to_string()),
        base_url: Some("https://api.anthropic.com/v1".to_string()),
        capabilities: Capabilities::default(),
        context_window: None,
        context_window_options: None,
        max_output_tokens: Some(128_000),
        description: None,
        supported_speeds: None,
        supported_reasoning_levels: None,
        thinking_format: Some("anthropic".to_string()),
    }
}

#[test]
#[serial(env_lock)]
fn upsert_list_and_remove_user_model_roundtrip() {
    clear_managed_credentials_for_test();
    let (_work_dir, cfg) = temp_cfg();
    let capabilities = Capabilities {
        tools: true,
        reasoning: true,
        ..Default::default()
    };
    let input = ModelEntryInput {
        id: "custom-openai".to_string(),
        model_name: Some("gpt-5.4".to_string()),
        api: "openai-responses".to_string(),
        provider: "openai".to_string(),
        api_key_env: Some("ADMIN_TEST_OPENAI_KEY".to_string()),
        base_url: Some("https://gateway.example.test/v1".to_string()),
        capabilities,
        context_window: Some(256_000),
        context_window_options: None,
        max_output_tokens: Some(128_000),
        description: None,
        supported_speeds: None,
        supported_reasoning_levels: None,
        thinking_format: Some("openai".to_string()),
    };

    let view = upsert_user_model(&cfg, input).expect("upsert user model");
    assert_eq!(view.model.source, ModelSource::User);
    assert_eq!(view.model.api_key_env, "ADMIN_TEST_OPENAI_KEY");
    assert!(!view.model.key_present);
    assert_eq!(view.model.model_name.as_deref(), Some("gpt-5.4"));
    assert!(view.warnings.is_empty());

    let catalog = ModelCatalog::load(&cfg).expect("reload catalog");
    let views = list_model_views(&catalog);
    let custom = views
        .iter()
        .find(|entry| entry.id == "custom-openai")
        .expect("custom model in list");
    assert_eq!(custom.source, ModelSource::User);
    assert_eq!(
        custom.base_url.as_deref(),
        Some("https://gateway.example.test/v1")
    );

    remove_user_model(&cfg, "custom-openai").expect("remove custom model");
    let catalog = ModelCatalog::load(&cfg).expect("reload catalog after remove");
    assert!(catalog.lookup("custom-openai").is_none());
}

#[test]
fn speed_admin_roundtrip_keeps_all_eight_synthetic_declarations() {
    use crate::core::llm::Speed;
    let (work, cfg) = temp_cfg();
    let prefs = model_prefs(work.path());
    for (i, speeds) in [
        vec![Speed::Fast, Speed::Ultrafast],
        vec![Speed::Fast],
        vec![Speed::Fast],
        vec![Speed::Fast],
        vec![Speed::Fast, Speed::Ultrafast],
        vec![Speed::Fast],
        vec![Speed::Fast],
        vec![Speed::Fast],
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("relay/synthetic-{i}");
        let mut input = custom_claude_input();
        input.id = id.clone();
        input.model_name = Some(format!("synthetic-{i}"));
        input.api = "openai-responses".into();
        input.thinking_format = Some("openai".into());
        input.supported_speeds = Some(speeds.clone());
        input.supported_reasoning_levels = Some(vec!["high".into(), "xhigh".into(), "max".into()]);
        let view = upsert_user_model(&cfg, input.clone()).unwrap().model;
        assert_eq!(view.supported_speeds, speeds);
        upsert_user_model(
            &cfg,
            ModelEntryInput {
                description: Some("edited".into()),
                ..input
            },
        )
        .unwrap();
        prefs.set_speed(&id, Speed::Ultrafast).unwrap();
        let catalog = ModelCatalog::load(&cfg).unwrap();
        let reloaded = list_model_views_with_prefs(&catalog, &prefs)
            .into_iter()
            .find(|m| m.id == id)
            .unwrap();
        assert_eq!(reloaded.supported_speeds, speeds);
        assert_eq!(
            reloaded.selected_speed,
            Some(if speeds.contains(&Speed::Ultrafast) {
                Speed::Ultrafast
            } else {
                Speed::Standard
            })
        );
        assert!(reloaded.supported_reasoning_levels.contains(&"max".into()));
    }
}

#[test]
fn upsert_user_model_rejects_speeds_on_non_openai_api() {
    let (work, cfg) = temp_cfg();
    upsert_user_model(&cfg, custom_claude_input()).unwrap();
    let path = work.path().join("models.toml");
    let original = fs::read(&path).unwrap();
    let error = upsert_user_model(
        &cfg,
        ModelEntryInput {
            supported_speeds: Some(vec![crate::core::llm::Speed::Fast]),
            ..custom_claude_input()
        },
    )
    .expect_err("non-OpenAI speed declaration must be rejected before writing");
    assert!(error.to_string().contains("supported_speeds"));
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn upsert_and_remove_keep_unknown_model_fields() {
    use crate::core::llm::Speed;
    let (work, cfg) = temp_cfg();
    let path = work.path().join("models.toml");
    fs::write(
        &path,
        r#"
[[models]]
id = "model-a"
api = "openai-responses"
provider = "openai"
supported_speeds = ["fast"]
future_field = "x"
future_options = { enabled = true, weights = [1, 2], nested = { label = "keep" } }

[[models]]
id = "model-b"
api = "anthropic-messages"
provider = "anthropic"
future_flag = false
"#,
    )
    .unwrap();
    let original: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let assert_preserved = || {
        let value: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let models = value["models"].as_array().unwrap();
        let a = models
            .iter()
            .find(|m| m["id"].as_str() == Some("model-a"))
            .unwrap();
        let b = models
            .iter()
            .find(|m| m["id"].as_str() == Some("model-b"))
            .unwrap();
        assert_eq!(a["future_field"], original["models"][0]["future_field"]);
        assert_eq!(a["future_options"], original["models"][0]["future_options"]);
        assert_eq!(
            a["supported_speeds"],
            original["models"][0]["supported_speeds"]
        );
        assert_eq!(b["future_flag"], original["models"][1]["future_flag"]);
    };
    upsert_user_model(
        &cfg,
        ModelEntryInput {
            id: "model-c".into(),
            ..custom_claude_input()
        },
    )
    .unwrap();
    assert_preserved();
    upsert_user_model(
        &cfg,
        ModelEntryInput {
            id: "model-b".into(),
            description: Some("edited B".into()),
            ..custom_claude_input()
        },
    )
    .unwrap();
    assert_preserved();
    remove_user_model(&cfg, "model-c").unwrap();
    assert_preserved();
    upsert_user_model(
        &cfg,
        ModelEntryInput {
            id: "model-a".into(),
            api: "openai-responses".into(),
            thinking_format: Some("openai".into()),
            supported_speeds: Some(vec![Speed::Fast]),
            description: Some("edited A".into()),
            ..custom_claude_input()
        },
    )
    .unwrap();
    assert_preserved();
    let catalog = ModelCatalog::load(&cfg).unwrap();
    assert_eq!(
        catalog.lookup("model-a").unwrap().description.as_deref(),
        Some("edited A")
    );
    assert_eq!(
        catalog.lookup("model-b").unwrap().description.as_deref(),
        Some("edited B")
    );
    assert!(catalog.lookup("model-c").is_none());
}

#[test]
fn model_views_resolve_defaults_for_unselected_preferences() {
    let (work_dir, cfg) = temp_cfg();
    let catalog = ModelCatalog::load(&cfg).expect("load catalog");
    let prefs = ModelPrefsStore::load(
        work_dir.path().join("model-thinking.json"),
        ThinkingLevel::Medium,
    )
    .expect("model preferences");

    let model = list_model_views_with_prefs(&catalog, &prefs)
        .into_iter()
        .find(|model| model.id == "gpt-5.4")
        .expect("builtin model");

    assert_eq!(model.selected_reasoning_level.as_deref(), Some("medium"));
    assert_eq!(model.selected_context_window, Some(400_000));
}

#[test]
fn model_views_clamp_invalid_persisted_preferences_to_supported_defaults() {
    let (work_dir, cfg) = temp_cfg();
    let catalog = ModelCatalog::load(&cfg).expect("load catalog");
    let prefs = ModelPrefsStore::load(
        work_dir.path().join("model-thinking.json"),
        ThinkingLevel::Max,
    )
    .expect("model preferences");
    prefs
        .set_reasoning("gpt-5.4", ThinkingLevel::Max)
        .expect("persist reasoning");
    prefs
        .set_context_window("gpt-5.4", Some(123_456))
        .expect("persist context window");

    let model = list_model_views_with_prefs(&catalog, &prefs)
        .into_iter()
        .find(|model| model.id == "gpt-5.4")
        .expect("builtin model");

    assert_eq!(model.selected_reasoning_level.as_deref(), Some("xhigh"));
    assert_eq!(model.selected_context_window, Some(400_000));
}

#[test]
#[serial(env_lock)]
fn list_model_views_keep_seeded_builtin_entries_as_builtin_sources() {
    clear_managed_credentials_for_test();
    let (_work_dir, cfg) = temp_cfg();
    let path = ModelCatalog::default_user_path(&cfg).expect("models.toml path");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "gpt-5.4"
base_url = "https://gateway.example.test/v1"

[[models]]
id = "claude-opus-gateway"
api = "anthropic-messages"
provider = "relay"
"#,
    )
    .expect("seed models.toml");

    let catalog = ModelCatalog::load(&cfg).expect("load catalog");
    let views = list_model_views(&catalog);
    let seeded_builtin = views
        .iter()
        .find(|entry| entry.id == "gpt-5.4")
        .expect("seeded builtin view");
    let custom = views
        .iter()
        .find(|entry| entry.id == "claude-opus-gateway")
        .expect("custom relay view");

    assert_eq!(seeded_builtin.source, ModelSource::Builtin);
    assert_eq!(custom.source, ModelSource::User);
}

#[test]
#[serial(env_lock)]
fn remove_user_model_can_drop_seeded_builtin_override_then_fall_back_to_builtin() {
    clear_managed_credentials_for_test();
    let (_work_dir, mut cfg) = temp_cfg();
    cfg.llm.default_model = "gpt-5.2".to_string();
    let path = ModelCatalog::default_user_path(&cfg).expect("models.toml path");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "gpt-5.4"
base_url = "https://gateway.example.test/v1"
"#,
    )
    .expect("seed builtin override");

    remove_user_model(&cfg, "gpt-5.4").expect("remove seeded builtin override");

    let catalog = ModelCatalog::load(&cfg).expect("reload catalog after remove");
    let seeded_builtin = list_model_views(&catalog)
        .into_iter()
        .find(|entry| entry.id == "gpt-5.4")
        .expect("builtin gpt-5.4 remains available");
    assert_eq!(seeded_builtin.source, ModelSource::Builtin);
    assert!(!catalog.is_user_model("gpt-5.4"));
    assert!(catalog.is_builtin_seed("gpt-5.4"));

    let error = remove_user_model(&cfg, "gpt-5.4").expect_err("pure builtin remove must fail");
    assert!(
        error.to_string().contains(&crate::infra::i18n::tr_in(
            crate::infra::i18n::Locale::En,
            "model.builtinDelete",
            &[("id", "gpt-5.4")]
        )),
        "unexpected error: {error}"
    );
}

#[test]
#[serial(env_lock)]
fn upsert_user_model_accepts_id_with_slash() {
    clear_managed_credentials_for_test();
    let (_work_dir, cfg) = temp_cfg();
    let input = ModelEntryInput {
        id: "chatanywhere/gpt-5.4".to_string(),
        model_name: Some("gpt-5.4".to_string()),
        api: "openai-responses".to_string(),
        provider: "chatanywhere".to_string(),
        api_key_env: Some("CHATANYWHERE_API_KEY".to_string()),
        base_url: Some("https://api.chatanywhere.tech/v1".to_string()),
        capabilities: Capabilities {
            tools: true,
            reasoning: true,
            ..Default::default()
        },
        context_window: None,
        context_window_options: None,
        max_output_tokens: None,
        description: None,
        supported_speeds: None,
        supported_reasoning_levels: None,
        thinking_format: None,
    };

    let view = upsert_user_model(&cfg, input).expect("slash id should be accepted");
    assert_eq!(view.model.id, "chatanywhere/gpt-5.4");

    let catalog = ModelCatalog::load(&cfg).expect("reload catalog");
    let custom = catalog
        .lookup("chatanywhere/gpt-5.4")
        .expect("model with slash id should roundtrip");
    assert_eq!(custom.provider, "chatanywhere");
}

#[test]
#[serial(env_lock)]
fn upsert_user_model_collects_warning_for_openai_api_with_non_effort_format() {
    clear_managed_credentials_for_test();
    let (_work_dir, cfg) = temp_cfg();
    let view = upsert_user_model(
        &cfg,
        ModelEntryInput {
            id: "relay-openai".to_string(),
            model_name: Some("gpt-5.4".to_string()),
            api: "openai-responses".to_string(),
            provider: "relay".to_string(),
            api_key_env: Some("RELAY_API_KEY".to_string()),
            base_url: Some("https://gateway.example.test/v1".to_string()),
            capabilities: Capabilities {
                tools: true,
                reasoning: true,
                ..Default::default()
            },
            context_window: None,
            context_window_options: None,
            max_output_tokens: None,
            description: None,
            supported_speeds: None,
            supported_reasoning_levels: None,
            thinking_format: Some("anthropic".to_string()),
        },
    )
    .expect("upsert mismatched format");

    assert_eq!(view.model.thinking_format.as_deref(), Some("anthropic"));
    assert_eq!(view.warnings.len(), 1);
    assert!(
        view.warnings[0].contains("openai-responses") && view.warnings[0].contains("anthropic"),
        "warning should explain the api/format mismatch, got: {:?}",
        view.warnings
    );
}

#[test]
#[serial(env_lock)]
fn set_provider_key_persists_env_and_flips_key_presence() {
    clear_managed_credentials_for_test();
    let (work_dir, cfg) = temp_cfg();
    upsert_user_model(&cfg, custom_claude_input()).expect("seed custom model");

    let status = set_provider_key(
        &cfg,
        ProviderKeyInput {
            env_name: "ADMIN_TEST_ANTHROPIC_API_KEY".to_string(),
            value: "super-secret".to_string(),
        },
    )
    .expect("persist provider key");
    assert_eq!(status.env_name, "ADMIN_TEST_ANTHROPIC_API_KEY");
    assert!(status.key_present);

    let env_path = work_dir.path().join("assets").join(".env");
    let env_text = fs::read_to_string(&env_path).expect("read .env");
    assert!(
        env_text.contains("ADMIN_TEST_ANTHROPIC_API_KEY=super-secret"),
        "expected persisted env entry, got: {env_text}"
    );
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&env_path)
            .expect("env metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    let catalog = ModelCatalog::load(&cfg).expect("reload catalog");
    let custom = list_model_views(&catalog)
        .into_iter()
        .find(|entry| entry.id == "custom-claude")
        .expect("custom claude view");
    assert!(custom.key_present);

    let key_view = list_provider_keys(&cfg)
        .expect("list provider keys")
        .into_iter()
        .find(|entry| entry.env_name == "ADMIN_TEST_ANTHROPIC_API_KEY")
        .expect("provider key view");
    assert!(key_view.key_present);
    assert!(key_view.provider.is_empty());
    assert!(key_view.model_ids.is_empty());
}

#[test]
#[serial(env_lock)]
fn list_provider_keys_reloads_env_and_uses_env_as_the_only_inventory() {
    clear_managed_credentials_for_test();
    let (work_dir, cfg) = temp_cfg();
    upsert_user_model(&cfg, custom_claude_input()).expect("seed model-only key reference");
    let env_path = work_dir.path().join("assets").join(".env");
    std::fs::create_dir_all(env_path.parent().expect("env parent")).expect("mkdir assets");
    std::fs::write(
        &env_path,
        "FCODEX_OPENAI_API_KEY=secret-openai\nFCODEX_ANTHROPIC_API_KEY=secret-anthropic\nNOT_A_CREDENTIAL=value\nlower_API_KEY=value\nEMPTY_API_KEY=\n",
    )
    .expect("seed env file");

    let first = list_provider_keys(&cfg).expect("first key inventory");
    assert_eq!(
        first
            .iter()
            .map(|entry| entry.env_name.as_str())
            .collect::<Vec<_>>(),
        vec!["FCODEX_ANTHROPIC_API_KEY", "FCODEX_OPENAI_API_KEY"]
    );
    assert!(first.iter().all(|entry| entry.key_present));
    assert!(first.iter().all(|entry| entry.provider.is_empty()));
    assert!(first.iter().all(|entry| entry.model_ids.is_empty()));
    let serialized = serde_json::to_string(&first).expect("serialize key inventory");
    assert!(!serialized.contains("secret-openai"));
    assert!(!serialized.contains("secret-anthropic"));
    assert!(!serialized.contains("ADMIN_TEST_ANTHROPIC_API_KEY"));

    std::fs::write(&env_path, "FCODEX_ANTHROPIC_API_KEY=rotated\n")
        .expect("replace env file externally");
    let second = list_provider_keys(&cfg).expect("reloaded key inventory");
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].env_name, "FCODEX_ANTHROPIC_API_KEY");

    let catalog = ModelCatalog::load(&cfg).expect("reload catalog after credential refresh");
    let custom = list_model_views(&catalog)
        .into_iter()
        .find(|entry| entry.id == "custom-claude")
        .expect("custom model");
    assert!(
        !custom.key_present,
        "removed model key must stop being present"
    );
}

#[test]
#[serial(env_lock)]
fn model_and_key_writes_reject_invalid_environment_variable_names() {
    clear_managed_credentials_for_test();
    let (_work_dir, cfg) = temp_cfg();

    let mut invalid_model = custom_claude_input();
    invalid_model.api_key_env = Some("bad-key\nname".to_string());
    let model_error = upsert_user_model(&cfg, invalid_model).expect_err("invalid model key env");
    assert!(model_error.to_string().contains("^[A-Z_][A-Z0-9_]*$"));

    for env_name in ["lower_API_KEY", "9STARTS_WITH_DIGIT", "BAD-KEY", "BAD\nKEY"] {
        let error = set_provider_key(
            &cfg,
            ProviderKeyInput {
                env_name: env_name.to_string(),
                value: "secret".to_string(),
            },
        )
        .expect_err("invalid provider key env");
        assert!(error.to_string().contains("^[A-Z_][A-Z0-9_]*$"));
    }
}

#[test]
#[serial(env_lock)]
fn set_provider_key_preserves_other_env_entries() {
    clear_managed_credentials_for_test();
    let (work_dir, cfg) = temp_cfg();
    upsert_user_model(&cfg, custom_claude_input()).expect("seed custom model");

    let env_path = work_dir.path().join("assets").join(".env");
    std::fs::create_dir_all(env_path.parent().expect("env parent")).expect("mkdir assets");
    std::fs::write(
        &env_path,
        "OTHER_KEEP=keep-me\nHTTPS_PROXY=http://127.0.0.1:9999\n",
    )
    .expect("seed env file");

    set_provider_key(
        &cfg,
        ProviderKeyInput {
            env_name: "ADMIN_TEST_ANTHROPIC_API_KEY".to_string(),
            value: "rotated-secret".to_string(),
        },
    )
    .expect("persist provider key");

    let env_text = fs::read_to_string(&env_path).expect("read .env");
    assert!(env_text.contains("OTHER_KEEP=keep-me"));
    assert!(env_text.contains("ADMIN_TEST_ANTHROPIC_API_KEY=rotated-secret"));
    assert!(env_text.contains("HTTPS_PROXY=http://127.0.0.1:9999"));
}

#[test]
#[serial(env_lock)]
fn set_provider_key_waits_for_env_lock_and_then_succeeds() {
    clear_managed_credentials_for_test();
    let (_work_dir, cfg) = temp_cfg();
    upsert_user_model(&cfg, custom_claude_input()).expect("seed custom model");

    let env_lock_path = cfg
        .storage
        .work_dir
        .as_ref()
        .map(|path| std::path::Path::new(path).join("assets").join(".env.lock"))
        .expect("work dir");
    std::fs::create_dir_all(env_lock_path.parent().expect("lock parent")).expect("mkdir assets");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(&env_lock_path)
        .expect("open env lock");
    lock_file.lock_exclusive().expect("lock env file");

    let cfg_for_thread = cfg.clone();
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = set_provider_key(
            &cfg_for_thread,
            ProviderKeyInput {
                env_name: "ADMIN_TEST_ANTHROPIC_API_KEY".to_string(),
                value: "after-lock".to_string(),
            },
        );
        tx.send(result).expect("send result");
    });

    std::thread::sleep(Duration::from_millis(200));
    assert!(
        rx.try_recv().is_err(),
        "lock should block concurrent writer"
    );

    lock_file.unlock().expect("unlock env file");
    let status = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("worker result")
        .expect("set provider key after unlock");
    worker.join().expect("join worker");

    assert!(status.key_present);
}

#[test]
#[serial(env_lock)]
fn remove_user_model_forgets_config_and_all_session_overrides() {
    clear_managed_credentials_for_test();
    let (work_dir, mut cfg) = temp_cfg();
    upsert_user_model(&cfg, custom_claude_input()).expect("seed custom model");
    cfg.llm.default_model = "custom-claude".to_string();
    cfg.llm.vision_model = Some("custom-claude".to_string());
    cfg.llm.title_model = Some("custom-claude".to_string());
    cfg.context.compaction_model = "custom-claude".to_string();
    let config_path = work_dir.path().join("tomcat.config.toml");
    fs::write(
        &config_path,
        toml::to_string_pretty(&cfg).expect("serialize config"),
    )
    .expect("write config");

    let entry = SessionEntry {
        session_key: "agent:main:main".to_string(),
        session_id: "session-1".to_string(),
        updated_at: 1,
        session_file: None,
        cwd: Some("/workspace".to_string()),
        project_root: None,
        thinking_level: None,
        model_override: Some("custom-claude".to_string()),
        input_tokens: Some(10),
        output_tokens: Some(20),
        compaction_count: None,
        compaction_tokens_freed: None,
        tool_result_chars_persisted: None,
        context_utilization_ratio: None,
        last_checkpoint_id: None,
        title: Some("Pinned Claude Session".to_string()),
    };
    let store = SessionStore {
        current: [("agent:main:main".to_string(), "session-1".to_string())]
            .into_iter()
            .collect(),
        sessions: [("session-1".to_string(), entry)].into_iter().collect(),
    };
    let mut document = serde_json::to_value(&store).expect("serialize store");
    document.as_object_mut().expect("store root").insert(
        "futureRoot".to_string(),
        serde_json::json!({ "keep": true }),
    );
    document["sessions"]["session-1"]
        .as_object_mut()
        .expect("session object")
        .insert("futureField".to_string(), serde_json::json!("keep me"));

    let main_store = resolve_sessions_dir(&cfg)
        .expect("main sessions dir")
        .join("sessions.json");
    let reviewer_store = work_dir
        .path()
        .join("agents")
        .join("reviewer")
        .join("sessions")
        .join("sessions.json");
    for path in [&main_store, &reviewer_store] {
        std::fs::create_dir_all(path.parent().expect("store parent")).expect("mkdir store parent");
        std::fs::write(
            path,
            serde_json::to_string_pretty(&document).expect("render store"),
        )
        .expect("write store");
    }

    remove_user_model_with_config_path(&cfg, Some(&config_path), "custom-claude")
        .expect("remove model and clear references");

    let persisted = crate::load_config_toml_file(&config_path).expect("read cleaned config");
    assert_eq!(persisted.llm.default_model, "");
    assert_eq!(persisted.context.compaction_model, "");
    assert_eq!(persisted.llm.vision_model.as_deref(), Some(""));
    assert_eq!(persisted.llm.title_model.as_deref(), Some(""));
    for path in [&main_store, &reviewer_store] {
        let cleaned: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("read cleaned store"))
                .expect("parse cleaned store");
        assert!(cleaned["sessions"]["session-1"]
            .get("modelOverride")
            .is_none());
        assert_eq!(
            cleaned["sessions"]["session-1"]["title"],
            "Pinned Claude Session"
        );
        assert_eq!(cleaned["sessions"]["session-1"]["futureField"], "keep me");
        assert_eq!(cleaned["futureRoot"]["keep"], true);
    }
    assert!(
        ModelCatalog::load(&cfg)
            .expect("reload catalog")
            .lookup("custom-claude")
            .is_none(),
        "models.toml is committed only after the reference cleanup"
    );
}

#[test]
#[serial(env_lock)]
fn remove_user_model_does_not_touch_files_when_session_precheck_fails() {
    clear_managed_credentials_for_test();
    let (work_dir, mut cfg) = temp_cfg();
    upsert_user_model(&cfg, custom_claude_input()).expect("seed custom model");
    cfg.llm.default_model = "custom-claude".to_string();
    let config_path = work_dir.path().join("tomcat.config.toml");
    fs::write(
        &config_path,
        toml::to_string_pretty(&cfg).expect("serialize config"),
    )
    .expect("write config");
    let store_path = resolve_sessions_dir(&cfg)
        .expect("sessions dir")
        .join("sessions.json");
    std::fs::create_dir_all(store_path.parent().expect("store parent")).expect("mkdir sessions");
    let corrupt = "{not-json";
    fs::write(&store_path, corrupt).expect("write corrupt store");
    let models_path = ModelCatalog::default_user_path(&cfg).expect("models path");
    let models_before = fs::read(&models_path).expect("read models before");
    let config_before = fs::read(&config_path).expect("read config before");

    let error = remove_user_model_with_config_path(&cfg, Some(&config_path), "custom-claude")
        .expect_err("corrupt sessions store must block deletion");
    assert!(matches!(error, crate::AppError::Config(_)));
    assert_eq!(
        fs::read(&models_path).expect("read models after"),
        models_before
    );
    assert_eq!(
        fs::read(&config_path).expect("read config after"),
        config_before
    );
    assert_eq!(
        fs::read_to_string(&store_path).expect("read store after"),
        corrupt
    );
}

#[test]
#[serial(env_lock)]
fn upsert_user_model_rejects_unknown_api() {
    clear_managed_credentials_for_test();
    let (_work_dir, cfg) = temp_cfg();
    let error = upsert_user_model(
        &cfg,
        ModelEntryInput {
            id: "bad-api".to_string(),
            model_name: Some("bad-api".to_string()),
            api: "mystery-wire".to_string(),
            provider: "openai".to_string(),
            api_key_env: None,
            base_url: Some("https://example.test/v1".to_string()),
            capabilities: Capabilities::default(),
            context_window: None,
            context_window_options: None,
            max_output_tokens: None,
            description: None,
            supported_speeds: None,
            supported_reasoning_levels: None,
            thinking_format: None,
        },
    )
    .expect_err("unknown api must fail");
    assert!(
        error.to_string().contains("mystery-wire"),
        "error should mention rejected api, got: {error}"
    );
}

#[test]
#[serial(env_lock)]
fn upsert_user_model_rejects_anthropic_files_capability() {
    clear_managed_credentials_for_test();
    let (_work_dir, cfg) = temp_cfg();
    let error = upsert_user_model(
        &cfg,
        ModelEntryInput {
            capabilities: Capabilities {
                files: true,
                ..Default::default()
            },
            ..custom_claude_input()
        },
    )
    .expect_err("anthropic files capability must fail");
    assert!(
        error.to_string().contains("files"),
        "error should mention unsupported files capability, got: {error}"
    );
}

#[test]
#[serial(env_lock)]
fn set_provider_key_rejects_invalid_env_file() {
    clear_managed_credentials_for_test();
    let (work_dir, cfg) = temp_cfg();
    upsert_user_model(&cfg, custom_claude_input()).expect("seed custom model");

    let env_path = work_dir.path().join("assets").join(".env");
    std::fs::create_dir_all(env_path.parent().expect("env parent")).expect("mkdir assets");
    let invalid_env = "BROKEN_ENV=\"unterminated\n";
    std::fs::write(&env_path, invalid_env).expect("seed broken env");

    let error = set_provider_key(
        &cfg,
        ProviderKeyInput {
            env_name: "ADMIN_TEST_ANTHROPIC_API_KEY".to_string(),
            value: "should-not-write".to_string(),
        },
    )
    .expect_err("broken env file must fail");
    assert!(matches!(error, crate::AppError::Config(_)));
    assert!(error
        .to_string()
        .contains(env_path.to_string_lossy().as_ref()));
    assert_eq!(std::fs::read_to_string(&env_path).unwrap(), invalid_env);
}

#[test]
#[serial(env_lock)]
fn key_rotation_rebuilds_provider_for_same_resolver() {
    clear_managed_credentials_for_test();
    let (work_dir, cfg) = temp_cfg();
    upsert_user_model(&cfg, custom_claude_input()).expect("seed custom model");

    set_provider_key(
        &cfg,
        ProviderKeyInput {
            env_name: "ADMIN_TEST_ANTHROPIC_API_KEY".to_string(),
            value: "first-secret".to_string(),
        },
    )
    .expect("seed first provider key");

    let shared_catalog = SharedModelCatalog::load(&cfg).expect("shared catalog");
    let resolver = DefaultLlmResolver::new(
        cfg.clone(),
        shared_catalog.clone(),
        model_prefs(work_dir.path()),
    );
    let first = resolver
        .resolve(LlmScene::Main, Some("custom-claude"))
        .expect("resolve first provider");

    set_provider_key(
        &cfg,
        ProviderKeyInput {
            env_name: "ADMIN_TEST_ANTHROPIC_API_KEY".to_string(),
            value: "second-secret".to_string(),
        },
    )
    .expect("rotate provider key");
    shared_catalog.reload(&cfg).expect("reload shared catalog");

    let second = resolver
        .resolve(LlmScene::Main, Some("custom-claude"))
        .expect("resolve rotated provider");
    assert!(
        !Arc::ptr_eq(&first.provider_impl, &second.provider_impl),
        "provider cache should rebuild after key rotation"
    );
}

#[test]
#[serial(env_lock)]
fn model_config_reload_rebuilds_provider_without_restart() {
    clear_managed_credentials_for_test();
    let (work_dir, cfg) = temp_cfg();
    let model_id = "relay-openai";
    let api_key_env = "ADMIN_TEST_RELAY_API_KEY";
    let base_url = "https://gateway.example.test/v1";
    let capabilities = Capabilities {
        tools: true,
        reasoning: true,
        ..Default::default()
    };

    upsert_user_model(
        &cfg,
        ModelEntryInput {
            id: model_id.to_string(),
            model_name: Some("gpt-5.4".to_string()),
            api: "openai-responses".to_string(),
            provider: "relay".to_string(),
            api_key_env: Some(api_key_env.to_string()),
            base_url: Some(base_url.to_string()),
            capabilities: capabilities.clone(),
            context_window: None,
            context_window_options: None,
            max_output_tokens: None,
            description: None,
            supported_speeds: None,
            supported_reasoning_levels: None,
            thinking_format: Some("openai".to_string()),
        },
    )
    .expect("seed relay model");
    set_provider_key(
        &cfg,
        ProviderKeyInput {
            env_name: api_key_env.to_string(),
            value: "relay-secret".to_string(),
        },
    )
    .expect("seed relay key");

    let shared_catalog = SharedModelCatalog::load(&cfg).expect("shared catalog");
    let resolver = DefaultLlmResolver::new(
        cfg.clone(),
        shared_catalog.clone(),
        model_prefs(work_dir.path()),
    );
    let first = resolver
        .resolve(LlmScene::Main, Some(model_id))
        .expect("resolve openai wire");
    assert_eq!(first.thinking_format, ThinkingFormat::Openai);

    upsert_user_model(
        &cfg,
        ModelEntryInput {
            id: model_id.to_string(),
            model_name: Some("gpt-5.4".to_string()),
            api: "openai-responses".to_string(),
            provider: "relay".to_string(),
            api_key_env: Some(api_key_env.to_string()),
            base_url: Some(base_url.to_string()),
            capabilities,
            context_window: None,
            context_window_options: None,
            max_output_tokens: None,
            description: None,
            supported_speeds: None,
            supported_reasoning_levels: None,
            thinking_format: Some("anthropic".to_string()),
        },
    )
    .expect("rewrite relay model with new format");
    shared_catalog.reload(&cfg).expect("reload shared catalog");

    let second = resolver
        .resolve(LlmScene::Main, Some(model_id))
        .expect("resolve reconfigured wire");
    assert_eq!(second.thinking_format, ThinkingFormat::Anthropic);
    assert!(
        !Arc::ptr_eq(&first.provider_impl, &second.provider_impl),
        "provider cache should rebuild after model config reload"
    );
}
