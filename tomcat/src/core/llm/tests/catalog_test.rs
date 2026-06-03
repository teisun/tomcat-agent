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
        .lookup("deepseek-v4-pro")
        .expect("builtin deepseek-v4-pro");
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
fn infer_mimo_v25_pro_from_bare_models_toml_entry() {
    // 仅写裸 id 也应推断出 MiMo 的 provider/api/base_url/能力（catalog 推断兜底）。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "mimo-v2.5-pro"
"#,
    )
    .unwrap();

    let cfg = AppConfig::default();
    let catalog = ModelCatalog::load_from_path(&cfg, path).expect("load mimo catalog");
    let entry = catalog
        .lookup("mimo-v2.5-pro")
        .expect("inferred mimo entry");
    assert_eq!(entry.api, "openai");
    assert_eq!(entry.provider, "mimo");
    assert_eq!(
        entry.base_url.as_deref(),
        Some("https://token-plan-cn.xiaomimimo.com")
    );
    assert!(
        !entry.capabilities.vision,
        "mimo-v2.5-pro 仅文本，无 vision"
    );
    assert!(!entry.capabilities.files);
    assert!(entry.capabilities.tools);
    assert!(entry.capabilities.reasoning);
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
fn missing_model_requires_explicit_catalog_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let mut cfg = AppConfig::default();
    cfg.llm.default_model = "custom-deepseek".to_string();

    let catalog = ModelCatalog::load_from_path(&cfg, path).expect("load catalog");
    assert!(catalog.lookup("custom-deepseek").is_none());
    let err = catalog.lookup_explicit("custom-deepseek").unwrap_err();
    assert!(err.to_string().contains("custom-deepseek"));
}
