use std::sync::Arc;

use serial_test::serial;

use crate::core::llm::{DefaultLlmResolver, LlmResolver, LlmScene, ModelCatalog};
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
fn provider_cache_reuses_arc_for_same_route() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let cfg = AppConfig::default();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver = DefaultLlmResolver::new(cfg, catalog);

    unsafe {
        std::env::set_var("OPENAI_API_KEY", "stub");
    }

    let default_call = resolver.resolve(LlmScene::Main, None).unwrap();
    let switched_call = resolver.resolve(LlmScene::Main, Some("gpt-5.2")).unwrap();
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
fn catalog_route_ignores_legacy_api_base_override() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let mut cfg = AppConfig::default();
    cfg.llm.provider = "openai".to_string();
    cfg.llm.api_base = Some("http://127.0.0.1:8899".to_string());
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
    assert_eq!(resolved.base_url.as_deref(), Some("https://api.openai.com"));
    assert_eq!(resolved.key_source, "OPENAI_API_KEY");

    unsafe {
        std::env::remove_var("OPENAI_API_KEY");
    }
}
