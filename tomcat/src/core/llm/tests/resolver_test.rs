use std::sync::Arc;

use serial_test::serial;

use crate::core::llm::{DefaultLlmResolver, LlmResolver, LlmScene, ModelCatalog};
use crate::infra::config::AppConfig;
use crate::infra::error::AppError;

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
        .resolve(LlmScene::Main, Some("deepseek-reasoner"))
        .expect("session override should win");
    assert_eq!(resolved.model, "deepseek-reasoner");
    assert_eq!(resolved.provider, "deepseek");
    assert_eq!(resolved.key_source, "DEEPSEEK_API_KEY");

    unsafe {
        std::env::remove_var("DEEPSEEK_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");
    }
}

#[test]
#[serial(env_lock)]
fn reject_vision_on_text_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let mut cfg = AppConfig::default();
    cfg.llm.vision_model = Some("deepseek-reasoner".to_string());
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
    let resolver = DefaultLlmResolver::new(cfg, catalog);

    unsafe {
        std::env::set_var("DEEPSEEK_API_KEY", "stub");
    }

    let err = resolver.resolve(LlmScene::Vision, None).unwrap_err();
    assert!(matches!(err, AppError::Llm(_)));
    assert!(err.to_string().contains("vision"));

    unsafe {
        std::env::remove_var("DEEPSEEK_API_KEY");
    }
}

#[test]
#[serial(env_lock)]
fn legacy_single_provider_mode_preserves_api_base_override() {
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
        .expect("legacy config should keep explicit api base");
    assert_eq!(resolved.model, "gpt-5.4");
    assert_eq!(resolved.api, "openai");
    assert_eq!(resolved.provider, "openai");
    assert_eq!(resolved.base_url.as_deref(), Some("http://127.0.0.1:8899"));
    assert_eq!(resolved.key_source, "OPENAI_API_KEY");

    unsafe {
        std::env::remove_var("OPENAI_API_KEY");
    }
}
