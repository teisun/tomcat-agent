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
    let dir = std::env::temp_dir().join("pi_wasm_tools_cfg_test");
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
    let dir = std::env::temp_dir().join("pi_wasm_tools_write_cfg_test");
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
