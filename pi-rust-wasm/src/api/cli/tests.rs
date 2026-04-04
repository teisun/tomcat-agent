use super::*;
use crate::load_config_toml_file;
use crate::resolve_extra_roots_paths;
use crate::wire;
use std::sync::Mutex;

/// `pi workspace` 读写 `~/.pi_/pi.config.toml`；单测串行化并隔离 `HOME`，避免触碰真实用户目录。
static WORKSPACE_CLI_HOME_LOCK: Mutex<()> = Mutex::new(());

fn with_pi_config_in_home<R>(work_dir: &std::path::Path, f: impl FnOnce() -> R) -> R {
    let _lock = WORKSPACE_CLI_HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let pi = home.path().join(".pi_");
    std::fs::create_dir_all(&pi).unwrap();
    let mut c = AppConfig::default();
    c.log.level = "info".to_string();
    c.storage.work_dir = Some(work_dir.to_str().unwrap().to_string());
    std::fs::write(
        pi.join("pi.config.toml"),
        toml::to_string_pretty(&c).unwrap(),
    )
    .unwrap();
    let prev = std::env::var("HOME").ok();
    std::env::set_var("HOME", home.path());
    let out = f();
    match prev {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
    out
}

fn test_config(dir: &std::path::Path) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.to_str().unwrap().to_string());
    cfg
}

#[test]
fn cli_parse_init() {
    let cli = Cli::try_parse_from(["pi", "init"]).unwrap();
    let cmd = cli.command.expect("subcommand");
    assert!(matches!(cmd, Commands::Init));
}

#[test]
fn cli_parse_init_rejects_config_flag() {
    let r = Cli::try_parse_from(["pi", "init", "--config", "/tmp/pi.config.toml"]);
    assert!(r.is_err(), "--config should be rejected after removal");
}

#[test]
fn cli_parse_doctor() {
    let cli = Cli::try_parse_from(["pi", "doctor"]).unwrap();
    assert!(matches!(cli.command, Some(Commands::Doctor)));
}

#[test]
fn cli_parse_config_get() {
    let cli = Cli::try_parse_from(["pi", "config", "get"]).unwrap();
    let cmd = cli.command.unwrap();
    if let Commands::Config { sub } = cmd {
        assert!(matches!(sub, ConfigSub::Get { key: None }));
    }
}

#[test]
fn cli_parse_session_list() {
    let cli = Cli::try_parse_from(["pi", "session", "list"]).unwrap();
    let cmd = cli.command.unwrap();
    assert!(matches!(
        cmd,
        Commands::Session {
            sub: SessionSub::List
        }
    ));
}

#[test]
fn cli_parse_plugin_list() {
    let cli = Cli::try_parse_from(["pi", "plugin", "list"]).unwrap();
    let cmd = cli.command.unwrap();
    assert!(matches!(
        cmd,
        Commands::Plugin {
            sub: PluginSub::List
        }
    ));
}

#[test]
fn cli_parse_audit_list() {
    let cli = Cli::try_parse_from(["pi", "audit", "list"]).unwrap();
    let cmd = cli.command.unwrap();
    assert!(matches!(
        cmd,
        Commands::Audit {
            sub: AuditSub::List { limit: None }
        }
    ));
}

#[test]
fn cli_parse_default_chat() {
    let cli = Cli::try_parse_from(["pi"]).unwrap();
    assert!(cli.command.is_none());
}

#[test]
fn run_init_returns_ok() {
    let r = run_init();
    assert!(r.is_ok());
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
    let cfg = AppConfig::default();
    let r = run_config(
        ConfigSub::Set {
            key: "log.level".to_string(),
            value: "debug".to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_config_edit_returns_ok() {
    run_init().unwrap();

    let old_editor = std::env::var("EDITOR").ok();
    if cfg!(unix) {
        std::env::set_var("EDITOR", "true");
    } else {
        std::env::set_var("EDITOR", "cmd /c exit 0");
    }

    let cfg = AppConfig::default();
    let r = run_config(ConfigSub::Edit, &cfg);

    match old_editor {
        Some(v) => std::env::set_var("EDITOR", v),
        None => std::env::remove_var("EDITOR"),
    }
    assert!(r.is_ok());
}

#[test]
fn run_doctor_is_always_ok() {
    let r = run_doctor();
    assert!(r.is_ok());
}

// --- session tests (direct AppConfig, no env vars) ---

#[test]
fn run_session_list_empty_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_session(SessionSub::List, &cfg);
    assert!(r.is_ok());
}

#[test]
fn run_session_new_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_session(SessionSub::New, &cfg);
    assert!(r.is_ok());
}

#[test]
fn run_session_list_after_new_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let _ = run_session(SessionSub::New, &cfg);
    let r = run_session(SessionSub::List, &cfg);
    assert!(r.is_ok());
}

#[test]
fn run_session_switch_nonexistent_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_session(
        SessionSub::Switch {
            key: "nonexistent".to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_session_switch_existing_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let _ = run_session(SessionSub::New, &cfg);
    let r = run_session(
        SessionSub::Switch {
            key: crate::DEFAULT_SESSION_KEY.to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_session_delete_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let _ = run_session(SessionSub::New, &cfg);
    let r = run_session(
        SessionSub::Delete {
            key: crate::DEFAULT_SESSION_KEY.to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok(), "run_session(Delete) failed: {:?}", r);
}

#[test]
fn run_session_archive_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let _ = run_session(SessionSub::New, &cfg);
    let r = run_session(
        SessionSub::Archive {
            key: crate::DEFAULT_SESSION_KEY.to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_session_search_empty_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_session(SessionSub::Search { query: None }, &cfg);
    assert!(r.is_ok());
}

#[test]
fn run_session_search_with_query_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_session(
        SessionSub::Search {
            query: Some("q".to_string()),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

// --- workspace tests ---

#[test]
fn run_workspace_add_list_remove() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    with_pi_config_in_home(dir.path(), || {
        crate::ensure_work_dir_structure(&cfg).unwrap();

        let target = tempfile::tempdir().unwrap();
        let target_path = target.path().to_str().unwrap().to_string();

        let r = run_workspace(
            WorkspaceSub::Add {
                path: Some(target_path.clone()),
                cwd: false,
            },
            &cfg,
        );
        assert!(r.is_ok());

        let r = run_workspace(WorkspaceSub::List, &cfg);
        assert!(r.is_ok());

        let r = run_workspace(WorkspaceSub::Remove { path: target_path }, &cfg);
        assert!(r.is_ok());

        let r = run_workspace(WorkspaceSub::List, &cfg);
        assert!(r.is_ok());
    });
}

#[test]
fn run_workspace_add_nonexistent_fails() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    with_pi_config_in_home(dir.path(), || {
        crate::ensure_work_dir_structure(&cfg).unwrap();

        let r = run_workspace(
            WorkspaceSub::Add {
                path: Some("/nonexistent/path/should/fail".to_string()),
                cwd: false,
            },
            &cfg,
        );
        assert!(r.is_err());
    });
}

#[test]
fn run_workspace_add_duplicate_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    with_pi_config_in_home(dir.path(), || {
        crate::ensure_work_dir_structure(&cfg).unwrap();

        let target = tempfile::tempdir().unwrap();
        let target_path = target.path().to_str().unwrap().to_string();

        let r = run_workspace(
            WorkspaceSub::Add {
                path: Some(target_path.clone()),
                cwd: false,
            },
            &cfg,
        );
        assert!(r.is_ok());

        let r = run_workspace(
            WorkspaceSub::Add {
                path: Some(target_path),
                cwd: false,
            },
            &cfg,
        );
        assert!(r.is_ok());
    });
}

#[test]
fn run_workspace_add_cwd_adds_current_dir() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    with_pi_config_in_home(dir.path(), || {
        crate::ensure_work_dir_structure(&cfg).unwrap();

        let target = tempfile::tempdir().unwrap();
        let canon = std::fs::canonicalize(target.path()).unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(target.path()).unwrap();
        let r = run_workspace(
            WorkspaceSub::Add {
                path: None,
                cwd: true,
            },
            &cfg,
        );
        std::env::set_current_dir(&prev).unwrap();
        assert!(r.is_ok());

        let cfg_path = crate::normalize_path(DEFAULT_CONFIG_PATH).unwrap();
        let file_cfg = load_config_toml_file(&cfg_path).unwrap();
        let list = resolve_extra_roots_paths(&file_cfg).unwrap();
        assert!(list.iter().any(|p| p == &canon));
    });
}

#[test]
fn run_workspace_remove_nonexistent_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    with_pi_config_in_home(dir.path(), || {
        crate::ensure_work_dir_structure(&cfg).unwrap();

        let r = run_workspace(
            WorkspaceSub::Remove {
                path: "/some/path".to_string(),
            },
            &cfg,
        );
        assert!(r.is_ok());
    });
}

// --- plugin registry tests ---

#[test]
fn plugin_registry_load_save_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.json");

    let reg = load_plugin_registry(&path);
    assert!(reg.plugins.is_empty());

    let mut reg = PluginRegistryFile::default();
    reg.plugins.push(PluginRegistryEntry {
        id: "test-plugin".to_string(),
        path: "/some/path".to_string(),
        enabled: true,
        loaded_at: "2026-01-01T00:00:00Z".to_string(),
    });
    save_plugin_registry(&path, &reg).unwrap();

    let loaded = load_plugin_registry(&path);
    assert_eq!(loaded.plugins.len(), 1);
    assert_eq!(loaded.plugins[0].id, "test-plugin");
    assert!(loaded.plugins[0].enabled);
}

#[test]
fn plugin_registry_corrupt_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.json");
    std::fs::write(&path, "not valid json {{{").unwrap();

    let reg = load_plugin_registry(&path);
    assert!(reg.plugins.is_empty());
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

// --- doctor tests ---

#[test]
fn run_doctor_after_init_returns_ok() {
    run_init().unwrap();
    let r = run_doctor();
    assert!(r.is_ok());
}

// --- config get/set/edit tests ---

#[test]
fn resolve_toml_key_finds_nested() {
    let cfg = AppConfig::default();
    let val = toml::Value::try_from(&cfg).unwrap();
    let found = resolve_toml_key(&val, "log.level");
    assert!(found.is_some());
    assert_eq!(found.unwrap().as_str().unwrap(), "info");
}

#[test]
fn resolve_toml_key_returns_none_for_missing() {
    let cfg = AppConfig::default();
    let val = toml::Value::try_from(&cfg).unwrap();
    assert!(resolve_toml_key(&val, "nonexistent.key").is_none());
}

#[test]
fn set_toml_key_changes_string_value() {
    let cfg = AppConfig::default();
    let mut val = toml::Value::try_from(&cfg).unwrap();
    let r = set_toml_key(&mut val, "log.level", "debug");
    assert!(r.is_ok());
    let found = resolve_toml_key(&val, "log.level").unwrap();
    assert_eq!(found.as_str().unwrap(), "debug");
}

#[test]
fn set_toml_key_changes_integer_value() {
    let cfg = AppConfig::default();
    let mut val = toml::Value::try_from(&cfg).unwrap();
    let r = set_toml_key(&mut val, "security.audit_log_retention_days", "30");
    assert!(r.is_ok());
    let found = resolve_toml_key(&val, "security.audit_log_retention_days").unwrap();
    assert_eq!(found.as_integer().unwrap(), 30);
}

#[test]
fn set_toml_key_changes_bool_value() {
    let cfg = AppConfig::default();
    let mut val = toml::Value::try_from(&cfg).unwrap();
    let r = set_toml_key(&mut val, "log.file_enabled", "true");
    assert!(r.is_ok());
    let found = resolve_toml_key(&val, "log.file_enabled").unwrap();
    assert!(found.as_bool().unwrap());
}

#[test]
fn set_toml_key_rejects_nonexistent_path() {
    let cfg = AppConfig::default();
    let mut val = toml::Value::try_from(&cfg).unwrap();
    let r = set_toml_key(&mut val, "nonexistent.key", "val");
    assert!(r.is_err());
    let msg = r.unwrap_err().to_string();
    assert!(msg.contains("不存在"));
}

#[test]
fn set_toml_key_rejects_bad_integer() {
    let cfg = AppConfig::default();
    let mut val = toml::Value::try_from(&cfg).unwrap();
    let r = set_toml_key(
        &mut val,
        "security.audit_log_retention_days",
        "not_a_number",
    );
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("整数"));
}

#[test]
fn config_set_with_real_file() {
    run_init().unwrap();
    let config_path = crate::normalize_path(DEFAULT_CONFIG_PATH).unwrap();
    assert!(config_path.exists(), "config should exist after init");
}

#[test]
fn config_file_path_resolves_default() {
    let r = config_file_path();
    assert!(r.is_ok());
}

// --- parse_audit_line tests ---

#[test]
fn parse_audit_line_matches_primitive() {
    let line = r#"2025-03-10T12:00:00Z  INFO audit primitive operation=Read path_or_cmd=/tmp/foo plugin_id=p1 user_approved=true success=true"#;
    let entry = parse_audit_line(line, 0);
    assert!(entry.is_some());
    let e = entry.unwrap();
    assert_eq!(e.audit_type, wire::WIRE_AUDIT_PRIMITIVE);
    assert_eq!(e.success, "OK");
}

#[test]
fn parse_audit_line_matches_tool_call() {
    let line = r#"2025-03-10T12:00:00Z  INFO audit tool_call tool_name=run success=false"#;
    let entry = parse_audit_line(line, 1);
    assert!(entry.is_some());
    let e = entry.unwrap();
    assert_eq!(e.audit_type, wire::WIRE_TOOL_CALL);
    assert_eq!(e.success, "FAIL");
}

#[test]
fn parse_audit_line_matches_hostcall() {
    let line =
        r#"2025-03-10T12:00:00Z  INFO audit hostcall module=fs method=readFile success=true"#;
    let entry = parse_audit_line(line, 2);
    assert!(entry.is_some());
    assert_eq!(entry.unwrap().audit_type, wire::WIRE_AUDIT_HOSTCALL);
}

#[test]
fn parse_audit_line_returns_none_for_non_audit() {
    let line = "2025-03-10T12:00:00Z  INFO some other log line";
    assert!(parse_audit_line(line, 0).is_none());
}

#[test]
fn read_audit_entries_from_file_with_audit_lines() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("test.log");
    std::fs::write(
        &log,
        "line1\n2025-01-01 INFO audit primitive operation=Read success=true\nline3\n2025-01-02 INFO audit tool_call tool_name=x success=false\n",
    )
    .unwrap();
    let entries = read_audit_entries(&log, Some(10)).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].audit_type, wire::WIRE_TOOL_CALL);
    assert_eq!(entries[1].audit_type, wire::WIRE_AUDIT_PRIMITIVE);
}

#[test]
fn read_audit_entries_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("empty.log");
    std::fs::write(&log, "no audit here\njust logs\n").unwrap();
    let entries = read_audit_entries(&log, None).unwrap();
    assert!(entries.is_empty());
}

// --- plugin tests ---

#[test]
fn run_plugin_list_returns_ok_with_empty() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_plugin(PluginSub::List, &cfg);
    assert!(r.is_ok());
}

#[test]
fn run_plugin_load_nonexistent_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_plugin(
        PluginSub::Load {
            path: "/nonexistent/path/to/plugin".to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_plugin_info_not_found_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_plugin(
        PluginSub::Info {
            id: "nonexistent-plugin".to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_plugin_unload_not_found_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_plugin(
        PluginSub::Unload {
            id: "nonexistent-plugin".to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_plugin_enable_not_found_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_plugin(
        PluginSub::Enable {
            id: "nonexistent-plugin".to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_plugin_disable_not_found_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_plugin(
        PluginSub::Disable {
            id: "nonexistent-plugin".to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

// --- audit with enable_audit_log = false ---

#[test]
fn run_audit_list_file_disabled_returns_ok() {
    let mut cfg = AppConfig::default();
    cfg.security.enable_audit_log = false;
    let r = run_audit(AuditSub::List { limit: None }, &cfg);
    assert!(r.is_ok());
}

#[test]
fn audit_export_with_entries() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("test.log");
    std::fs::write(
        &log,
        "2025-01-01 INFO audit primitive operation=Read success=true\n",
    )
    .unwrap();
    let export_path = dir.path().join("out.json");
    let entries = read_audit_entries(&log, None).unwrap();
    assert!(!entries.is_empty());
    let json = serde_json::to_string_pretty(&entries).unwrap();
    std::fs::write(&export_path, &json).unwrap();
    let content = std::fs::read_to_string(&export_path).unwrap();
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed.len(), 1);
}
