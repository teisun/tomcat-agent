//! # 简单子命令运行路径
//!
//! 覆盖 `init` / `doctor` / `config get|set|edit` / `audit list/show/export` /
//! `plugin list`（启用审计与默认配置场景）等 happy-path：用例核心断言只是
//! 「不返回错误」，因为这些子命令多为本地副作用（创建目录、读写
//! sessions.json、写日志），用集成测试覆盖更细的字段。

use super::super::*;
use super::mocks::test_config;
use serial_test::serial;

#[test]
#[serial(env_lock)]
fn run_init_returns_ok() {
    let _home = crate::test_support::home_env_lock().lock().unwrap();
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
#[serial(env_lock)]
fn run_config_edit_returns_ok() {
    let _home = crate::test_support::home_env_lock().lock().unwrap();
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

#[test]
#[serial(env_lock)]
fn run_doctor_after_init_returns_ok() {
    let _home = crate::test_support::home_env_lock().lock().unwrap();
    run_init().unwrap();
    let r = run_doctor();
    assert!(r.is_ok());
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
fn run_audit_list_file_disabled_returns_ok() {
    let mut cfg = AppConfig::default();
    cfg.security.enable_audit_log = false;
    let r = run_audit(AuditSub::List { limit: None }, &cfg);
    assert!(r.is_ok());
}
