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

#[test]
fn llm_files_default_expires_after_seconds_is_86400() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.llm.files.expires_after_seconds, 86_400);
}

#[test]
fn llm_timeout_defaults_match_four_layer_policy() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.llm.http_timeout_sec, 1_800);
    assert_eq!(cfg.llm.stream_timeout_sec, 180);
    assert_eq!(cfg.llm.non_stream_stale_timeout_sec, 300);
    assert_eq!(cfg.llm.http_read_timeout_sec, 120);
}

#[test]
fn checkpoint_config_defaults_are_wired_into_app_config() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.checkpoint.retention_max, 50);
    assert_eq!(cfg.checkpoint.retention_days, 7);
    assert!(cfg.preflight.auto_install_git);
}

#[test]
fn thinking_show_default_is_summary() {
    let cfg = AppConfig::default();
    assert!(cfg.llm.thinking.enabled, "thinking 默认仍应启用");
    assert!(
        matches!(cfg.llm.thinking.show, ThinkingDisplay::Summary),
        "ThinkingConfig::default().show 应为 summary"
    );
}
