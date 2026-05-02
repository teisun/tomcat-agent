//! # `AppConfig` / `SecurityConfig` 默认值与序列化
//!
//! - 默认配置可通过 `serde_json::to_string` / `from_str` 完成 round-trip。
//! - `SecurityConfig::default` 不 panic。
//! - 仅传入 `{ "security": {} }` 时，缺省字段由 `default_*` 帮助函数填充
//!   （`enable_audit_log = true` / `audit_log_retention_days = 90`）。
//! - `AppConfig::default` 默认就包含 `context` 子配置。

use super::super::*;

#[test]
fn default_app_config_roundtrip() {
    let cfg = AppConfig::default();
    let j = serde_json::to_string(&cfg).unwrap();
    let _: AppConfig = serde_json::from_str(&j).unwrap();
}

#[test]
fn security_config_default() {
    let _ = SecurityConfig::default();
}

#[test]
fn deserialize_security_config_uses_default_helpers() {
    let s = r#"{"security":{}}"#;
    let cfg: AppConfig = serde_json::from_str(s).unwrap();
    assert!(cfg.security.enable_audit_log);
    assert_eq!(cfg.security.audit_log_retention_days, 90);
}

#[test]
fn app_config_includes_context() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.context.context_window, 400_000);
}
