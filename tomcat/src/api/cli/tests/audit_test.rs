//! # 审计日志解析与导出
//!
//! 覆盖 `audit_cmd::parse_audit_line` / `read_audit_entries` 两个工具函数：
//!
//! - `parse_audit_line` 能区分 primitive / tool_call / hostcall 三种审计行；
//!   `success=true|false` 映射为 `OK|FAIL`；非审计日志返回 `None`。
//! - `read_audit_entries` 仅保留含 `audit ` 关键字的行，按时间逆序返回；
//!   空文件 / 无审计行返回空 `Vec`。
//! - `audit_export_with_entries`：把 `read_audit_entries` 输出落盘成 JSON 数组
//!   并验证可被 `serde_json` 反序列化。

use super::super::*;
use crate::wire;

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
fn parse_audit_line_handles_multibyte_detail_without_panic() {
    let detail = "修".repeat(90);
    let line =
        format!("2025-03-10T12:00:00Z  INFO audit primitive operation={detail} success=true");
    let entry = parse_audit_line(&line, 3).unwrap();
    assert_eq!(entry.audit_type, wire::WIRE_AUDIT_PRIMITIVE);
    assert_eq!(entry.success, "OK");
    assert!(entry.detail.starts_with("operation="));
    assert!(entry.detail.chars().count() <= 80);
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
