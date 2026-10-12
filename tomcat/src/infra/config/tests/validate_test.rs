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
fn mcp_runtime_validation_rejects_bad_values_and_accepts_bounds() {
    for (name, invalid) in [
        ("startup_timeout_ms", 0),
        ("startup_timeout_ms", u64::MAX),
        ("call_timeout_ms", 0),
        ("call_timeout_ms", u64::MAX),
    ] {
        let mut cfg = AppConfig::default();
        match name {
            "startup_timeout_ms" => cfg.connector.mcp.startup_timeout_ms = invalid,
            _ => cfg.connector.mcp.call_timeout_ms = invalid,
        }
        assert!(validate_config(&cfg)
            .unwrap_err()
            .to_string()
            .contains(name));
    }
    for invalid in [0, 65] {
        let mut cfg = AppConfig::default();
        cfg.connector.mcp.max_concurrent_calls = invalid;
        assert!(validate_config(&cfg)
            .unwrap_err()
            .to_string()
            .contains("max_concurrent_calls"));
    }
    for valid in [1, 64] {
        let mut cfg = AppConfig::default();
        cfg.connector.mcp.max_concurrent_calls = valid;
        assert!(validate_config(&cfg).is_ok());
    }
    for invalid in [
        "[connector.mcp]\ncall_timeout_ms = -1\n",
        "[connector.mcp]\nmax_concurrent_calls = \"many\"\n",
    ] {
        assert!(toml::from_str::<AppConfig>(invalid).is_err());
    }
}

#[test]
fn validate_config_rejects_invalid_session_default_mode() {
    let mut cfg = AppConfig::default();
    cfg.session.default_mode = "invalid".to_string();
    assert!(validate_config(&cfg).is_err());
}

#[test]
fn project_resource_dir_allows_only_project_relative_non_legacy_paths() {
    let project = tempfile::tempdir().unwrap();
    let cfg = AppConfig::default();
    assert_eq!(
        resolve_project_resource_dir(&cfg, project.path()).unwrap(),
        project.path().join(".agents")
    );

    let mut nested = AppConfig::default();
    nested.workspace.project_resource_dir = "tools/agents".to_string();
    assert_eq!(
        resolve_project_resource_dir(&nested, project.path()).unwrap(),
        project.path().join("tools/agents")
    );

    for invalid in [
        "",
        " ",
        ".",
        "..",
        "./agents",
        "tools/./agents",
        "../agents",
        "/tmp/agents",
        "~/agents",
        ".tomcat",
        ".tomcat/skills",
        ".tomcat\\skills",
        "tools/.tomcat",
    ] {
        let mut cfg = AppConfig::default();
        cfg.workspace.project_resource_dir = invalid.to_string();
        assert!(
            validate_config(&cfg).is_err(),
            "{invalid:?} must be rejected"
        );
        assert!(
            resolve_project_resource_dir(&cfg, project.path()).is_err(),
            "resolver must not bypass validation for {invalid:?}"
        );
    }
}

#[test]
fn validate_config_accepts_custom_web_search_plugin_backend() {
    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "mimo".to_string();
    assert!(validate_config(&cfg).is_ok());
}

#[test]
fn validate_config_rejects_invalid_web_fetch_limits() {
    let mut cfg = AppConfig::default();
    cfg.tools.web_fetch.fetch_timeout_ms = 0;
    assert!(validate_config(&cfg).is_err());

    let mut cfg = AppConfig::default();
    cfg.tools.web_fetch.max_redirects = 0;
    assert!(validate_config(&cfg).is_err());

    let mut cfg = AppConfig::default();
    cfg.tools.web_fetch.cache_capacity_bytes = 0;
    assert!(validate_config(&cfg).is_err());
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
    for url in [
        "socks5://127.0.0.1:1080",
        "ftp://user:PRIVATE_PROXY_SENTINEL@proxy.example.com",
    ] {
        cfg.llm.proxy = Some(url.to_string());
        let error = validate_config(&cfg).expect_err("unsupported scheme must fail");
        assert!(matches!(error, crate::AppError::Config(_)));
        assert!(error.to_string().contains(&crate::infra::i18n::tr_in(
            crate::infra::i18n::Locale::En,
            "config.urlScheme",
            &[("field", "llm.proxy")],
        )));
        assert!(!error.to_string().contains(url));
        assert!(!error.to_string().contains("PRIVATE_PROXY_SENTINEL"));
        assert_eq!(cfg.llm.proxy.as_deref(), Some(url));
    }
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
fn validate_config_accepts_three_layer_timeouts_in_range() {
    let mut cfg = AppConfig::default();
    cfg.llm.stream_timeout_sec = 180;
    cfg.llm.non_stream_stale_timeout_sec = 300;
    cfg.llm.http_read_timeout_sec = 120;
    assert!(validate_config(&cfg).is_ok());
}

#[test]
fn validate_config_rejects_three_layer_timeouts_out_of_range() {
    let mut cfg = AppConfig::default();
    cfg.llm.stream_timeout_sec = 4;
    assert!(validate_config(&cfg).is_err());

    let mut cfg = AppConfig::default();
    cfg.llm.non_stream_stale_timeout_sec = 4;
    assert!(validate_config(&cfg).is_err());

    let mut cfg = AppConfig::default();
    cfg.llm.http_read_timeout_sec = 4;
    assert!(validate_config(&cfg).is_err());
}

#[test]
fn resolve_workspace_roots_skips_blank_entries() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.workspace.workspace_roots =
        vec!["  ".to_string(), dir.path().to_str().unwrap().to_string()];
    let roots = resolve_workspace_roots_paths(&cfg).unwrap();
    assert_eq!(roots.len(), 2, "用户根 + 内置 ~/.tomcat/temp");
}

#[test]
fn resolve_workspace_roots_always_includes_dot_tomcat_temp() {
    let cfg = AppConfig::default();
    let roots = crate::resolve_workspace_roots_paths(&cfg).unwrap();
    let temp = crate::resolve_dot_tomcat_temp_dir().unwrap();
    let temp_canon = std::fs::canonicalize(&temp).unwrap_or(temp);
    assert!(
        roots.iter().any(|r| r == &temp_canon),
        "应始终包含 ~/.tomcat/temp，实际: {:?}",
        roots
    );
}
