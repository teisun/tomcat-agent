use std::sync::Arc;

use serial_test::serial;

use crate::core::llm::thinking_policy::ThinkingFormat;
use crate::core::llm::{
    auth::clear_managed_credentials_for_test, Capabilities, DefaultLlmResolver,
    EffectiveModelLimits, LimitSource, LlmResolver, LlmScene, ModelCatalog, ModelEntry,
    SharedModelCatalog,
};
use crate::infra::config::{AppConfig, ContextConfig};
use crate::{ModelPrefsStore, ThinkingLevel};

fn model_prefs(path: &std::path::Path) -> Arc<ModelPrefsStore> {
    Arc::new(
        ModelPrefsStore::load(path.join("model-thinking.json"), ThinkingLevel::Medium)
            .expect("model preferences"),
    )
}

fn limit_test_entry(
    api: &str,
    context_window: Option<u32>,
    max_output_tokens: Option<u32>,
) -> ModelEntry {
    ModelEntry {
        id: "limit-test".to_string(),
        model_name: None,
        api: api.to_string(),
        provider: "test".to_string(),
        api_key_env: None,
        base_url: None,
        capabilities: Capabilities::default(),
        context_window,
        context_window_options: Vec::new(),
        max_output_tokens,
        description: None,
        supported_speeds: Vec::new(),
        thinking_format: None,
        supported_reasoning_levels: Vec::new(),
    }
}

#[test]
fn effective_limits_use_model_capabilities_and_preserve_the_reserve_floor() {
    let known = EffectiveModelLimits::resolve(
        &limit_test_entry("anthropic-messages", Some(1_000_000), Some(128_000)),
        &ContextConfig::default(),
    )
    .expect("known limits resolve");
    assert_eq!(known.input_budget_tokens, 872_000);
    assert_eq!(known.context_source, LimitSource::ModelCatalog);
    assert_eq!(known.output_source, LimitSource::ModelCatalog);

    let unknown_anthropic = EffectiveModelLimits::resolve(
        &limit_test_entry("anthropic-messages", None, None),
        &ContextConfig::default(),
    )
    .expect("unknown Anthropic limits resolve");
    assert_eq!(unknown_anthropic.context_window, 400_000);
    assert_eq!(unknown_anthropic.output_reserve_tokens, 32_000);
    assert_eq!(unknown_anthropic.input_budget_tokens, 368_000);
    assert_eq!(
        unknown_anthropic.context_source,
        LimitSource::LegacyFallback
    );
    assert_eq!(
        unknown_anthropic.output_source,
        LimitSource::UnknownAnthropicFallback
    );

    let unknown_openai = EffectiveModelLimits::resolve(
        &limit_test_entry("openai-responses", None, None),
        &ContextConfig::default(),
    )
    .expect("unknown OpenAI limits resolve");
    assert_eq!(unknown_openai.output_reserve_tokens, 100_000);
    assert_eq!(unknown_openai.input_budget_tokens, 300_000);
    assert_eq!(
        unknown_openai.output_source,
        LimitSource::UnknownOpenAiLocalReserve
    );

    let below_cap = ContextConfig {
        output_reserve_tokens: Some(10_000),
        ..ContextConfig::default()
    };
    let protected = EffectiveModelLimits::resolve(
        &limit_test_entry("anthropic-messages", Some(1_000_000), Some(128_000)),
        &below_cap,
    )
    .expect("reserve is raised to model output capacity");
    assert_eq!(protected.output_reserve_tokens, 128_000);
    assert_eq!(protected.input_budget_tokens, 872_000);

    let legacy_reserve = ContextConfig {
        output_reserve_tokens: Some(128_000),
        ..ContextConfig::default()
    };
    let legacy_unknown = EffectiveModelLimits::resolve(
        &limit_test_entry("anthropic-messages", None, None),
        &legacy_reserve,
    )
    .expect("legacy reserve remains compatible");
    assert_eq!(legacy_unknown.input_budget_tokens, 272_000);

    assert!(
        EffectiveModelLimits::resolve(
            &limit_test_entry("anthropic-messages", Some(100_000), Some(100_001)),
            &ContextConfig::default(),
        )
        .is_err(),
        "a model output cap cannot exceed its context window"
    );
    assert!(
        EffectiveModelLimits::resolve(
            &limit_test_entry("anthropic-messages", Some(200_000), Some(64_000)),
            &ContextConfig {
                output_reserve_tokens: Some(200_000),
                ..ContextConfig::default()
            },
        )
        .is_err(),
        "the local reserve must stay below the context window"
    );
}

#[test]
fn per_request_output_limit_clamps_explicit_requests_without_forcing_openai_defaults() {
    let anthropic = EffectiveModelLimits::resolve(
        &limit_test_entry("anthropic-messages", Some(200_000), Some(8_192)),
        &ContextConfig::default(),
    )
    .expect("Anthropic limits");
    assert_eq!(
        anthropic.wire_output_limit_for_request("anthropic-messages", None),
        (Some(8_192), LimitSource::ModelCatalog)
    );
    assert_eq!(
        anthropic.wire_output_limit_for_request("anthropic-messages", Some(16_384)),
        (Some(8_192), LimitSource::ExplicitRequest)
    );

    let openai = EffectiveModelLimits::resolve(
        &limit_test_entry("openai-responses", Some(200_000), Some(8_192)),
        &ContextConfig::default(),
    )
    .expect("OpenAI limits");
    assert_eq!(
        openai.wire_output_limit_for_request("openai-responses", None),
        (None, LimitSource::ModelCatalog),
        "OpenAI must omit an unspecified output cap"
    );
    assert_eq!(
        openai.wire_output_limit_for_request("openai-responses", Some(16_384)),
        (Some(8_192), LimitSource::ExplicitRequest),
        "an explicit OpenAI request is clamped before reaching either wire adapter"
    );
}

#[test]
fn selected_context_tier_changes_effective_input_budget_and_invalid_choice_falls_back() {
    let mut entry = limit_test_entry("openai-responses", Some(400_000), Some(64_000));
    entry.context_window_options = vec![400_000, 1_000_000];

    let selected = EffectiveModelLimits::resolve_with_context_window(
        &entry,
        &ContextConfig::default(),
        Some(1_000_000),
    )
    .expect("selected context tier resolves");
    assert_eq!(selected.context_window, 1_000_000);
    assert_eq!(selected.input_budget_tokens, 936_000);

    let invalid = EffectiveModelLimits::resolve_with_context_window(
        &entry,
        &ContextConfig::default(),
        Some(123_456),
    )
    .expect("the resolver itself keeps byte-compatible fallback behavior");
    assert_eq!(invalid.context_window, 400_000);
}

#[test]
fn no_selected_prefs_keep_limits_byte_identical() {
    let entry = limit_test_entry("openai-responses", Some(400_000), Some(64_000));
    let config = ContextConfig::default();
    let legacy = EffectiveModelLimits::resolve(&entry, &config).expect("legacy limits");
    let no_store = EffectiveModelLimits::resolve_with_context_window(&entry, &config, None)
        .expect("no-store limits");
    assert_eq!(no_store, legacy);
}

#[test]
#[serial(env_lock)]
fn resolver_applies_persisted_context_choice_to_effective_limits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "tiered-model"
api = "openai-responses"
provider = "openai"
api_key_env = "OPENAI_API_KEY"
base_url = "https://api.openai.com"
context_window = 400000
context_window_options = [400000, 1000000]
max_output_tokens = 64000
"#,
    )
    .unwrap();
    let mut cfg = AppConfig::default();
    cfg.llm.default_model = "tiered-model".to_string();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let prefs = model_prefs(dir.path());
    prefs
        .set_context_window("tiered-model", Some(1_000_000))
        .unwrap();
    let resolver = DefaultLlmResolver::new(cfg, catalog, prefs);

    unsafe {
        std::env::set_var("OPENAI_API_KEY", "stub");
    }

    let resolved = resolver
        .resolve(LlmScene::Main, None)
        .expect("resolve selected tier");
    assert_eq!(resolved.limits.context_window, 1_000_000);
    assert_eq!(resolved.limits.input_budget_tokens, 936_000);

    unsafe {
        std::env::remove_var("OPENAI_API_KEY");
    }
}

#[test]
#[serial(env_lock)]
fn scene_fallback_to_main() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let cfg = AppConfig::default();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver = DefaultLlmResolver::new(cfg, catalog, model_prefs(dir.path()));

    unsafe {
        std::env::set_var("OPENAI_API_KEY", "stub");
    }

    let resolved = resolver
        .resolve(LlmScene::Vision, None)
        .expect("vision fallback");
    assert_eq!(resolved.model, "gpt-5.4");
    assert_eq!(resolved.key_source, "OPENAI_API_KEY");

    unsafe {
        std::env::remove_var("OPENAI_API_KEY");
    }
}

#[test]
#[serial(env_lock)]
fn override_priority() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let cfg = AppConfig::default();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver = DefaultLlmResolver::new(cfg, catalog, model_prefs(dir.path()));

    unsafe {
        std::env::set_var("DEEPSEEK_API_KEY", "stub");
        std::env::set_var("OPENAI_API_KEY", "stub");
    }

    let resolved = resolver
        .resolve(LlmScene::Main, Some("deepseek-v4-pro"))
        .expect("session override should win");
    assert_eq!(resolved.model, "deepseek-v4-pro");
    assert_eq!(resolved.provider, "deepseek");
    assert_eq!(resolved.key_source, "DEEPSEEK_API_KEY");

    unsafe {
        std::env::remove_var("DEEPSEEK_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");
    }
}

#[test]
#[serial(env_lock)]
fn resolves_mimo_via_models_toml_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "mimo-v2.5-pro"
api = "openai"
provider = "mimo"
base_url = "https://token-plan-cn.xiaomimimo.com"
thinking_format = "doubao"
capabilities = { vision = false, files = false, tools = true, reasoning = true }
"#,
    )
    .unwrap();

    let cfg = AppConfig::default();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver = DefaultLlmResolver::new(cfg, catalog, model_prefs(dir.path()));

    unsafe {
        std::env::set_var("MIMO_API_KEY", "tp-stub");
        std::env::remove_var("OPENAI_API_KEY");
    }

    let resolved = resolver
        .resolve(LlmScene::Main, Some("mimo-v2.5-pro"))
        .expect("mimo route should resolve");
    assert_eq!(resolved.model, "mimo-v2.5-pro");
    assert_eq!(resolved.api, "openai");
    assert_eq!(resolved.provider, "mimo");
    assert_eq!(
        resolved.base_url.as_deref(),
        Some("https://token-plan-cn.xiaomimimo.com")
    );
    assert_eq!(resolved.key_source, "MIMO_API_KEY");

    unsafe {
        std::env::remove_var("MIMO_API_KEY");
    }
}

#[test]
#[serial(env_lock)]
fn resolver_auto_thinking_format_uses_wire_not_model_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "claude-relay"
model_name = "claude-opus-4-6"
api = "openai"
provider = "relay"
api_key_env = "RELAY_API_KEY"
base_url = "https://gateway.example.test/v1"
capabilities = { vision = false, files = false, tools = true, reasoning = true }
"#,
    )
    .unwrap();

    let cfg = AppConfig::default();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver = DefaultLlmResolver::new(cfg, catalog, model_prefs(dir.path()));

    unsafe {
        std::env::set_var("RELAY_API_KEY", "stub");
    }

    let resolved = resolver
        .resolve(LlmScene::Main, Some("claude-relay"))
        .expect("relay model should resolve");
    assert_eq!(resolved.thinking_format, ThinkingFormat::Openai);

    unsafe {
        std::env::remove_var("RELAY_API_KEY");
    }
}

#[test]
#[serial(env_lock)]
fn resolver_thinking_format_priority_is_entry_then_global_then_wire() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "entry-explicit"
model_name = "claude-opus-4-6"
api = "openai"
provider = "relay"
api_key_env = "RELAY_API_KEY"
base_url = "https://gateway.example.test/v1"
thinking_format = "anthropic"
capabilities = { vision = false, files = false, tools = true, reasoning = true }

[[models]]
id = "global-explicit"
model_name = "claude-opus-4-6"
api = "openai"
provider = "relay"
api_key_env = "RELAY_API_KEY"
base_url = "https://gateway.example.test/v1"
capabilities = { vision = false, files = false, tools = true, reasoning = true }
"#,
    )
    .unwrap();

    let mut cfg = AppConfig::default();
    cfg.llm.thinking.format = Some("deepseek".to_string());
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver = DefaultLlmResolver::new(cfg, catalog, model_prefs(dir.path()));

    unsafe {
        std::env::set_var("RELAY_API_KEY", "stub");
    }

    let entry_explicit = resolver
        .resolve(LlmScene::Main, Some("entry-explicit"))
        .expect("entry-explicit should resolve");
    assert_eq!(entry_explicit.thinking_format, ThinkingFormat::Anthropic);

    let global_explicit = resolver
        .resolve(LlmScene::Main, Some("global-explicit"))
        .expect("global-explicit should resolve");
    assert_eq!(global_explicit.thinking_format, ThinkingFormat::Deepseek);

    unsafe {
        std::env::remove_var("RELAY_API_KEY");
    }
}

#[test]
#[serial(env_lock)]
fn provider_cache_isolates_models_and_reuses_same_model_and_route() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "gpt-5.4-copy"
model_name = "gpt-5.4"
api = "openai-responses"
provider = "openai"
api_key_env = "OPENAI_API_KEY"
base_url = "https://api.openai.com"
capabilities = { vision = true, files = true, tools = true, reasoning = true }
"#,
    )
    .unwrap();
    let cfg = AppConfig::default();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver = DefaultLlmResolver::new(cfg, catalog, model_prefs(dir.path()));

    unsafe {
        std::env::set_var("OPENAI_API_KEY", "stub");
    }

    let default_call = resolver.resolve(LlmScene::Main, None).unwrap();
    let switched_call = resolver
        .resolve(LlmScene::Main, Some("gpt-5.4-copy"))
        .unwrap();
    assert!(
        !Arc::ptr_eq(&default_call.provider_impl, &switched_call.provider_impl),
        "different catalog IDs must not share model-specific state"
    );
    let again = resolver
        .resolve(LlmScene::Main, Some("gpt-5.4-copy"))
        .unwrap();
    assert!(Arc::ptr_eq(
        &again.provider_impl,
        &switched_call.provider_impl
    ));
    let route_a = resolver.route_for_model(&default_call.catalog_id);
    let route_b = resolver.route_for_model(&switched_call.catalog_id);
    assert!(Arc::ptr_eq(&route_a, &route_b));
    assert!(Arc::ptr_eq(
        route_a.semaphore.as_ref().unwrap(),
        route_b.semaphore.as_ref().unwrap()
    ));

    unsafe {
        std::env::remove_var("OPENAI_API_KEY");
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn same_route_models_use_their_own_wire_format_and_vision_capabilities() {
    use super::mocks::{MockHttpServer, ScriptedHttpResponse};
    use crate::core::llm::{ChatMessage, ChatMessageContent, ChatMessageContentPart, ChatRequest};
    for (api, format_a, format_b, field) in [
        ("openai", "deepseek", "openai", "reasoning_effort"),
        ("openai-responses", "deepseek", "openai", "reasoning"),
        (
            "anthropic-messages",
            "anthropic-adaptive",
            "anthropic",
            "thinking",
        ),
    ] {
        // A deliberate non-retryable response: we inspect serialization, not the remote model.
        let server = MockHttpServer::start(vec![ScriptedHttpResponse::json(
            400,
            r#"{"error":{"message":"fixture rejection"}}"#,
        )])
        .await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.toml");
        std::fs::write(
            &path,
            format!(
                r#"
[[models]]
id = "route-a"
model_name = "wire-a"
api = "{api}"
provider = "relay"
api_key_env = "ROUTE_TEST_KEY"
base_url = "{base}"
thinking_format = "{format_a}"
capabilities = {{ vision = false, files = false, reasoning = true, tools = true }}
[[models]]
id = "route-b"
model_name = "wire-b"
api = "{api}"
provider = "relay"
api_key_env = "ROUTE_TEST_KEY"
base_url = "{base}"
thinking_format = "{format_b}"
capabilities = {{ vision = true, files = true, reasoning = true, tools = true }}
"#,
                base = server.base_url
            ),
        )
        .unwrap();
        let mut cfg = AppConfig::default();
        cfg.llm.default_model = "route-a".into();
        cfg.llm.retry_count = 0;
        let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
        let resolver = DefaultLlmResolver::new(cfg, catalog, model_prefs(dir.path()));
        std::env::set_var("ROUTE_TEST_KEY", "stub");
        let a = resolver.resolve(LlmScene::Main, None).unwrap();
        let b = resolver.resolve(LlmScene::Main, Some("route-b")).unwrap();
        assert!(!Arc::ptr_eq(&a.provider_impl, &b.provider_impl));
        let mut image_message = ChatMessage::user("look");
        image_message.content = Some(ChatMessageContent::Parts(vec![serde_json::from_value::<
            ChatMessageContentPart,
        >(
            serde_json::json!({
            "type": "input_image", "mime_type": "image/png", "image_b64": "aGVsbG8="
        })
        )
        .unwrap()]));
        let request = ChatRequest {
            messages: vec![image_message],
            model: b.model.clone(),
            thinking_level: Some(ThinkingLevel::High),
            resolved_output_limit: Some(8192),
            stream: Some(false),
            ..Default::default()
        };
        assert!(b.provider_impl.chat(request).await.is_err());
        let texts = server.request_texts();
        assert_eq!(texts.len(), 1, "{api}");
        let body: serde_json::Value =
            serde_json::from_str(texts[0].split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["model"], "wire-b", "{api}");
        assert!(!body[field].is_null(), "{api}: {body}");
        assert!(
            texts[0].contains("aGVsbG8="),
            "{api} stripped B's image using A's capabilities"
        );
        if api == "openai" {
            assert!(body.get("thinking").is_none());
        }
        if api == "anthropic-messages" {
            assert_eq!(body["thinking"]["type"], "enabled");
        }
        std::env::remove_var("ROUTE_TEST_KEY");
        server.shutdown().await;
    }
}

#[test]
#[serial(env_lock)]
fn catalog_route_uses_entry_base_url() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "gpt-5.4"
api = "openai-responses"
provider = "openai"
api_key_env = "OPENAI_API_KEY"
base_url = "http://127.0.0.1:8899"
capabilities = { vision = true, files = true, tools = true, reasoning = true }
"#,
    )
    .unwrap();
    let cfg = AppConfig::default();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver = DefaultLlmResolver::new(cfg, catalog, model_prefs(dir.path()));

    unsafe {
        std::env::set_var("OPENAI_API_KEY", "stub");
    }

    let resolved = resolver
        .resolve(LlmScene::Main, None)
        .expect("catalog route");
    assert_eq!(resolved.model, "gpt-5.4");
    assert_eq!(resolved.api, "openai-responses");
    assert_eq!(resolved.provider, "openai");
    assert_eq!(resolved.base_url.as_deref(), Some("http://127.0.0.1:8899"));
    assert_eq!(resolved.key_source, "OPENAI_API_KEY");

    unsafe {
        std::env::remove_var("OPENAI_API_KEY");
    }
}

#[test]
fn shared_catalog_reload_picks_up_new_user_models() {
    let work_dir = tempfile::tempdir().unwrap();
    let path = work_dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "custom-before-reload"
api = "openai-responses"
provider = "openai"
api_key_env = "OPENAI_API_KEY"
"#,
    )
    .unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().into_owned());

    let shared = SharedModelCatalog::load(&cfg).expect("load shared catalog");
    assert!(shared.lookup("custom-before-reload").is_some());
    assert!(shared.lookup("custom-after-reload").is_none());

    std::fs::write(
        &path,
        r#"
[[models]]
id = "custom-after-reload"
api = "anthropic-messages"
provider = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"
"#,
    )
    .unwrap();

    shared.reload(&cfg).expect("reload shared catalog");

    assert!(shared.lookup("custom-before-reload").is_none());
    assert!(shared.lookup("custom-after-reload").is_some());
    assert!(shared.is_user_model("custom-after-reload"));
}

#[test]
#[serial(env_lock)]
fn resolver_uses_reloaded_shared_catalog_for_new_model() {
    clear_managed_credentials_for_test();
    let work_dir = tempfile::tempdir().unwrap();
    let path = work_dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "custom-before-reload"
api = "openai-responses"
provider = "openai"
api_key_env = "OPENAI_API_KEY"
"#,
    )
    .unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().into_owned());

    let shared = SharedModelCatalog::load(&cfg).expect("load shared catalog");
    let resolver =
        DefaultLlmResolver::new(cfg.clone(), shared.clone(), model_prefs(work_dir.path()));
    assert!(
        resolver
            .resolve(LlmScene::Main, Some("custom-after-reload"))
            .is_err(),
        "resolver should not see new model before reload"
    );

    std::fs::write(
        &path,
        r#"
[[models]]
id = "custom-after-reload"
model_name = "claude-sonnet-4-5"
api = "anthropic-messages"
provider = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"
base_url = "https://api.anthropic.com/v1"
"#,
    )
    .unwrap();
    shared.reload(&cfg).expect("reload shared catalog");

    unsafe {
        std::env::set_var("ANTHROPIC_API_KEY", "stub");
    }

    let resolved = resolver
        .resolve(LlmScene::Main, Some("custom-after-reload"))
        .expect("resolver should use reloaded model catalog");
    assert_eq!(resolved.provider, "anthropic");
    assert_eq!(resolved.model, "claude-sonnet-4-5");

    unsafe {
        std::env::remove_var("ANTHROPIC_API_KEY");
    }
}

#[test]
#[serial(env_lock)]
fn compaction_falls_back_to_default_model_when_selected_provider_key_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let mut cfg = AppConfig::default();
    cfg.llm.default_model = "deepseek-v4-pro".to_string();
    cfg.context.compaction_model = "gpt-5.4".to_string();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver = DefaultLlmResolver::new(cfg, catalog, model_prefs(dir.path()));

    unsafe {
        std::env::set_var("DEEPSEEK_API_KEY", "stub");
        std::env::remove_var("OPENAI_API_KEY");
    }

    let resolved = resolver
        .resolve(LlmScene::Compaction, None)
        .expect("compaction should fall back to default model");
    assert_eq!(resolved.model, "deepseek-v4-pro");
    assert_eq!(resolved.provider, "deepseek");
    assert_eq!(resolved.key_source, "DEEPSEEK_API_KEY");

    unsafe {
        std::env::remove_var("DEEPSEEK_API_KEY");
    }
}

#[test]
#[serial(env_lock)]
fn compaction_keeps_original_error_when_already_on_default_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let mut cfg = AppConfig::default();
    cfg.llm.default_model = "deepseek-v4-pro".to_string();
    cfg.context.compaction_model = "deepseek-v4-pro".to_string();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver = DefaultLlmResolver::new(cfg, catalog, model_prefs(dir.path()));

    unsafe {
        std::env::remove_var("DEEPSEEK_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");
    }

    let err = resolver
        .resolve(LlmScene::Compaction, None)
        .expect_err("missing default-model credential should surface original error");
    let msg = err.to_string();
    assert!(msg.contains("DEEPSEEK_API_KEY"));
    assert!(
        !msg.contains("压缩模型 `deepseek-v4-pro` 不可用"),
        "same-model path should not wrap the error as a fallback failure: {msg}"
    );
}

#[test]
#[serial(env_lock)]
fn resolve_main_with_session_override_returns_provider_bound_to_that_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "deepseek-v4-flash"
model_name = "deepseek-v4-flash"
api = "openai"
provider = "deepseek"
api_key_env = "DEEPSEEK_API_KEY"
base_url = "https://api.deepseek.com"

[[models]]
id = "fcodex/gpt-5.6-sol"
model_name = "gpt-5.6-sol"
api = "openai-responses"
provider = "fcodex"
api_key_env = "FCODEX_OPENAI_API_KEY"
base_url = "https://fcodex.top"
"#,
    )
    .unwrap();

    let mut cfg = AppConfig::default();
    cfg.llm.default_model = "deepseek-v4-flash".to_string();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver = DefaultLlmResolver::new(cfg, catalog, model_prefs(dir.path()));

    unsafe {
        std::env::set_var("DEEPSEEK_API_KEY", "stub-deepseek");
        std::env::set_var("FCODEX_OPENAI_API_KEY", "stub-fcodex");
    }

    let resolved = resolver
        .resolve(LlmScene::Main, Some("fcodex/gpt-5.6-sol"))
        .expect("session override should resolve to fcodex entry");
    assert_eq!(resolved.provider, "fcodex");
    assert_eq!(resolved.catalog_id, "fcodex/gpt-5.6-sol");
    assert_eq!(resolved.model, "gpt-5.6-sol");
    assert!(
        resolved
            .base_url
            .as_deref()
            .is_some_and(|u| u.contains("fcodex.top")),
        "base_url={:?}",
        resolved.base_url
    );

    unsafe {
        std::env::remove_var("DEEPSEEK_API_KEY");
        std::env::remove_var("FCODEX_OPENAI_API_KEY");
    }
}

#[test]
#[serial(env_lock)]
fn reloaded_blank_defaults_keep_compaction_on_the_current_session_catalog_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "relay/selected-alias"
model_name = "provider-wire-name"
api = "openai-responses"
provider = "relay"
api_key_env = "RELAY_API_KEY"
base_url = "https://gateway.example.test/v1"
"#,
    )
    .unwrap();
    let mut cfg = AppConfig::default();
    cfg.llm.default_model = "relay/selected-alias".to_string();
    cfg.context.compaction_model = "relay/selected-alias".to_string();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver = DefaultLlmResolver::new(cfg.clone(), catalog, model_prefs(dir.path()));

    unsafe {
        std::env::set_var("RELAY_API_KEY", "stub");
    }
    let mut cleared = cfg;
    cleared.llm.default_model.clear();
    cleared.context.compaction_model.clear();
    resolver.reload_config(cleared);

    assert!(
        resolver.resolve(LlmScene::Main, None).is_err(),
        "the deleted default must not survive in a live resolver"
    );
    let main = resolver
        .resolve(LlmScene::Main, Some("relay/selected-alias"))
        .expect("surviving session selection resolves");
    let compaction = resolver
        .resolve(LlmScene::Compaction, Some(main.catalog_id()))
        .expect("compaction follows the selected main catalog id when both defaults are blank");
    assert_eq!(compaction.catalog_id(), "relay/selected-alias");
    assert_eq!(compaction.model, "provider-wire-name");

    unsafe {
        std::env::remove_var("RELAY_API_KEY");
    }
}

#[test]
#[serial(env_lock)]
fn main_scene_is_unchanged_by_compaction_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let mut cfg = AppConfig::default();
    cfg.context.compaction_model = "deepseek-v4-pro".to_string();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver = DefaultLlmResolver::new(cfg, catalog, model_prefs(dir.path()));

    unsafe {
        std::env::set_var("OPENAI_API_KEY", "stub");
        std::env::remove_var("DEEPSEEK_API_KEY");
    }

    let resolved = resolver
        .resolve(LlmScene::Main, None)
        .expect("main scene should keep using the configured default model");
    assert_eq!(resolved.model, "gpt-5.4");
    assert_eq!(resolved.provider, "openai");
    assert_eq!(resolved.key_source, "OPENAI_API_KEY");

    unsafe {
        std::env::remove_var("OPENAI_API_KEY");
    }
}
