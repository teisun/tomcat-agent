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
        let models_path = crate::core::llm::ModelCatalog::default_user_path(&AppConfig::default())
            .expect("models path");
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
fn path_export_targets_cover_shell_profiles() {
    let home = std::path::Path::new("/tmp/tomcat-home");
    assert_eq!(
        path_export_targets("/bin/zsh", home),
        vec![home.join(".zprofile"), home.join(".zshrc")]
    );
    assert_eq!(
        path_export_targets("/bin/bash", home),
        vec![home.join(".bashrc")]
    );
    assert_eq!(
        path_export_targets("/bin/fish", home),
        vec![home.join(".profile")]
    );
}

#[test]
#[serial(env_lock)]
fn install_canonical_symlink_creates_local_bin_and_points_to_exe() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("target").join("debug").join("tomcat");
    let local_bin = dir.path().join(".local").join("bin");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"#!/bin/sh\n").unwrap();

    let installed = install_canonical_symlink(&exe, &local_bin).expect("install symlink");
    let link = installed.expect("should create canonical command entry");
    assert_eq!(
        link,
        local_bin.join(if cfg!(windows) {
            "tomcat.exe"
        } else {
            "tomcat"
        })
    );
    assert!(local_bin.is_dir(), "local bin should be created");

    #[cfg(unix)]
    {
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            std::fs::canonicalize(&exe).unwrap()
        );
    }
    #[cfg(windows)]
    {
        assert!(
            link.exists(),
            "windows fallback should copy an .exe-like target"
        );
    }
}

#[cfg(unix)]
#[test]
#[serial(env_lock)]
fn install_canonical_symlink_replaces_existing_symlink_target() {
    let dir = tempfile::tempdir().unwrap();
    let old_exe = dir.path().join("target").join("debug").join("tomcat-old");
    let new_exe = dir.path().join("target").join("debug").join("tomcat-new");
    let local_bin = dir.path().join(".local").join("bin");
    let link = local_bin.join("tomcat");
    std::fs::create_dir_all(old_exe.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&local_bin).unwrap();
    std::fs::write(&old_exe, b"old").unwrap();
    std::fs::write(&new_exe, b"new").unwrap();
    std::os::unix::fs::symlink(&old_exe, &link).unwrap();

    let installed = install_canonical_symlink(&new_exe, &local_bin).expect("refresh symlink");
    assert_eq!(installed.as_deref(), Some(link.as_path()));
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        std::fs::canonicalize(&new_exe).unwrap()
    );
}

#[test]
#[serial(env_lock)]
fn install_canonical_symlink_skips_when_exe_is_already_in_local_bin() {
    let dir = tempfile::tempdir().unwrap();
    let local_bin = dir.path().join(".local").join("bin");
    let exe = local_bin.join(if cfg!(windows) {
        "tomcat.exe"
    } else {
        "tomcat"
    });
    std::fs::create_dir_all(&local_bin).unwrap();
    std::fs::write(&exe, b"release-binary").unwrap();

    let installed = install_canonical_symlink(&exe, &local_bin).expect("skip self-referential");
    assert!(
        installed.is_none(),
        "already canonical install should be skipped"
    );
    assert_eq!(std::fs::read(&exe).unwrap(), b"release-binary");
}

#[test]
#[serial(env_lock)]
fn install_canonical_symlink_preserves_existing_regular_file() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("target").join("debug").join("tomcat");
    let local_bin = dir.path().join(".local").join("bin");
    let existing = local_bin.join(if cfg!(windows) {
        "tomcat.exe"
    } else {
        "tomcat"
    });
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&local_bin).unwrap();
    std::fs::write(&exe, b"debug-binary").unwrap();
    std::fs::write(&existing, b"real-user-install").unwrap();

    let installed = install_canonical_symlink(&exe, &local_bin).expect("preserve regular file");
    assert!(installed.is_none(), "regular file should not be replaced");
    assert_eq!(std::fs::read(&existing).unwrap(), b"real-user-install");
}

#[test]
#[serial(env_lock)]
fn install_canonical_symlink_skips_target_deps_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir
        .path()
        .join("target")
        .join("debug")
        .join("deps")
        .join("tomcat-hash");
    let local_bin = dir.path().join(".local").join("bin");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"deps-binary").unwrap();

    let installed = install_canonical_symlink(&exe, &local_bin).expect("skip deps binary");
    assert!(installed.is_none(), "deps artifacts must not be linked");
    assert!(
        !local_bin.exists() || std::fs::read_dir(&local_bin).unwrap().next().is_none(),
        "deps skip should not create a populated local bin directory"
    );
}

#[test]
fn prune_stale_lines_removes_only_target_exports() {
    let content = r#"
# Added by tomcat init
export PATH="/tmp/project/target/release:$PATH"
# Added by tomcat init
export PATH="$HOME/.local/bin:$PATH"
export PATH="/usr/local/bin:$PATH"
# Added by tomcat init
export PATH="/tmp/project/target/debug/deps:$PATH"
"#;
    let pruned = prune_stale_lines(content);
    assert!(
        !pruned.contains("/target/release") && !pruned.contains("/target/debug/deps"),
        "target exports should be removed, got:\n{pruned}"
    );
    assert!(
        pruned.contains("export PATH=\"$HOME/.local/bin:$PATH\"")
            && pruned.contains("export PATH=\"/usr/local/bin:$PATH\""),
        "valid exports should remain, got:\n{pruned}"
    );
}

#[test]
#[serial(env_lock)]
fn auto_add_to_path_writes_zprofile_and_zshrc_without_touching_zshenv() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let zshenv = home.join(".zshenv");
    std::fs::write(&zshenv, ". \"$HOME/.cargo/env\"\n").unwrap();
    let _env = EnvVarGuard::set_many(&[("HOME", home.to_str()), ("SHELL", Some("/bin/zsh"))]);

    assert!(
        auto_add_to_path(home),
        "auto_add_to_path should succeed for zsh"
    );

    let zprofile = std::fs::read_to_string(home.join(".zprofile")).unwrap();
    let zshrc = std::fs::read_to_string(home.join(".zshrc")).unwrap();
    assert!(
        zprofile.contains("export PATH=\"$HOME/.local/bin:$PATH\"")
            && zshrc.contains("export PATH=\"$HOME/.local/bin:$PATH\""),
        "zprofile/zshrc should both receive the stable export"
    );
    assert_eq!(
        std::fs::read_to_string(&zshenv).unwrap(),
        ". \"$HOME/.cargo/env\"\n",
        ".zshenv should remain untouched"
    );
}

#[test]
#[serial(env_lock)]
fn auto_add_to_path_prunes_stale_target_exports_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let bashrc = home.join(".bashrc");
    std::fs::write(
        &bashrc,
        [
            "# Added by tomcat init",
            "export PATH=\"/tmp/project/target/release:$PATH\"",
            "# Added by tomcat init",
            "export PATH=\"/tmp/project/target/debug/deps:$PATH\"",
            "export PATH=\"/usr/local/bin:$PATH\"",
        ]
        .join("\n"),
    )
    .unwrap();
    let _env = EnvVarGuard::set_many(&[("HOME", home.to_str()), ("SHELL", Some("/bin/bash"))]);

    assert!(
        auto_add_to_path(home),
        "first auto_add_to_path should succeed"
    );
    assert!(
        auto_add_to_path(home),
        "second auto_add_to_path should stay idempotent"
    );

    let bashrc_content = std::fs::read_to_string(&bashrc).unwrap();
    assert!(
        !bashrc_content.contains("/target/"),
        "stale target exports should be pruned, got:\n{bashrc_content}"
    );
    assert_eq!(
        bashrc_content
            .matches("export PATH=\"$HOME/.local/bin:$PATH\"")
            .count(),
        1,
        "stable export should appear exactly once, got:\n{bashrc_content}"
    );
    assert!(
        bashrc_content.contains("export PATH=\"/usr/local/bin:$PATH\""),
        "user exports must be preserved, got:\n{bashrc_content}"
    );
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
        supported_reasoning_levels: Vec::new(),
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
fn preload_runtime_env_rejects_invalid_env_file() {
    let work_dir = tempfile::tempdir().expect("tempdir");
    let cfg = test_config(work_dir.path());
    let env_path = work_dir.path().join("assets").join(".env");
    std::fs::create_dir_all(env_path.parent().expect("env parent")).expect("mkdir assets");
    std::fs::write(&env_path, "BROKEN_ENV=\"unterminated\n").expect("write broken env");

    let error = preload_runtime_env(&cfg).expect_err("broken runtime env must fail");
    assert!(
        error.to_string().contains("加载"),
        "error should mention runtime env loading failure, got: {error}"
    );
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

    assert_eq!(
        extra_for_openai,
        vec![
            "ANTHROPIC_API_KEY".to_string(),
            "DEEPSEEK_API_KEY".to_string(),
            "MIMO_API_KEY".to_string(),
            "MOONSHOT_API_KEY".to_string(),
            "ZHIPU_API_KEY".to_string(),
        ]
    );
    assert_eq!(
        extra_for_deepseek,
        vec![
            "ANTHROPIC_API_KEY".to_string(),
            "MIMO_API_KEY".to_string(),
            "MOONSHOT_API_KEY".to_string(),
            "OPENAI_API_KEY".to_string(),
            "ZHIPU_API_KEY".to_string(),
        ]
    );
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
        supported_reasoning_levels: Vec::new(),
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
