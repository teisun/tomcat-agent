//! # `validate_config` 与 `resolve_workspace_roots_paths`
//!
//! 校验与 workspace 路径整理：
//!
//! - `validate_config_*`：日志级别 / 审计保留期 / LLM 代理 schema /
//!   workspace.workspace_roots 重复 / 不存在 / 全部存在 等多个等价类。
//! - `resolve_workspace_roots_skips_blank_entries`：仅含空白字符的路径会被
//!   过滤后再判定。

use super::super::*;

#[test]
fn validate_config_accepts_valid() {
    let mut cfg = AppConfig::default();
    cfg.log.level = "info".to_string();
    assert!(validate_config(&cfg).is_ok());
}

#[test]
fn validate_config_rejects_invalid_log_level() {
    let mut cfg = AppConfig::default();
    cfg.log.level = "invalid".to_string();
    assert!(validate_config(&cfg).is_err());
}

#[test]
fn validate_config_rejects_zero_audit_retention() {
    let mut cfg = AppConfig::default();
    cfg.security.audit_log_retention_days = 0;
    assert!(validate_config(&cfg).is_err());
}

#[test]
fn validate_config_rejects_invalid_checkpoint_retention() {
    let mut cfg = AppConfig::default();
    cfg.checkpoint.retention_max = 0;
    assert!(validate_config(&cfg).is_err());

    let mut cfg = AppConfig::default();
    cfg.checkpoint.retention_days = 0;
    assert!(validate_config(&cfg).is_err());
}

#[test]
fn validate_config_rejects_invalid_proxy() {
    let mut cfg = AppConfig::default();
    cfg.log.level = "info".to_string();
    cfg.llm.proxy = Some("socks5://127.0.0.1:1080".to_string());
    assert!(validate_config(&cfg).is_err());
    cfg.llm.proxy = Some("http://127.0.0.1:7890".to_string());
    assert!(validate_config(&cfg).is_ok());
    cfg.llm.proxy = Some("https://proxy.example.com".to_string());
    assert!(validate_config(&cfg).is_ok());
}

#[test]
fn validate_config_rejects_duplicate_workspace_roots() {
    let dir = tempfile::tempdir().unwrap();
    let c = std::fs::canonicalize(dir.path()).unwrap();
    let s = c.to_string_lossy().into_owned();
    let mut cfg = AppConfig::default();
    cfg.log.level = "info".to_string();
    cfg.workspace.workspace_roots = vec![s.clone(), s];
    assert!(validate_config(&cfg).is_err());
}

#[test]
fn validate_config_rejects_nonexistent_extra_root() {
    let mut cfg = AppConfig::default();
    cfg.log.level = "info".to_string();
    cfg.workspace
        .workspace_roots
        .push("/nonexistent/pi_workspace_root_test_path".to_string());
    assert!(validate_config(&cfg).is_err());
}

#[test]
fn validate_config_accepts_workspace_roots_when_dirs_exist() {
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.log.level = "info".to_string();
    cfg.workspace.workspace_roots = vec![
        d1.path().to_str().unwrap().to_string(),
        d2.path().to_str().unwrap().to_string(),
    ];
    assert!(validate_config(&cfg).is_ok());
}

#[test]
fn validate_config_accepts_llm_files_expires_after_zero_and_min_bound() {
    let mut cfg = AppConfig::default();
    cfg.llm.files.expires_after_seconds = 0;
    assert!(validate_config(&cfg).is_ok());
    cfg.llm.files.expires_after_seconds = 3600;
    assert!(validate_config(&cfg).is_ok());
}

#[test]
fn validate_config_rejects_llm_files_expires_after_out_of_range() {
    let mut cfg = AppConfig::default();
    cfg.llm.files.expires_after_seconds = 3599;
    assert!(validate_config(&cfg).is_err());
    cfg.llm.files.expires_after_seconds = 2_592_001;
    assert!(validate_config(&cfg).is_err());
}

#[test]
fn resolve_workspace_roots_skips_blank_entries() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.workspace.workspace_roots =
        vec!["  ".to_string(), dir.path().to_str().unwrap().to_string()];
    let roots = resolve_workspace_roots_paths(&cfg).unwrap();
    assert_eq!(roots.len(), 1);
}
