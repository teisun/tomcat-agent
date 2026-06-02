//! # 简单子命令运行路径
//!
//! 覆盖 `init` / `doctor` / `config get|set|edit` / `audit list/show/export` /
//! `plugin list`（启用审计与默认配置场景）等 happy-path：用例核心断言只是
//! 「不返回错误」，因为这些子命令多为本地副作用（创建目录、读写
//! sessions.json、写日志），用集成测试覆盖更细的字段。

use super::super::*;
use super::mocks::{test_config, with_temp_home, with_tomcat_config_in_home};
use serial_test::serial;
use std::collections::BTreeMap;

#[test]
#[serial(env_lock)]
fn run_init_returns_ok() {
    with_temp_home(|| {
        let r = run_init();
        assert!(r.is_ok());
    });
}

#[test]
#[serial(env_lock)]
fn run_init_writes_openai_responses_as_default_provider() {
    with_temp_home(|| {
        run_init().expect("init should succeed");

        let config_path = normalize_path(DEFAULT_CONFIG_PATH).expect("config path");
        let config_text = std::fs::read_to_string(&config_path).expect("config text");
        assert!(
            config_text.contains("provider = \"openai-responses\""),
            "generated config should default to openai-responses, got:\n{config_text}"
        );
    });
}

#[test]
fn run_doctor_returns_ok() {
    let r = run_doctor();
    assert!(r.is_ok());
}

#[test]
fn run_plugin_list_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_plugin(PluginSub::List, &cfg);
    assert!(r.is_ok());
}

#[test]
fn run_audit_list_returns_ok() {
    let cfg = AppConfig::default();
    let r = run_audit(AuditSub::List { limit: None }, &cfg);
    assert!(r.is_ok());
}

#[test]
fn run_config_get_with_key_returns_ok() {
    let cfg = AppConfig::default();
    let r = run_config(
        ConfigSub::Get {
            key: Some("log.level".to_string()),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_config_get_without_key_returns_ok() {
    let cfg = AppConfig::default();
    let r = run_config(ConfigSub::Get { key: None }, &cfg);
    assert!(r.is_ok());
}

#[test]
fn run_config_set_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    with_tomcat_config_in_home(dir.path(), || {
        let r = run_config(
            ConfigSub::Set {
                key: "log.level".to_string(),
                value: "debug".to_string(),
            },
            &cfg,
        );
        assert!(r.is_ok());

        let config_path = normalize_path(DEFAULT_CONFIG_PATH).expect("config path");
        let config_text = std::fs::read_to_string(&config_path).expect("config text");
        assert!(
            config_text.contains("level = \"debug\""),
            "config set should update temp HOME config only, got:\n{config_text}"
        );
    });
}

#[test]
#[serial(env_lock)]
fn run_config_edit_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    with_tomcat_config_in_home(dir.path(), || {
        let old_editor = std::env::var("EDITOR").ok();
        if cfg!(unix) {
            std::env::set_var("EDITOR", "true");
        } else {
            std::env::set_var("EDITOR", "cmd /c exit 0");
        }

        let r = run_config(ConfigSub::Edit, &cfg);

        match old_editor {
            Some(v) => std::env::set_var("EDITOR", v),
            None => std::env::remove_var("EDITOR"),
        }
        assert!(r.is_ok());
    });
}

#[test]
fn run_doctor_is_always_ok() {
    let r = run_doctor();
    assert!(r.is_ok());
}

#[test]
#[serial(env_lock)]
fn run_doctor_after_init_returns_ok() {
    with_temp_home(|| {
        run_init().unwrap();
        let r = run_doctor();
        assert!(r.is_ok());
    });
}

#[test]
fn run_audit_show_and_export_returns_ok() {
    let cfg = AppConfig::default();
    let r = run_audit(
        AuditSub::Show {
            id: "id1".to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
    let dir = tempfile::tempdir().unwrap();
    let r2 = run_audit(
        AuditSub::Export {
            path: dir.path().join("audit.json"),
        },
        &cfg,
    );
    assert!(r2.is_ok());
}

#[test]
fn apply_model_choice_updates_provider_and_key_env() {
    let mut cfg = AppConfig::default();
    let entry = crate::core::llm::ModelEntry {
        id: "deepseek-v4-pro".to_string(),
        api: "openai".to_string(),
        provider: "deepseek".to_string(),
        base_url: Some("https://api.deepseek.com".to_string()),
        capabilities: crate::core::llm::Capabilities::default(),
        context_window: None,
        cost: None,
        thinking_format: Some("deepseek".to_string()),
    };

    let choice = apply_model_choice(&mut cfg, &entry);
    assert_eq!(cfg.llm.default_model, "deepseek-v4-pro");
    assert_eq!(cfg.llm.provider, "openai");
    assert_eq!(
        cfg.llm.api_base.as_deref(),
        Some("https://api.deepseek.com")
    );
    assert_eq!(cfg.llm.api_key_env.as_deref(), Some("DEEPSEEK_API_KEY"));
    assert_eq!(choice.env_name, "DEEPSEEK_API_KEY");
}

#[test]
fn write_env_entries_writes_provider_keys() {
    let dir = tempfile::tempdir().unwrap();
    let env_path = dir.path().join(".env");
    let mut vars = BTreeMap::new();
    vars.insert(
        "DEEPSEEK_API_KEY".to_string(),
        "deepseek-secret".to_string(),
    );
    vars.insert("OPENAI_API_KEY".to_string(), "openai-secret".to_string());

    write_env_entries(&env_path, &vars).expect("write env entries");
    let content = std::fs::read_to_string(&env_path).expect("read env");
    assert!(content.contains("DEEPSEEK_API_KEY=deepseek-secret"));
    assert!(content.contains("OPENAI_API_KEY=openai-secret"));
    assert!(content.contains("HTTPS_PROXY"));
}

#[test]
fn additional_provider_env_names_skip_selected_provider_and_dedupe() {
    let cfg = AppConfig::default();
    let catalog = crate::core::llm::ModelCatalog::load_from_path(
        &cfg,
        tempfile::tempdir().unwrap().path().join("models.toml"),
    )
    .expect("load catalog");

    let extra_for_openai =
        super::super::init_model_wizard::additional_provider_env_names(&catalog, "openai");
    let extra_for_deepseek =
        super::super::init_model_wizard::additional_provider_env_names(&catalog, "deepseek");

    assert_eq!(extra_for_openai, vec!["DEEPSEEK_API_KEY".to_string()]);
    assert_eq!(extra_for_deepseek, vec!["OPENAI_API_KEY".to_string()]);
}

#[test]
fn apply_model_choice_skips_default_openai_base_url() {
    let mut cfg = AppConfig::default();
    let entry = crate::core::llm::ModelEntry {
        id: "gpt-5.4".to_string(),
        api: "openai-responses".to_string(),
        provider: "openai".to_string(),
        base_url: Some("https://api.openai.com".to_string()),
        capabilities: crate::core::llm::Capabilities::default(),
        context_window: None,
        cost: None,
        thinking_format: None,
    };

    apply_model_choice(&mut cfg, &entry);
    assert_eq!(cfg.llm.provider, "openai-responses");
    assert_eq!(cfg.llm.api_base, None);
    assert_eq!(cfg.llm.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
}

#[test]
fn run_audit_list_file_disabled_returns_ok() {
    let mut cfg = AppConfig::default();
    cfg.security.enable_audit_log = false;
    let r = run_audit(AuditSub::List { limit: None }, &cfg);
    assert!(r.is_ok());
}
