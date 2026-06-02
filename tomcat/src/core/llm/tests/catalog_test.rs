use crate::core::llm::ModelCatalog;
use crate::infra::config::AppConfig;

#[test]
fn resolve_known_model() {
    let cfg = AppConfig::default();
    let catalog = ModelCatalog::load_from_path(
        &cfg,
        tempfile::tempdir().unwrap().path().join("models.toml"),
    )
    .expect("load default catalog");

    let gpt = catalog.lookup("gpt-5.4").expect("builtin gpt-5.4");
    assert_eq!(gpt.api, "openai-responses");
    assert_eq!(gpt.provider, "openai");

    let deepseek = catalog
        .lookup("deepseek-reasoner")
        .expect("builtin deepseek-reasoner");
    assert_eq!(deepseek.api, "openai");
    assert_eq!(deepseek.provider, "deepseek");
}

#[test]
fn merge_user_override() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "gpt-5.4"
base_url = "https://example.override"

[models.capabilities]
vision = false
"#,
    )
    .unwrap();

    let cfg = AppConfig::default();
    let catalog = ModelCatalog::load_from_path(&cfg, path).expect("load merged catalog");
    let entry = catalog.lookup("gpt-5.4").expect("overridden model");
    assert_eq!(entry.base_url.as_deref(), Some("https://example.override"));
    assert!(!entry.capabilities.vision);
    assert_eq!(entry.provider, "openai");
}

#[test]
fn missing_explicit_model_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let cfg = AppConfig::default();
    let catalog = ModelCatalog::load_from_path(&cfg, path.clone()).expect("load catalog");

    let err = catalog.lookup_explicit("unknown-model").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("unknown-model"));
    assert!(msg.contains(&path.display().to_string()));
}

#[test]
fn legacy_fallback_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let mut cfg = AppConfig::default();
    cfg.llm.provider = "openai".to_string();
    cfg.llm.api_base = Some("https://legacy.gateway".to_string());
    cfg.llm.api_key_env = Some("DEEPSEEK_API_KEY".to_string());

    let catalog = ModelCatalog::load_from_path(&cfg, path).expect("load catalog");
    let entry = catalog
        .lookup_or_legacy("custom-deepseek", &cfg.llm, &cfg.context)
        .expect("legacy fallback");
    assert_eq!(entry.id, "custom-deepseek");
    assert_eq!(entry.api, "openai");
    assert_eq!(entry.provider, "deepseek");
    assert_eq!(entry.base_url.as_deref(), Some("https://legacy.gateway"));
}
