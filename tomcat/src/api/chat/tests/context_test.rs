use std::sync::Arc;

use serial_test::serial;

use super::{
    connector_registry_for, normalize_plan_transcript_event_for_live_delivery,
    resolve_bash_production_policy, ChatContext,
};
use crate::core::llm::{DefaultLlmResolver, LlmResolver, ModelCatalog};
use crate::core::plan_runtime::prod_reviewer::resolve_subagent_runtime;
use crate::{AppConfig, ModelPrefsStore, ThinkingLevel};

fn model_prefs(path: &std::path::Path) -> Arc<ModelPrefsStore> {
    Arc::new(
        ModelPrefsStore::load(path.join("model-thinking.json"), ThinkingLevel::Medium)
            .expect("model preferences"),
    )
}

#[test]
fn bash_production_policy_resolves_non_default_wait_output_and_path() {
    let mut cfg = AppConfig::default();
    cfg.tools.bash.foreground_wait_ms = 9_000;
    cfg.tools.bash.max_output_chars = 42_000;
    let trail = std::path::Path::new("/tmp/tomcat-policy-test/agent-trail");

    let policy = resolve_bash_production_policy(&cfg, trail);

    assert_eq!(policy.foreground_wait_ms, 9_000);
    assert_eq!(policy.max_output_chars, 42_000);
    assert_eq!(policy.persist_dir, trail.join("tool-results"));
    let registry = crate::core::tools::primitive::BashTaskRegistry::new(policy.persist_dir.clone())
        .with_foreground_wait_ms(policy.foreground_wait_ms);
    assert_eq!(registry.foreground_wait_ms(), 9_000);
}

#[test]
fn live_plan_event_preserves_skipped_review_reason_in_camel_case() {
    let (event_name, payload) = normalize_plan_transcript_event_for_live_delivery(
        serde_json::json!({
            "event": "plan.code_review",
            "plan_id": "plan-1",
            "skip_reason": "no_reviewable_code_diff",
        }),
        "session-1",
    )
    .expect("plan event should be emitted live");

    assert_eq!(event_name, "plan.code_review");
    assert_eq!(payload["type"], "plan.code_review");
    assert_eq!(payload["sessionId"], "session-1");
    assert_eq!(payload["planId"], "plan-1");
    assert_eq!(payload["skipReason"], "no_reviewable_code_diff");
    assert!(payload.get("event").is_none());
    assert!(payload.get("plan_id").is_none());
    assert!(payload.get("skip_reason").is_none());
}

#[test]
fn connector_registry_constructed_when_enabled() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let mut enabled = AppConfig::default();
    enabled.connector.enabled = true;
    enabled.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    assert!(connector_registry_for(&enabled, Some(temp.path()))
        .expect("enabled connector registry")
        .is_some());

    let mut disabled = enabled.clone();
    disabled.connector.enabled = false;
    assert!(connector_registry_for(&disabled, Some(temp.path()))
        .expect("disabled connector registry")
        .is_none());
}
#[tokio::test]
async fn legacy_mcp_keys_do_not_disable_connectors() {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let path = crate::core::connector::mcp::config::global_mcp_path(&cfg).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"mcpServers":{"old":{"command":"node","trusted":true,"integrity":"obsolete"}}}"#,
    )
    .unwrap();

    assert!(connector_registry_for(&cfg, Some(temp.path()))
        .expect("legacy MCP keys must not disable connectors")
        .is_some());
}

#[test]
fn malformed_mcp_json_disables_mcp_without_failing_chat() {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let path = crate::core::connector::mcp::config::global_mcp_path(&cfg).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, r#"{"mcpServers":{"broken": "#).unwrap();
    let path = path.canonicalize().unwrap();

    let error = match connector_registry_for(&cfg, Some(temp.path())) {
        Ok(_) => panic!("invalid JSON should isolate the MCP runtime"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains(&crate::infra::i18n::tr_in(
            crate::infra::i18n::Locale::En,
            "mcp.config.parse",
            &[("path", &path.display().to_string()), ("detail", "")]
        )),
        "{error}"
    );
    assert!(error.contains(&path.to_string_lossy().to_string()));
}

#[test]
#[serial(env_lock)]
fn child_agent_dispatch_runtime_preserves_resolved_model_provider_pair() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let mut cfg = AppConfig::default();
    cfg.context.compaction_model = "deepseek-v4-pro".to_string();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver: Arc<dyn LlmResolver> = Arc::new(DefaultLlmResolver::new(
        cfg.clone(),
        catalog,
        model_prefs(dir.path()),
    ));

    unsafe {
        std::env::set_var("DEEPSEEK_API_KEY", "stub");
    }

    let runtime = resolve_subagent_runtime(
        resolver.as_ref(),
        &cfg.context,
        &cfg.llm.files,
        dir.path(),
        "parent-session",
        "deepseek-v4-pro",
    )
    .expect("runtime should resolve");
    assert_eq!(runtime.context_config.compaction_model, "deepseek-v4-pro");
    assert!(
        runtime.compaction_provider.is_some(),
        "应把解析成功的 compaction provider 注入给子 Agent"
    );

    unsafe {
        std::env::remove_var("DEEPSEEK_API_KEY");
    }
}

#[test]
#[serial(env_lock)]
fn child_agent_dispatch_runtime_preserves_fallback_pair() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let mut cfg = AppConfig::default();
    cfg.llm.default_model = "deepseek-v4-pro".to_string();
    cfg.context.compaction_model = "gpt-5.4".to_string();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver: Arc<dyn LlmResolver> = Arc::new(DefaultLlmResolver::new(
        cfg.clone(),
        catalog,
        model_prefs(dir.path()),
    ));

    unsafe {
        std::env::set_var("DEEPSEEK_API_KEY", "stub");
        std::env::remove_var("OPENAI_API_KEY");
    }

    let runtime = resolve_subagent_runtime(
        resolver.as_ref(),
        &cfg.context,
        &cfg.llm.files,
        dir.path(),
        "parent-session",
        "deepseek-v4-pro",
    )
    .expect("runtime should resolve");
    assert_eq!(runtime.context_config.compaction_model, "deepseek-v4-pro");
    assert!(
        runtime.compaction_provider.is_some(),
        "fallback 成功时也应把 fallback 后的 provider 一起注入"
    );

    unsafe {
        std::env::remove_var("DEEPSEEK_API_KEY");
    }
}

#[test]
#[serial(env_lock)]
fn child_agent_dispatch_runtime_keeps_compat_fallback_boundary_when_unresolved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let mut cfg = AppConfig::default();
    cfg.llm.default_model = "missing-compaction-model".to_string();
    cfg.context.compaction_model = "missing-compaction-model".to_string();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver: Arc<dyn LlmResolver> = Arc::new(DefaultLlmResolver::new(
        cfg.clone(),
        catalog,
        model_prefs(dir.path()),
    ));

    unsafe {
        std::env::set_var("OPENAI_API_KEY", "stub");
        std::env::remove_var("DEEPSEEK_API_KEY");
    }

    let runtime = resolve_subagent_runtime(
        resolver.as_ref(),
        &cfg.context,
        &cfg.llm.files,
        dir.path(),
        "parent-session",
        "gpt-5.4",
    )
    .expect("main model should still resolve");

    assert!(
        runtime.compaction_provider.is_none(),
        "无法解析 pair 时应保留兼容回退边界"
    );
    assert_eq!(
        runtime.context_config.compaction_model, "missing-compaction-model",
        "未解析成功时不应偷偷改写 compaction_model"
    );

    unsafe {
        std::env::remove_var("OPENAI_API_KEY");
    }
}

#[test]
#[serial(env_lock)]
fn resolve_thinking_level_and_speed_use_catalog_id_key_when_model_name_differs() {
    const ENV_KEY: &str = "TOMCAT_REASONING_LOOKUP_TEST_KEY";

    let dir = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());
    cfg.llm.default_model = "relay/gpt-sol".to_string();
    cfg.context.compaction_model = "relay/gpt-sol".to_string();
    std::fs::write(
        dir.path().join("models.toml"),
        format!(
            r#"
[[models]]
id = "relay/gpt-sol"
model_name = "gpt-sol"
api = "openai-responses"
provider = "relay"
api_key_env = "{env}"
base_url = "https://api.example.test"
thinking_format = "openai"
supported_reasoning_levels = ["low", "medium", "high", "xhigh"]
capabilities = {{ vision = false, files = false, tools = true, reasoning = true, web_search = false }}
"#,
            env = ENV_KEY,
        ),
    )
    .unwrap();

    unsafe {
        std::env::set_var(ENV_KEY, "stub");
    }

    let ctx = ChatContext::from_config(cfg).expect("ctx");
    ctx.global_services
        .model_prefs
        .set_reasoning("relay/gpt-sol", ThinkingLevel::Xhigh)
        .expect("persist relay override");
    ctx.global_services
        .model_prefs
        .set_speed("relay/gpt-sol", crate::core::llm::Speed::Fast)
        .unwrap();
    assert_eq!(
        ctx.resolve_speed("relay/gpt-sol"),
        None,
        "undeclared model ignores speed preference"
    );
    assert_eq!(
        ctx.resolve_speed("gpt-sol"),
        None,
        "wire name is not the preference key"
    );
    let path = dir.path().join("models.toml");
    let original = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        format!("{original}\nsupported_speeds = [\"fast\", \"ultrafast\"]\n"),
    )
    .unwrap();
    ctx.global_services
        .model_catalog
        .reload(&ctx.config)
        .unwrap();
    assert_eq!(
        ctx.resolve_speed("relay/gpt-sol"),
        Some(crate::core::llm::Speed::Fast)
    );
    ctx.global_services
        .model_prefs
        .set_speed("relay/gpt-sol", crate::core::llm::Speed::Ultrafast)
        .unwrap();
    std::fs::write(
        &path,
        format!("{original}\nsupported_speeds = [\"fast\"]\n"),
    )
    .unwrap();
    ctx.global_services
        .model_catalog
        .reload(&ctx.config)
        .unwrap();
    assert_eq!(
        ctx.resolve_speed("relay/gpt-sol"),
        Some(crate::core::llm::Speed::Standard)
    );

    assert_eq!(
        ctx.resolve_thinking_level("relay/gpt-sol"),
        ThinkingLevel::Xhigh
    );
    assert_eq!(
        ctx.resolve_thinking_level("gpt-sol"),
        ThinkingLevel::High,
        "wire model_name key miss should still fall back to the configured default level"
    );

    unsafe {
        std::env::remove_var(ENV_KEY);
    }
}

#[test]
#[serial(env_lock)]
fn resolve_thinking_level_preserves_empty_supported_levels_toggle_default() {
    const ENV_KEY: &str = "TOMCAT_REASONING_TOGGLE_TEST_KEY";

    let dir = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());
    cfg.llm.default_model = "relay/toggle-only".to_string();
    cfg.context.compaction_model = "relay/toggle-only".to_string();
    std::fs::write(
        dir.path().join("models.toml"),
        format!(
            r#"
[[models]]
id = "relay/toggle-only"
model_name = "toggle-only"
api = "openai"
provider = "relay"
api_key_env = "{env}"
base_url = "https://api.example.test"
thinking_format = "doubao"
supported_reasoning_levels = []
capabilities = {{ vision = false, files = false, tools = true, reasoning = true, web_search = false }}
"#,
            env = ENV_KEY,
        ),
    )
    .unwrap();

    unsafe {
        std::env::set_var(ENV_KEY, "stub");
    }

    let ctx = ChatContext::from_config(cfg).expect("ctx");
    ctx.global_services
        .model_prefs
        .set_reasoning("relay/toggle-only", ThinkingLevel::Low)
        .expect("persist toggle override");

    assert_eq!(
        ctx.resolve_thinking_level("relay/toggle-only"),
        ThinkingLevel::High,
        "empty supported_reasoning_levels should keep the existing Thinking On/Off fallback"
    );

    unsafe {
        std::env::remove_var(ENV_KEY);
    }
}
