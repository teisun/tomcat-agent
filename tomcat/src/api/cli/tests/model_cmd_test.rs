use serial_test::serial;

use super::super::models_toml::ensure_default_models_toml;
use super::super::*;
use super::mocks::{test_config, with_temp_home, with_tomcat_config_in_home};
use crate::core::llm::{list_model_views, ModelCatalog, ModelSource};

#[test]
#[serial(env_lock)]
fn run_model_add_list_remove_and_key_roundtrip() {
    with_temp_home(|| {
        let work_dir = tempfile::tempdir().expect("tempdir");
        let cfg = test_config(work_dir.path());

        run_model(
            ModelSub::Add {
                id: "claude-opus-gateway".to_string(),
                api: "anthropic-messages".to_string(),
                provider: "cli-gateway".to_string(),
                model_name: Some("claude-opus-4-6".to_string()),
                api_key_env: None,
                base_url: Some("https://api.example.test/v1".to_string()),
                vision: false,
                files: false,
                tools: true,
                reasoning: true,
                web_search: false,
                context_window: Some(200_000),
                thinking_format: Some("anthropic".to_string()),
            },
            &cfg,
        )
        .expect("add model");

        run_model(ModelSub::List, &cfg).expect("list models");

        let catalog = ModelCatalog::load(&cfg).expect("load catalog");
        let inserted = list_model_views(&catalog)
            .into_iter()
            .find(|entry| entry.id == "claude-opus-gateway")
            .expect("inserted model");
        assert_eq!(inserted.api_key_env, "CLI_GATEWAY_API_KEY");
        assert_eq!(inserted.context_window, Some(200_000));
        assert!(!inserted.key_present);

        run_model(
            ModelSub::Key {
                sub: ModelKeySub::Set {
                    provider: "cli-gateway".to_string(),
                    value: Some("relay-secret".to_string()),
                },
            },
            &cfg,
        )
        .expect("set key");
        run_model(
            ModelSub::Key {
                sub: ModelKeySub::List,
            },
            &cfg,
        )
        .expect("list keys");

        let env_text =
            std::fs::read_to_string(work_dir.path().join("assets").join(".env")).expect("env text");
        assert!(env_text.contains("CLI_GATEWAY_API_KEY=relay-secret"));

        let catalog = ModelCatalog::load(&cfg).expect("reload catalog after key");
        let keyed = list_model_views(&catalog)
            .into_iter()
            .find(|entry| entry.id == "claude-opus-gateway")
            .expect("keyed model");
        assert!(keyed.key_present);

        run_model(
            ModelSub::Remove {
                id: "claude-opus-gateway".to_string(),
            },
            &cfg,
        )
        .expect("remove model");
        let catalog = ModelCatalog::load(&cfg).expect("reload catalog after remove");
        assert!(catalog.lookup("claude-opus-gateway").is_none());
    });
}

#[test]
#[serial(env_lock)]
fn run_model_default_persists_selected_model_to_config() {
    let work_dir = tempfile::tempdir().expect("tempdir");
    with_tomcat_config_in_home(work_dir.path(), || {
        let cfg = test_config(work_dir.path());
        run_model(
            ModelSub::Default {
                model: "gpt-5.4".to_string(),
            },
            &cfg,
        )
        .expect("set default model");

        let home = std::env::var("HOME").expect("temp home");
        let config_text = std::fs::read_to_string(
            std::path::Path::new(&home)
                .join(".tomcat")
                .join("tomcat.config.toml"),
        )
        .expect("read config");
        assert!(config_text.contains("default_model = \"gpt-5.4\""));
    });
}

#[test]
#[serial(env_lock)]
fn run_model_default_returns_err_for_unknown_model() {
    let work_dir = tempfile::tempdir().expect("tempdir");
    with_tomcat_config_in_home(work_dir.path(), || {
        let cfg = test_config(work_dir.path());
        let error = run_model(
            ModelSub::Default {
                model: "missing-model".to_string(),
            },
            &cfg,
        )
        .expect_err("unknown model should fail");
        let message = error.to_string();
        assert!(
            message.contains("missing-model"),
            "error should mention invalid model id, got: {message}"
        );
    });
}

#[test]
#[serial(env_lock)]
fn run_model_list_marks_seeded_builtin_models_as_builtin() {
    with_temp_home(|| {
        let work_dir = tempfile::tempdir().expect("tempdir");
        let cfg = test_config(work_dir.path());
        ensure_default_models_toml(&cfg).expect("seed builtin models.toml");

        run_model(ModelSub::List, &cfg).expect("list models");

        let catalog = ModelCatalog::load(&cfg).expect("load catalog");
        let gpt = list_model_views(&catalog)
            .into_iter()
            .find(|entry| entry.id == "gpt-5.4")
            .expect("seeded builtin gpt-5.4");
        assert_eq!(gpt.source, ModelSource::Builtin);
    });
}
