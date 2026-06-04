//! # `[tools.read]` 配置（PR-RB · t1-config）
//!
//! 覆盖：
//! - `ToolsReadConfig::default().max_bytes` 与 `DEFAULT_TOOLS_READ_MAX_BYTES`
//!   常量同值（25 MiB）；
//! - `AppConfig::default().tools.read.max_bytes` 与上同值；
//! - 缺省 toml（无 `[tools]` 段）解析后回落到默认值；
//! - `[tools.read] max_bytes = N` 能正确覆盖默认值；
//! - `serde_json` round-trip 不丢字段。

use super::super::*;
use std::io::Write;

#[test]
fn tools_read_config_default_value() {
    let cfg = ToolsReadConfig::default();
    assert_eq!(cfg.max_bytes, DEFAULT_TOOLS_READ_MAX_BYTES);
    assert_eq!(cfg.max_bytes, 25 * 1024 * 1024);
}

#[test]
fn app_config_includes_tools_read_default() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.tools.read.max_bytes, DEFAULT_TOOLS_READ_MAX_BYTES);
}

#[test]
fn deserialize_missing_tools_section_uses_default() {
    let s = r#"{}"#;
    let cfg: AppConfig = serde_json::from_str(s).unwrap();
    assert_eq!(cfg.tools.read.max_bytes, DEFAULT_TOOLS_READ_MAX_BYTES);
}

#[test]
fn deserialize_empty_tools_section_uses_default() {
    let s = r#"{"tools":{}}"#;
    let cfg: AppConfig = serde_json::from_str(s).unwrap();
    assert_eq!(cfg.tools.read.max_bytes, DEFAULT_TOOLS_READ_MAX_BYTES);
}

#[test]
fn deserialize_empty_tools_read_section_uses_default() {
    let s = r#"{"tools":{"read":{}}}"#;
    let cfg: AppConfig = serde_json::from_str(s).unwrap();
    assert_eq!(cfg.tools.read.max_bytes, DEFAULT_TOOLS_READ_MAX_BYTES);
}

#[test]
fn tools_read_max_bytes_toml_override() {
    let dir = std::env::temp_dir().join("tomcat_tools_cfg_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(b"[tools.read]\nmax_bytes = 1048576\n").unwrap();
    drop(f);
    let cfg = load_config(Some(path.as_path())).expect("load_config");
    assert_eq!(cfg.tools.read.max_bytes, 1_048_576);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn app_config_default_roundtrip_preserves_tools_read() {
    let cfg = AppConfig::default();
    let j = serde_json::to_string(&cfg).unwrap();
    let back: AppConfig = serde_json::from_str(&j).unwrap();
    assert_eq!(back.tools.read.max_bytes, DEFAULT_TOOLS_READ_MAX_BYTES);
}

// ── T2-P0-016 PR-G：[tools.write] normalize_crlf ─────────────────────────────

#[test]
fn tools_write_config_default_value() {
    let cfg = ToolsWriteConfig::default();
    assert!(cfg.normalize_crlf);
    assert_eq!(cfg.normalize_crlf, DEFAULT_TOOLS_WRITE_NORMALIZE_CRLF);
}

#[test]
fn app_config_includes_tools_write_default() {
    let cfg = AppConfig::default();
    assert!(cfg.tools.write.normalize_crlf);
}

#[test]
fn deserialize_empty_tools_write_section_uses_default() {
    let s = r#"{"tools":{"write":{}}}"#;
    let cfg: AppConfig = serde_json::from_str(s).unwrap();
    assert!(cfg.tools.write.normalize_crlf);
}

#[test]
fn tools_write_normalize_crlf_toml_override_off() {
    let dir = std::env::temp_dir().join("tomcat_tools_write_cfg_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(b"[tools.write]\nnormalize_crlf = false\n")
        .unwrap();
    drop(f);
    let cfg = load_config(Some(path.as_path())).expect("load_config");
    assert!(!cfg.tools.write.normalize_crlf);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn app_config_default_roundtrip_preserves_tools_write() {
    let cfg = AppConfig::default();
    let j = serde_json::to_string(&cfg).unwrap();
    let back: AppConfig = serde_json::from_str(&j).unwrap();
    assert!(back.tools.write.normalize_crlf);
}

// ── T2-P1-012：[tools.web_search] defaults / TOML override ───────────────────

#[test]
fn tools_web_search_config_default_value() {
    let cfg = ToolsWebSearchConfig::default();
    assert_eq!(cfg.backend, DEFAULT_TOOLS_WEB_SEARCH_BACKEND);
    assert_eq!(cfg.count, DEFAULT_TOOLS_WEB_SEARCH_COUNT);
    assert_eq!(cfg.cache_ttl_secs, DEFAULT_TOOLS_WEB_SEARCH_CACHE_TTL_SECS);
    assert_eq!(cfg.cache_capacity, DEFAULT_TOOLS_WEB_SEARCH_CACHE_CAPACITY);
    assert_eq!(cfg.timeout_ms, DEFAULT_TOOLS_WEB_SEARCH_TIMEOUT_MS);
    assert_eq!(
        cfg.tavily_base_url,
        DEFAULT_TOOLS_WEB_SEARCH_TAVILY_BASE_URL
    );
    assert_eq!(cfg.brave_base_url, DEFAULT_TOOLS_WEB_SEARCH_BRAVE_BASE_URL);
    assert_eq!(
        cfg.serper_base_url,
        DEFAULT_TOOLS_WEB_SEARCH_SERPER_BASE_URL
    );
}

#[test]
fn app_config_includes_tools_web_search_default() {
    let cfg = AppConfig::default();
    assert_eq!(
        cfg.tools.web_search.backend,
        DEFAULT_TOOLS_WEB_SEARCH_BACKEND
    );
    assert_eq!(cfg.tools.web_search.count, DEFAULT_TOOLS_WEB_SEARCH_COUNT);
}

#[test]
fn tools_web_search_toml_override() {
    let dir = std::env::temp_dir().join("tomcat_tools_web_search_cfg_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(
        br#"[tools.web_search]
backend = "brave"
count = 7
cache_ttl_secs = 42
cache_capacity = 9
timeout_ms = 3456
tavily_base_url = "http://127.0.0.1:3001"
brave_base_url = "http://127.0.0.1:3002"
serper_base_url = "http://127.0.0.1:3003"
"#,
    )
    .unwrap();
    drop(f);
    let cfg = load_config(Some(path.as_path())).expect("load_config");
    assert_eq!(cfg.tools.web_search.backend, "brave");
    assert_eq!(cfg.tools.web_search.count, 7);
    assert_eq!(cfg.tools.web_search.cache_ttl_secs, 42);
    assert_eq!(cfg.tools.web_search.cache_capacity, 9);
    assert_eq!(cfg.tools.web_search.timeout_ms, 3456);
    assert_eq!(
        cfg.tools.web_search.tavily_base_url,
        "http://127.0.0.1:3001"
    );
    assert_eq!(cfg.tools.web_search.brave_base_url, "http://127.0.0.1:3002");
    assert_eq!(
        cfg.tools.web_search.serper_base_url,
        "http://127.0.0.1:3003"
    );
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}
