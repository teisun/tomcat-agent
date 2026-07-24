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
use serial_test::serial;
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

// ── T2-P1-013：[tools.web_fetch] defaults / TOML override ────────────────────

#[test]
fn tools_web_fetch_config_default_value() {
    let cfg = ToolsWebFetchConfig::default();
    assert_eq!(cfg.max_redirects, DEFAULT_TOOLS_WEB_FETCH_MAX_REDIRECTS);
    assert_eq!(cfg.fetch_timeout_ms, DEFAULT_TOOLS_WEB_FETCH_TIMEOUT_MS);
    assert_eq!(
        cfg.max_http_content_bytes,
        DEFAULT_TOOLS_WEB_FETCH_MAX_HTTP_CONTENT_BYTES
    );
    assert_eq!(
        cfg.max_markdown_chars,
        DEFAULT_TOOLS_WEB_FETCH_MAX_MARKDOWN_CHARS
    );
    assert_eq!(
        cfg.markdown_head_chars,
        DEFAULT_TOOLS_WEB_FETCH_MARKDOWN_HEAD_CHARS
    );
    assert_eq!(cfg.cache_ttl_secs, DEFAULT_TOOLS_WEB_FETCH_CACHE_TTL_SECS);
    assert_eq!(
        cfg.cache_capacity_bytes,
        DEFAULT_TOOLS_WEB_FETCH_CACHE_CAPACITY_BYTES
    );
    assert!(!cfg.use_llm_processing);
}

#[test]
fn app_config_includes_tools_web_fetch_default() {
    let cfg = AppConfig::default();
    assert_eq!(
        cfg.tools.web_fetch.max_redirects,
        DEFAULT_TOOLS_WEB_FETCH_MAX_REDIRECTS
    );
    assert_eq!(
        cfg.tools.web_fetch.fetch_timeout_ms,
        DEFAULT_TOOLS_WEB_FETCH_TIMEOUT_MS
    );
}

#[test]
fn tools_web_fetch_toml_override() {
    let dir = std::env::temp_dir().join("tomcat_tools_web_fetch_cfg_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(
        br#"[tools.web_fetch]
max_redirects = 4
fetch_timeout_ms = 3210
max_http_content_bytes = 4096
max_markdown_chars = 2048
markdown_head_chars = 256
cache_ttl_secs = 33
cache_capacity_bytes = 8192
use_llm_processing = true
"#,
    )
    .unwrap();
    drop(f);
    let cfg = load_config(Some(path.as_path())).expect("load_config");
    assert_eq!(cfg.tools.web_fetch.max_redirects, 4);
    assert_eq!(cfg.tools.web_fetch.fetch_timeout_ms, 3210);
    assert_eq!(cfg.tools.web_fetch.max_http_content_bytes, 4096);
    assert_eq!(cfg.tools.web_fetch.max_markdown_chars, 2048);
    assert_eq!(cfg.tools.web_fetch.markdown_head_chars, 256);
    assert_eq!(cfg.tools.web_fetch.cache_ttl_secs, 33);
    assert_eq!(cfg.tools.web_fetch.cache_capacity_bytes, 8192);
    assert!(cfg.tools.web_fetch.use_llm_processing);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

// ── Case 5：[tools.bash] foreground observation policy ───────────────────────

#[test]
fn tools_bash_defaults_match_contract() {
    let cfg = AppConfig::default();
    assert_eq!(
        cfg.tools.bash.foreground_wait_ms,
        DEFAULT_TOOLS_BASH_FOREGROUND_WAIT_MS
    );
    assert_eq!(
        cfg.tools.bash.max_output_chars,
        DEFAULT_TOOLS_BASH_MAX_OUTPUT_CHARS
    );
}

#[test]
fn tools_bash_toml_override_loads_non_default_policy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[tools.bash]\nforeground_wait_ms = 9000\nmax_output_chars = 42000\n",
    )
    .unwrap();

    let cfg = load_config(Some(&path)).unwrap();
    assert_eq!(cfg.tools.bash.foreground_wait_ms, 9_000);
    assert_eq!(cfg.tools.bash.max_output_chars, 42_000);
}

#[test]
#[serial(env_lock)]
fn tools_bash_env_override_uses_nested_config_mapping() {
    const KEY: &str = "TOMCAT__TOOLS__BASH__FOREGROUND_WAIT_MS";
    let previous = std::env::var(KEY).ok();
    // SAFETY: serial(env_lock) excludes concurrent environment mutation in this test suite.
    unsafe { std::env::set_var(KEY, "9000") };

    let loaded = load_config(None);

    // SAFETY: restore the exact pre-test process environment while still holding env_lock.
    unsafe {
        match previous {
            Some(value) => std::env::set_var(KEY, value),
            None => std::env::remove_var(KEY),
        }
    }
    assert_eq!(loaded.unwrap().tools.bash.foreground_wait_ms, 9_000);
}

#[test]
fn tools_bash_rejects_legacy_timeout_field_with_replacement_hint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[tools.bash]\ntimeout_ms = 120000\n").unwrap();

    let message = load_config(Some(&path)).unwrap_err().to_string();
    assert!(message.contains("timeout_ms"), "{message}");
    assert!(message.contains("foreground_wait_ms"), "{message}");
}

#[test]
fn tools_bash_rejects_foreground_wait_outside_8_to_16_seconds() {
    for invalid in [7_999, 16_001] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            format!("[tools.bash]\nforeground_wait_ms = {invalid}\n"),
        )
        .unwrap();

        let message = load_config(Some(&path)).unwrap_err().to_string();
        assert!(
            message.contains("tools.bash.foreground_wait_ms"),
            "{message}"
        );
        assert!(message.contains("[8000, 16000]"), "{message}");
    }
}
