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

struct EnvVarGuard {
    saved: Vec<(String, Option<String>)>,
}

impl EnvVarGuard {
    fn set_many(entries: &[(&str, Option<&str>)]) -> Self {
        let mut saved = Vec::new();
        for (key, value) in entries {
            saved.push(((*key).to_string(), std::env::var(key).ok()));
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        Self { saved }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            match value {
                Some(value) => std::env::set_var(&key, value),
                None => std::env::remove_var(&key),
            }
        }
    }
}

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
fn run_init_writes_default_model_without_legacy_llm_connection_fields() {
    with_temp_home(|| {
        run_init().expect("init should succeed");

        let config_path = normalize_path(DEFAULT_CONFIG_PATH).expect("config path");
        let config_text = std::fs::read_to_string(&config_path).expect("config text");
        assert!(
            !config_text.contains("provider = "),
            "generated config should not persist legacy llm.provider, got:\n{config_text}"
        );
        assert!(
            config_text.contains("[context]"),
            "generated config should persist context overrides, got:\n{config_text}"
        );
        assert!(
            config_text.contains("compaction_model = \"gpt-5.4\""),
            "generated config should align compaction model with init default model, got:\n{config_text}"
        );
    });
}

#[test]
#[serial(env_lock)]
fn run_init_migrates_legacy_llm_connection_fields_in_existing_config() {
    with_temp_home(|| {
        let config_path = normalize_path(DEFAULT_CONFIG_PATH).expect("config path");
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).expect("create config parent");
        }
        std::fs::write(
            &config_path,
            "[llm]\nprovider = \"openai-responses\"\ndefault_model = \"gpt-5.4\"\n",
        )
        .expect("write legacy config");

        run_init().expect("init should migrate legacy llm connection fields");
        let config_text = std::fs::read_to_string(&config_path).expect("config text");
        assert!(
            !config_text.contains("provider = "),
            "init should drop legacy llm.provider from rewritten config, got:\n{config_text}"
        );
        let models_path =
            crate::core::llm::ModelCatalog::default_user_path(&AppConfig::default()).expect("models path");
        let models_text = std::fs::read_to_string(models_path).expect("models.toml text");
        assert!(models_text.contains("id = \"gpt-5.2\""));
    });
}

#[test]
#[serial(env_lock)]
fn run_init_installs_builtin_web_search_backends_plugin() {
    with_temp_home(|| {
        run_init().expect("init should succeed");

        let plugins_dir = crate::resolve_plugins_dir(&AppConfig::default()).expect("plugins dir");
        let plugin_dir = plugins_dir.join("web-search-backends");
        assert!(plugin_dir.join("plugin.json").exists());
        assert!(plugin_dir.join("main.js").exists());
        assert!(plugin_dir.join("README.md").exists());
    });
}

#[test]
#[serial(env_lock)]
fn run_init_resets_sessions_store_to_new_shape() {
    with_temp_home(|| {
        let sessions_dir =
            crate::resolve_sessions_dir(&AppConfig::default()).expect("sessions dir");
        std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");
        std::fs::write(
            sessions_dir.join("sessions.json"),
            r#"{
  "agent:main:main": {
    "sessionId": "legacy_1",
    "updatedAt": 42
  }
}"#,
        )
        .expect("seed legacy store");

        run_init().expect("init should succeed");

        let store_text =
            std::fs::read_to_string(sessions_dir.join("sessions.json")).expect("store text");
        let store: crate::SessionStore =
            serde_json::from_str(&store_text).expect("new session store shape");
        assert!(
            store.is_empty(),
            "init should overwrite sessions.json with new shape"
        );
    });
}

#[test]
#[serial(env_lock)]
fn run_init_keeps_existing_sessions_store() {
    with_temp_home(|| {
        let sessions_dir =
            crate::resolve_sessions_dir(&AppConfig::default()).expect("sessions dir");
        std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");
        std::fs::write(
            sessions_dir.join("sessions.json"),
            r#"{
  "sessions": {
    "1781192456203_deadbeefcafebabe": {
      "sessionKey": "agent:main:proj:10c22afba719d09d",
      "sessionId": "1781192456203_deadbeefcafebabe",
      "updatedAt": 1781192660216,
      "sessionFile": "/tmp/1781192456203_deadbeefcafebabe.jsonl",
      "cwd": "/Users/demo/.tomcat/temp/project7"
    }
  },
  "current": {
    "agent:main:proj:10c22afba719d09d": "1781192456203_deadbeefcafebabe"
  }
}"#,
        )
        .expect("seed valid store");

        run_init().expect("init should succeed");

        let store_text =
            std::fs::read_to_string(sessions_dir.join("sessions.json")).expect("store text");
        let store: crate::SessionStore =
            serde_json::from_str(&store_text).expect("existing store should remain valid");
        assert_eq!(store.len(), 1, "init should preserve existing sessions");
        assert_eq!(
            store
                .current
                .get("agent:main:proj:10c22afba719d09d")
                .map(String::as_str),
            Some("1781192456203_deadbeefcafebabe")
        );
        assert_eq!(
            store
                .sessions
                .get("1781192456203_deadbeefcafebabe")
                .and_then(|entry| entry.cwd.as_deref()),
            Some("/Users/demo/.tomcat/temp/project7")
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
fn doctor_plugin_runtime_lines_report_success_exactly() {
    let lines = crate::api::cli::init::doctor_plugin_runtime_lines(Ok(()));
    assert_eq!(lines, vec!["✓ rquickjs 运行时：可用".to_string()]);
}

#[test]
fn doctor_plugin_runtime_lines_report_failure_and_hint() {
    let lines = crate::api::cli::init::doctor_plugin_runtime_lines(Err(AppError::Plugin(
        "boom".to_string(),
    )));
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], "✗ rquickjs 运行时：初始化失败 (插件错误: boom)");
    assert!(
        lines[1].contains("重新运行 tomcat init"),
        "failure hint should guide the user toward recovery: {}",
        lines[1]
    );
}

#[test]
#[serial(env_lock)]
fn doctor_proxy_lines_reports_env_proxy_without_llm_override() {
    let _env = EnvVarGuard::set_many(&[
        ("HTTPS_PROXY", Some("http://127.0.0.1:7890")),
        ("HTTP_PROXY", None),
        ("ALL_PROXY", None),
    ]);
    let mut cfg = AppConfig::default();
    cfg.llm.proxy = None;

    let lines = crate::api::cli::init::doctor_proxy_lines(&cfg);
    assert!(lines
        .iter()
        .any(|line| line.contains("环境代理") && line.contains("HTTPS_PROXY")));
}

#[test]
#[serial(env_lock)]
fn doctor_proxy_lines_warn_on_whitespace_and_socks() {
    let _env = EnvVarGuard::set_many(&[
        ("HTTPS_PROXY", Some("http://127.0.0.1:7890 ")),
        ("HTTP_PROXY", None),
        ("ALL_PROXY", Some("socks5://127.0.0.1:7890")),
    ]);
    let mut cfg = AppConfig::default();
    cfg.llm.proxy = Some("http://127.0.0.1:8888 ".to_string());

    let lines = crate::api::cli::init::doctor_proxy_lines(&cfg);
    assert!(lines.iter().any(|line| line.contains("llm.proxy 已配置")));
    assert!(lines
        .iter()
        .any(|line| line.contains("llm.proxy 含首尾空格")));
    assert!(lines
        .iter()
        .any(|line| line.contains("HTTPS_PROXY 含首尾空格")));
    assert!(lines
        .iter()
        .any(|line| line.contains("reqwest socks feature")));
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
        model_name: None,
        api: "openai".to_string(),
        provider: "deepseek".to_string(),
        api_key_env: None,
        base_url: Some("https://api.deepseek.com".to_string()),
        capabilities: crate::core::llm::Capabilities::default(),
        context_window: None,
        cost: None,
        thinking_format: Some("deepseek".to_string()),
    };

    let choice = apply_model_choice(&mut cfg, &entry);
    assert_eq!(cfg.llm.default_model, "deepseek-v4-pro");
    assert_eq!(cfg.context.compaction_model, "deepseek-v4-pro");
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
        super::super::init_model_wizard::additional_provider_env_names(&catalog, "OPENAI_API_KEY");
    let extra_for_deepseek = super::super::init_model_wizard::additional_provider_env_names(
        &catalog,
        "DEEPSEEK_API_KEY",
    );

    assert_eq!(extra_for_openai, vec!["DEEPSEEK_API_KEY".to_string()]);
    assert_eq!(extra_for_deepseek, vec!["OPENAI_API_KEY".to_string()]);
}

#[test]
fn apply_model_choice_skips_default_openai_base_url() {
    let mut cfg = AppConfig::default();
    let entry = crate::core::llm::ModelEntry {
        id: "gpt-5.4".to_string(),
        model_name: None,
        api: "openai-responses".to_string(),
        provider: "openai".to_string(),
        api_key_env: None,
        base_url: Some("https://api.openai.com".to_string()),
        capabilities: crate::core::llm::Capabilities::default(),
        context_window: None,
        cost: None,
        thinking_format: None,
    };

    apply_model_choice(&mut cfg, &entry);
    assert_eq!(cfg.context.compaction_model, "gpt-5.4");
    assert_eq!(cfg.llm.default_model, "gpt-5.4");
}

#[test]
fn run_audit_list_file_disabled_returns_ok() {
    let mut cfg = AppConfig::default();
    cfg.security.enable_audit_log = false;
    let r = run_audit(AuditSub::List { limit: None }, &cfg);
    assert!(r.is_ok());
}
