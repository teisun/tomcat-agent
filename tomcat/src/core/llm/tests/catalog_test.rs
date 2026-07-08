use crate::core::llm::catalog::{builtin_seed_entries, builtin_seed_toml_text, UserModelsFile};
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
    assert!(catalog.lookup("gpt-5.2").is_some());
    assert!(catalog.lookup("gpt-5.6").is_some());

    let deepseek = catalog
        .lookup("deepseek-v4-pro")
        .expect("builtin deepseek-v4-pro");
    assert_eq!(deepseek.api, "openai");
    assert_eq!(deepseek.provider, "deepseek");
    assert!(catalog.lookup("deepseek-v4-flash").is_some());
    assert!(catalog.lookup("utility-flash").is_some());
    let claude = catalog
        .lookup("claude-opus-4-6")
        .expect("builtin claude-opus-4-6");
    assert_eq!(claude.api, "anthropic-messages");
    assert_eq!(claude.provider, "anthropic");
    let kimi = catalog
        .lookup("kimi-k2.7-code")
        .expect("builtin kimi-k2.7-code");
    assert_eq!(kimi.api, "openai");
    assert_eq!(kimi.provider, "moonshot");
    assert_eq!(kimi.base_url.as_deref(), Some("https://api.moonshot.cn"));
}

#[test]
fn builtin_models_toml_parses() {
    let parsed =
        toml::from_str::<UserModelsFile>(builtin_seed_toml_text()).expect("parse embedded seed");
    assert_eq!(parsed.models.len(), 13);
}

#[test]
fn builtin_seed_entries_match_expected_presets_and_embedded_toml() {
    let cfg = AppConfig::default();
    let entries = builtin_seed_entries(&cfg.context);
    let ids = entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            "gpt-5.2",
            "gpt-5.4",
            "gpt-5.5",
            "gpt-5.6",
            "deepseek-v4-pro",
            "deepseek-v4-flash",
            "utility-flash",
            "mimo-v2.5-pro",
            "glm-5.2",
            "kimi-k2.7-code",
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-opus-4-6",
        ]
    );

    let utility = entries
        .iter()
        .find(|entry| entry.id == "utility-flash")
        .expect("utility-flash preset");
    assert_eq!(utility.request_model_name(), "deepseek-v4-flash");
    assert_eq!(utility.context_window, Some(400_000));
    assert!(!utility.capabilities.web_search);

    let kimi = entries
        .iter()
        .find(|entry| entry.id == "kimi-k2.7-code")
        .expect("kimi preset");
    assert_eq!(kimi.base_url.as_deref(), Some("https://api.moonshot.cn"));
    assert_eq!(kimi.provider, "moonshot");
    assert_eq!(kimi.context_window, Some(400_000));

    let mimo = entries
        .iter()
        .find(|entry| entry.id == "mimo-v2.5-pro")
        .expect("mimo preset");
    assert_eq!(mimo.context_window, Some(1_000_000));

    let embedded = builtin_seed_toml_text();
    assert!(embedded.contains("id = \"utility-flash\""));
    assert!(embedded.contains("model_name = \"deepseek-v4-flash\""));
    assert!(embedded.contains("base_url = \"https://api.moonshot.cn\""));
    assert!(embedded.contains("context_window = 1000000"));
}

#[test]
fn builtin_seed_entries_keep_embedded_context_window_when_runtime_default_changes() {
    let mut cfg = AppConfig::default();
    cfg.context.context_window = 200_000;
    let entries = builtin_seed_entries(&cfg.context);

    let gpt = entries
        .iter()
        .find(|entry| entry.id == "gpt-5.4")
        .expect("gpt-5.4 preset");
    assert_eq!(gpt.context_window, Some(400_000));

    let kimi = entries
        .iter()
        .find(|entry| entry.id == "kimi-k2.7-code")
        .expect("kimi preset");
    assert_eq!(kimi.context_window, Some(400_000));

    let mimo = entries
        .iter()
        .find(|entry| entry.id == "mimo-v2.5-pro")
        .expect("mimo preset");
    assert_eq!(mimo.context_window, Some(1_000_000));
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
    assert!(catalog.is_user_model("gpt-5.4"));
}

#[test]
fn new_user_entry_requires_explicit_api_and_provider() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "my-new-mimo-preset"
"#,
    )
    .unwrap();

    let cfg = AppConfig::default();
    let err =
        ModelCatalog::load_from_path(&cfg, path).expect_err("missing api/provider should fail");
    let msg = err.to_string();
    assert!(msg.contains("my-new-mimo-preset"));
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
    let builtin_override_index = ordered
        .iter()
        .position(|entry| entry.id == "gpt-5.4")
        .expect("builtin override should stay in ordered slots");
    let custom_index = ordered
        .iter()
        .position(|entry| entry.id == "custom-hosted")
        .expect("custom hosted entry should exist");
    assert!(
        builtin_override_index < custom_index,
        "builtin override should remain ahead of appended custom entries"
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
