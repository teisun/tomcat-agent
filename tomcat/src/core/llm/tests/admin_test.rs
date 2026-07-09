use std::fs;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use fs2::FileExt;
use serial_test::serial;

use crate::core::llm::{
    auth::clear_managed_credentials_for_test, list_model_views, list_provider_keys,
    remove_user_model, set_provider_key, upsert_user_model, Capabilities, DefaultLlmResolver,
    LlmResolver, LlmScene, ModelCatalog, ModelEntryInput, ModelSource, ProviderKeyInput,
    SharedModelCatalog,
};
use crate::infra::config::AppConfig;
use crate::{resolve_sessions_dir, save_store, SessionEntry, SessionStore};

fn temp_cfg() -> (tempfile::TempDir, AppConfig) {
    let work_dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().into_owned());
    (work_dir, cfg)
}

fn custom_claude_input() -> ModelEntryInput {
    ModelEntryInput {
        id: "custom-claude".to_string(),
        model_name: Some("claude-opus-4-6".to_string()),
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        api_key_env: Some("ADMIN_TEST_ANTHROPIC_KEY".to_string()),
        base_url: Some("https://api.anthropic.com/v1".to_string()),
        capabilities: Capabilities::default(),
        context_window: None,
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
        thinking_format: Some("openai-responses".to_string()),
    };

    let view = upsert_user_model(&cfg, input).expect("upsert user model");
    assert_eq!(view.source, ModelSource::User);
    assert_eq!(view.api_key_env, "ADMIN_TEST_OPENAI_KEY");
    assert!(!view.key_present);
    assert_eq!(view.model_name.as_deref(), Some("gpt-5.4"));

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
#[serial(env_lock)]
fn set_provider_key_persists_env_and_flips_key_presence() {
    clear_managed_credentials_for_test();
    let (work_dir, cfg) = temp_cfg();
    upsert_user_model(&cfg, custom_claude_input()).expect("seed custom model");

    let status = set_provider_key(
        &cfg,
        ProviderKeyInput {
            env_name: "ADMIN_TEST_ANTHROPIC_KEY".to_string(),
            value: "super-secret".to_string(),
        },
    )
    .expect("persist provider key");
    assert_eq!(status.env_name, "ADMIN_TEST_ANTHROPIC_KEY");
    assert!(status.key_present);

    let env_path = work_dir.path().join("assets").join(".env");
    let env_text = fs::read_to_string(&env_path).expect("read .env");
    assert!(
        env_text.contains("ADMIN_TEST_ANTHROPIC_KEY=super-secret"),
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

    let key_view = list_provider_keys(&catalog)
        .into_iter()
        .find(|entry| entry.env_name == "ADMIN_TEST_ANTHROPIC_KEY")
        .expect("provider key view");
    assert!(key_view.key_present);
    assert_eq!(key_view.provider, "anthropic");
    assert_eq!(key_view.model_ids, vec!["custom-claude".to_string()]);
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
            env_name: "ADMIN_TEST_ANTHROPIC_KEY".to_string(),
            value: "rotated-secret".to_string(),
        },
    )
    .expect("persist provider key");

    let env_text = fs::read_to_string(&env_path).expect("read .env");
    assert!(env_text.contains("OTHER_KEEP=keep-me"));
    assert!(env_text.contains("ADMIN_TEST_ANTHROPIC_KEY=rotated-secret"));
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
                env_name: "ADMIN_TEST_ANTHROPIC_KEY".to_string(),
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
fn remove_user_model_rejects_models_still_referenced_by_config_or_sessions() {
    clear_managed_credentials_for_test();
    let (_work_dir, mut cfg) = temp_cfg();
    upsert_user_model(&cfg, custom_claude_input()).expect("seed custom model");
    cfg.llm.default_model = "custom-claude".to_string();
    cfg.context.compaction_model = "custom-claude".to_string();

    let sessions_dir = resolve_sessions_dir(&cfg).expect("sessions dir");
    std::fs::create_dir_all(&sessions_dir).expect("mkdir sessions dir");
    save_store(
        &sessions_dir.join("sessions.json"),
        &SessionStore {
            current: [("agent:main:main".to_string(), "session-1".to_string())]
                .into_iter()
                .collect(),
            sessions: [(
                "session-1".to_string(),
                SessionEntry {
                    session_key: "agent:main:main".to_string(),
                    session_id: "session-1".to_string(),
                    updated_at: 1,
                    session_file: None,
                    cwd: None,
                    thinking_level: None,
                    model_override: Some("custom-claude".to_string()),
                    input_tokens: None,
                    output_tokens: None,
                    compaction_count: None,
                    compaction_tokens_freed: None,
                    tool_result_chars_persisted: None,
                    context_utilization_ratio: None,
                    last_checkpoint_id: None,
                    title: Some("Pinned Claude Session".to_string()),
                },
            )]
            .into_iter()
            .collect(),
        },
    )
    .expect("save session store");

    let error = remove_user_model(&cfg, "custom-claude").expect_err("in-use model must fail");
    let message = error.to_string();
    assert!(message.contains("llm.default_model"), "unexpected error: {message}");
    assert!(
        message.contains("context.compaction_model"),
        "unexpected error: {message}"
    );
    assert!(message.contains("session `session-1`"), "unexpected error: {message}");
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
    std::fs::write(&env_path, "BROKEN_ENV=\"unterminated\n").expect("seed broken env");

    let error = set_provider_key(
        &cfg,
        ProviderKeyInput {
            env_name: "ADMIN_TEST_ANTHROPIC_KEY".to_string(),
            value: "should-not-write".to_string(),
        },
    )
    .expect_err("broken env file must fail");
    assert!(
        error.to_string().contains("解析"),
        "error should mention env parse failure, got: {error}"
    );
    let env_text = std::fs::read_to_string(&env_path).expect("read .env");
    assert!(
        env_text.contains("unterminated"),
        "broken env file should remain unchanged"
    );
}

#[test]
#[serial(env_lock)]
fn key_rotation_rebuilds_provider_for_same_resolver() {
    clear_managed_credentials_for_test();
    let (_work_dir, cfg) = temp_cfg();
    upsert_user_model(&cfg, custom_claude_input()).expect("seed custom model");

    set_provider_key(
        &cfg,
        ProviderKeyInput {
            env_name: "ADMIN_TEST_ANTHROPIC_KEY".to_string(),
            value: "first-secret".to_string(),
        },
    )
    .expect("seed first provider key");

    let shared_catalog = SharedModelCatalog::load(&cfg).expect("shared catalog");
    let resolver = DefaultLlmResolver::new(cfg.clone(), shared_catalog.clone());
    let first = resolver
        .resolve(LlmScene::Main, Some("custom-claude"))
        .expect("resolve first provider");

    set_provider_key(
        &cfg,
        ProviderKeyInput {
            env_name: "ADMIN_TEST_ANTHROPIC_KEY".to_string(),
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
