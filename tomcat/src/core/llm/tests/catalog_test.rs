use crate::core::llm::catalog::{builtin_seed_entries, builtin_seed_toml_text, UserModelsFile};
use crate::core::llm::ModelCatalog;
use crate::infra::config::AppConfig;

#[test]
fn speed_declarations_inherit_clear_normalize_and_reject_invalid_apis() {
    use crate::core::llm::{SharedModelCatalog, Speed, ThinkingLevel};
    use crate::core::session::ModelPrefsStore;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let prefs = ModelPrefsStore::load(dir.path().join("prefs.json"), ThinkingLevel::High).unwrap();
    prefs.set_speed("gpt-5.6", Speed::Ultrafast).unwrap();
    for (declaration, expected) in [
        ("", vec![Speed::Fast]),
        ("supported_speeds = []", vec![]),
        (
            "supported_speeds = [\"standard\", \"fast\", \"fast\", \"ultrafast\"]",
            vec![Speed::Fast, Speed::Ultrafast],
        ),
        ("supported_speeds = [\"standard\"]", vec![]),
    ] {
        std::fs::write(&path, format!("[[models]]\nid = \"gpt-5.6\"\nsupported_reasoning_levels = [\"high\", \"xhigh\"]\n{declaration}\n")).unwrap();
        let catalog = ModelCatalog::load_from_path(&AppConfig::default(), path.clone()).unwrap();
        let entry = catalog.lookup("gpt-5.6").unwrap();
        assert_eq!(entry.supported_speeds, expected);
        assert_eq!(entry.supported_reasoning_levels, vec!["high", "xhigh"]);
        let selected = if expected.is_empty() {
            None
        } else if expected.contains(&Speed::Ultrafast) {
            Some(Speed::Ultrafast)
        } else {
            Some(Speed::Standard)
        };
        assert_eq!(
            SharedModelCatalog::from(catalog).resolve_speed(&prefs, "gpt-5.6"),
            selected
        );
    }
    std::fs::write(
        &path,
        "[[models]]\nid = \"gpt-5.6\"\nsupported_speeds = [\"warp\"]\n",
    )
    .unwrap();
    assert!(ModelCatalog::load_from_path(&AppConfig::default(), path.clone()).is_err());

    std::fs::write(
        &path,
        "[[models]]\nid = \"claude\"\napi = \"anthropic-messages\"\nprovider = \"anthropic\"\nsupported_speeds = [\"fast\"]\n\n[[models]]\nid = \"gpt-5.6\"\nsupported_speeds = [\"fast\", \"ultrafast\"]\n",
    )
    .unwrap();
    let catalog = ModelCatalog::load_from_path(&AppConfig::default(), path).unwrap();
    let claude = catalog.lookup("claude").unwrap();
    assert!(claude.supported_speeds.is_empty());
    assert_eq!(claude.resolve_speed(Speed::Fast), None);
    assert_eq!(
        catalog.lookup("gpt-5.6").unwrap().supported_speeds,
        vec![Speed::Fast, Speed::Ultrafast]
    );
}

#[test]
fn inherited_speeds_degrade_when_user_changes_builtin_to_non_openai_api() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        "[[models]]\nid = \"gpt-5.6\"\napi = \"anthropic-messages\"\nprovider = \"anthropic\"\n",
    )
    .unwrap();
    let catalog = ModelCatalog::load_from_path(&AppConfig::default(), path).unwrap();
    let entry = catalog.lookup("gpt-5.6").unwrap();
    assert_eq!(entry.api, "anthropic-messages");
    assert!(entry.supported_speeds.is_empty());
    assert_eq!(entry.resolve_speed(crate::core::llm::Speed::Fast), None);
}

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
    assert!(catalog.is_builtin_seed("gpt-5.4"));
    assert!(!catalog.is_user_model("gpt-5.4"));
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
    let kimi_k3 = catalog.lookup("kimi-k3").expect("builtin kimi-k3");
    assert_eq!(kimi_k3.api, "openai");
    assert_eq!(kimi_k3.provider, "moonshot");
    assert_eq!(kimi_k3.base_url.as_deref(), Some("https://api.moonshot.cn"));
}

#[test]
fn builtin_models_toml_parses() {
    let parsed =
        toml::from_str::<UserModelsFile>(builtin_seed_toml_text()).expect("parse embedded seed");
    assert_eq!(parsed.models.len(), 18);
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
            "kimi-k3",
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-fable-5",
            "claude-opus-5",
            "claude-sonnet-5",
            "claude-sonnet-4-6",
        ]
    );

    for id in [
        "claude-fable-5",
        "claude-opus-5",
        "claude-sonnet-5",
        "claude-sonnet-4-6",
        "claude-opus-4-8",
        "claude-opus-4-7",
        "claude-opus-4-6",
    ] {
        let entry = entries.iter().find(|entry| entry.id == id).unwrap();
        assert_eq!(entry.context_window, Some(1_000_000), "{id}");
        assert_eq!(
            entry.context_window_options,
            vec![400_000, 1_000_000],
            "{id}"
        );
        assert_eq!(entry.max_output_tokens, Some(128_000), "{id}");
    }

    let utility = entries
        .iter()
        .find(|entry| entry.id == "utility-flash")
        .expect("utility-flash preset");
    assert_eq!(utility.request_model_name(), "deepseek-v4-flash");
    assert_eq!(utility.context_window, Some(1_000_000));
    assert_eq!(utility.context_window_options, vec![400_000, 1_000_000]);
    assert_eq!(utility.max_output_tokens, Some(384_000));
    assert!(!utility.capabilities.web_search);
    assert_eq!(
        utility.supported_reasoning_levels,
        vec!["high".to_string(), "max".to_string()]
    );

    let kimi = entries
        .iter()
        .find(|entry| entry.id == "kimi-k2.7-code")
        .expect("kimi preset");
    assert_eq!(kimi.base_url.as_deref(), Some("https://api.moonshot.cn"));
    assert_eq!(kimi.provider, "moonshot");
    assert_eq!(kimi.context_window, Some(256_000));
    assert_eq!(kimi.supported_reasoning_levels, vec!["high".to_string()]);
    // Moonshot defines the ceiling as remaining context after the prompt, not
    // as a fixed model-wide number suitable for reserving input budget.
    assert_eq!(kimi.max_output_tokens, None);
    assert!(kimi.capabilities.vision);
    assert!(kimi.capabilities.files);

    let kimi_k3 = entries
        .iter()
        .find(|entry| entry.id == "kimi-k3")
        .expect("kimi-k3 preset");
    assert_eq!(kimi_k3.base_url.as_deref(), Some("https://api.moonshot.cn"));
    assert_eq!(kimi_k3.provider, "moonshot");
    assert_eq!(kimi_k3.context_window, Some(1_000_000));
    assert_eq!(kimi_k3.context_window_options, vec![400_000, 1_000_000]);
    assert_eq!(kimi_k3.max_output_tokens, Some(131_072));
    assert_eq!(
        kimi_k3.supported_reasoning_levels,
        vec!["low".to_string(), "high".to_string(), "max".to_string()]
    );
    assert_eq!(kimi_k3.thinking_format.as_deref(), Some("openai"));
    assert!(kimi_k3.capabilities.vision);
    assert!(kimi_k3.capabilities.files);

    let mimo = entries
        .iter()
        .find(|entry| entry.id == "mimo-v2.5-pro")
        .expect("mimo preset");
    assert_eq!(mimo.context_window, Some(1_000_000));
    assert_eq!(mimo.context_window_options, vec![400_000, 1_000_000]);
    assert_eq!(mimo.max_output_tokens, Some(128_000));
    assert_eq!(mimo.supported_reasoning_levels, vec!["high".to_string()]);

    let gpt = entries
        .iter()
        .find(|entry| entry.id == "gpt-5.6")
        .expect("gpt-5.6 preset");
    assert_eq!(
        gpt.supported_reasoning_levels,
        vec![
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
            "xhigh".to_string(),
            "max".to_string(),
        ]
    );
    assert_eq!(gpt.supported_speeds, vec![crate::core::llm::Speed::Fast]);
    assert_eq!(gpt.context_window_options, vec![400_000, 1_000_000]);
    assert_eq!(gpt.max_output_tokens, Some(128_000));

    let glm = entries
        .iter()
        .find(|entry| entry.id == "glm-5.2")
        .expect("glm-5.2 preset");
    assert_eq!(glm.context_window, Some(1_000_000));
    assert_eq!(glm.context_window_options, vec![400_000, 1_000_000]);
    assert_eq!(glm.max_output_tokens, Some(128_000));

    let claude = entries
        .iter()
        .find(|entry| entry.id == "claude-opus-4-8")
        .expect("claude-opus-4-8 preset");
    assert_eq!(
        claude.thinking_format.as_deref(),
        Some("anthropic-adaptive")
    );
    assert_eq!(
        claude.supported_reasoning_levels,
        vec![
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
            "xhigh".to_string(),
            "max".to_string(),
        ]
    );
    assert_eq!(claude.context_window, Some(1_000_000));
    assert_eq!(claude.max_output_tokens, Some(128_000));
    assert!(claude.capabilities.files);

    let embedded = builtin_seed_toml_text();
    assert!(embedded.contains("id = \"utility-flash\""));
    assert!(embedded.contains("model_name = \"deepseek-v4-flash\""));
    assert!(embedded.contains("base_url = \"https://api.moonshot.cn\""));
    assert!(embedded.contains("id = \"kimi-k3\""));
    assert!(embedded.contains("context_window = 1000000"));
    assert!(embedded.contains("supported_reasoning_levels = [\"low\", \"high\", \"max\"]"));
    assert!(embedded.contains("supported_reasoning_levels = [\"high\", \"max\"]"));
    assert!(embedded.contains("thinking_format = \"anthropic-adaptive\""));
}

#[test]
fn builtin_seed_entries_keep_embedded_context_window_when_runtime_default_changes() {
    let mut cfg = AppConfig::default();
    cfg.context.context_window_fallback = 200_000;
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
    assert_eq!(kimi.context_window, Some(256_000));

    let kimi_k3 = entries
        .iter()
        .find(|entry| entry.id == "kimi-k3")
        .expect("kimi-k3 preset");
    assert_eq!(kimi_k3.context_window, Some(1_000_000));

    let mimo = entries
        .iter()
        .find(|entry| entry.id == "mimo-v2.5-pro")
        .expect("mimo preset");
    assert_eq!(mimo.context_window, Some(1_000_000));

    let claude = entries
        .iter()
        .find(|entry| entry.id == "claude-opus-4-8")
        .expect("claude-opus-4-8 preset");
    assert_eq!(claude.context_window, Some(1_000_000));
    assert_eq!(claude.max_output_tokens, Some(128_000));
}

#[test]
fn custom_model_without_context_window_stays_unspecified() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "custom-hosted"
api = "openai-responses"
provider = "relay"
"#,
    )
    .unwrap();
    let catalog = ModelCatalog::load_from_path(&AppConfig::default(), path).expect("load catalog");
    let entry = catalog.lookup("custom-hosted").expect("custom entry");
    assert_eq!(entry.context_window, None);
    assert_eq!(entry.max_output_tokens, None);
}

#[test]
fn catalog_load_rejects_output_capacity_larger_than_context_window() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "invalid-capacity"
api = "openai-responses"
provider = "relay"
context_window = 4096
max_output_tokens = 8192
"#,
    )
    .unwrap();

    let error = ModelCatalog::load_from_path(&AppConfig::default(), path)
        .expect_err("catalog loading must reject an impossible capability declaration");
    assert!(error.to_string().contains("invalid-capacity"));
    assert!(error.to_string().contains("max_output_tokens"));
}

#[test]
fn invalid_user_context_options_degrade_without_preventing_catalog_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "custom-invalid-tier"
api = "openai-responses"
provider = "relay"
context_window = 400000
context_window_options = [200000, 1000000]
"#,
    )
    .unwrap();

    let catalog = ModelCatalog::load_from_path(&AppConfig::default(), path)
        .expect("an invalid user tier list must not prevent the catalog from loading");
    let entry = catalog
        .lookup("custom-invalid-tier")
        .expect("user model remains available");
    assert_eq!(entry.context_window, Some(400_000));
    assert!(entry.context_window_options.is_empty());
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
    assert!(catalog.is_builtin_seed("gpt-5.4"));
    assert!(catalog.is_user_model("gpt-5.4"));
}

#[test]
fn builtin_seed_and_user_model_flags_are_tracked_independently() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "gpt-5.4"
base_url = "https://seeded.override"

[[models]]
id = "custom-hosted"
api = "openai"
provider = "relay"
"#,
    )
    .unwrap();

    let cfg = AppConfig::default();
    let catalog = ModelCatalog::load_from_path(&cfg, path).expect("load merged catalog");

    assert!(catalog.is_builtin_seed("gpt-5.4"));
    assert!(catalog.is_user_model("gpt-5.4"));
    assert!(!catalog.is_builtin_seed("custom-hosted"));
    assert!(catalog.is_user_model("custom-hosted"));
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

#[test]
fn context_tiers_normalize_and_description_round_trip_through_catalog() {
    let description = "用于档位归一化测试的中转模型。";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        format!(
            r#"
[[models]]
id = "custom-tiered"
api = "openai-responses"
provider = "relay"
context_window = 400000
context_window_options = [1000000, 400000, 400000]
description = "{description}"
"#
        ),
    )
    .unwrap();

    let catalog = ModelCatalog::load_from_path(&AppConfig::default(), path).expect("load catalog");
    let entry = catalog
        .lookup("custom-tiered")
        .expect("tiered custom model");
    assert_eq!(entry.context_window_options, vec![400_000, 1_000_000]);
    assert_eq!(entry.description.as_deref(), Some(description));
}

#[test]
fn single_context_window_without_options_is_a_single_tier_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "custom-single-tier"
api = "openai-responses"
provider = "relay"
context_window = 200000
"#,
    )
    .unwrap();

    let catalog = ModelCatalog::load_from_path(&AppConfig::default(), path).expect("load catalog");
    assert!(catalog
        .lookup("custom-single-tier")
        .expect("single-tier custom model")
        .context_window_options
        .is_empty());
}

#[test]
fn tier_validation_rejects_a_default_that_is_not_selectable() {
    let error = crate::core::llm::catalog::validate_context_window_options(
        "invalid-builtin-tier",
        Some(400_000),
        &[1_000_000],
        None,
    )
    .expect_err("the default tier must be selectable");
    assert!(error.to_string().contains("invalid-builtin-tier"));
    assert!(error.to_string().contains("context_window_options"));
}
