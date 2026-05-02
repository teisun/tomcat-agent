//! # `config get/set` 内部 TOML key 解析
//!
//! 覆盖 `config_cmd::resolve_toml_key` / `set_toml_key` 两个工具函数：
//!
//! - 嵌套 key 命中、找不到时返回 `None`/Err 并提示「不存在」。
//! - 字符串 / 整数 / 布尔三种类型的写入路径。
//! - 整数解析失败时报错文案包含「整数」。
//! - `config_file_path` / `config_set_with_real_file` 验证 `pi init` 后默认
//!   配置文件实际存在。

use super::super::*;
use serial_test::serial;

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
#[serial(env_lock)]
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
