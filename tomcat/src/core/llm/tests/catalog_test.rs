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
    assert!(catalog.lookup("gpt-5.2").is_none());

    let deepseek = catalog
        .lookup("deepseek-v4-pro")
        .expect("builtin deepseek-v4-pro");
    assert_eq!(deepseek.api, "openai");
    assert_eq!(deepseek.provider, "deepseek");
    assert!(catalog.lookup("deepseek-v4-flash").is_none());
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
fn new_user_entry_requires_explicit_api_and_provider() {
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
    let err =
        ModelCatalog::load_from_path(&cfg, path).expect_err("missing api/provider should fail");
    let msg = err.to_string();
    assert!(msg.contains("mimo-v2.5-pro"));
    assert!(msg.contains("api") || msg.contains("provider"));
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

#[test]
fn merged_catalog_preserves_override_slot_and_web_search_capability() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "custom-hosted"
api = "openai-responses"
provider = "openai"

[models.capabilities]
web_search = true

[[models]]
id = "gpt-5.4"

[models.capabilities]
web_search = true
"#,
    )
    .unwrap();

    let cfg = AppConfig::default();
    let catalog = ModelCatalog::load_from_path(&cfg, path).expect("load catalog");
    let ordered = catalog.entries_in_merge_order();
    assert_eq!(
        ordered.first().map(|entry| entry.id.as_str()),
        Some("gpt-5.4")
    );
    assert_eq!(
        ordered
            .iter()
            .find(|entry| entry.id == "custom-hosted")
            .map(|entry| entry.capabilities.web_search),
        Some(true)
    );
    assert!(
        catalog
            .lookup("gpt-5.4")
            .expect("builtin override")
            .capabilities
            .web_search
    );
}

#[test]
fn user_entry_can_define_model_name_and_api_key_env_alongside_builtin_gpt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "gpt-5.4_litellm-sunmi"
model_name = "gpt-5.4"
api = "openai-responses"
provider = "litellm-sunmi"
api_key_env = "LITELLM_SUNMI_API_KEY"
base_url = "https://aigateway.sunmi.com"

[models.capabilities]
vision = true
files = true
tools = true
reasoning = true
"#,
    )
    .unwrap();

    let cfg = AppConfig::default();
    let catalog = ModelCatalog::load_from_path(&cfg, path).expect("load catalog");
    let builtin = catalog.lookup("gpt-5.4").expect("builtin gpt entry");
    let gateway = catalog
        .lookup("gpt-5.4_litellm-sunmi")
        .expect("gateway entry");
    assert_eq!(builtin.request_model_name(), "gpt-5.4");
    assert_eq!(gateway.request_model_name(), "gpt-5.4");
    assert_eq!(
        gateway.api_key_env.as_deref(),
        Some("LITELLM_SUNMI_API_KEY")
    );
    assert_eq!(gateway.provider, "litellm-sunmi");
    assert_eq!(
        gateway.base_url.as_deref(),
        Some("https://aigateway.sunmi.com")
    );
}
