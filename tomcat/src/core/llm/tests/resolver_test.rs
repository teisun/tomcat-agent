use std::sync::Arc;

use serial_test::serial;

use crate::core::llm::{
    auth::clear_managed_credentials_for_test, DefaultLlmResolver, LlmResolver, LlmScene,
    ModelCatalog, SharedModelCatalog,
};
use crate::infra::config::AppConfig;

#[test]
#[serial(env_lock)]
fn scene_fallback_to_main() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let cfg = AppConfig::default();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver = DefaultLlmResolver::new(cfg, catalog);

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
    let resolver = DefaultLlmResolver::new(cfg, catalog);

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
    let resolver = DefaultLlmResolver::new(cfg, catalog);

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
fn provider_cache_reuses_arc_for_same_route() {
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
    let resolver = DefaultLlmResolver::new(cfg, catalog);

    unsafe {
        std::env::set_var("OPENAI_API_KEY", "stub");
    }

    let default_call = resolver.resolve(LlmScene::Main, None).unwrap();
    let switched_call = resolver
        .resolve(LlmScene::Main, Some("gpt-5.4-copy"))
        .unwrap();
    assert!(
        Arc::ptr_eq(&default_call.provider_impl, &switched_call.provider_impl),
        "same (api, base_url, key_source) should reuse provider instance"
    );

    unsafe {
        std::env::remove_var("OPENAI_API_KEY");
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
    let resolver = DefaultLlmResolver::new(cfg, catalog);

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
    let resolver = DefaultLlmResolver::new(cfg.clone(), shared.clone());
    assert!(
        resolver.resolve(LlmScene::Main, Some("custom-after-reload")).is_err(),
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
    let resolver = DefaultLlmResolver::new(cfg, catalog);

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
    let resolver = DefaultLlmResolver::new(cfg, catalog);

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
fn main_scene_is_unchanged_by_compaction_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let mut cfg = AppConfig::default();
    cfg.context.compaction_model = "deepseek-v4-pro".to_string();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver = DefaultLlmResolver::new(cfg, catalog);

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
