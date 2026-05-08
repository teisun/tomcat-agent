//! # `SessionHeader` 读写
//!
//! 验证 transcript JSONL 文件的首行 header：
//!
//! - 写入后能从同一路径读出原值（`id` / `version` 字段一致）。
//! - `read_header` 在文件不存在 / 空文件两种异常输入下返回 `Err`，
//!   不会 panic 也不会把垃圾数据当成合法 header。

use super::super::transcript::*;

#[test]
fn write_header_and_read_header() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.jsonl");
    let header = SessionHeader {
        r#type: "session".to_string(),
        version: Some(3),
        id: "sid_001".to_string(),
        timestamp: "2025-01-01T00:00:00.000Z".to_string(),
        cwd: Some("/tmp".to_string()),
    };
    write_header(&path, &header).unwrap();
    let read = read_header(&path).unwrap();
    assert_eq!(read.id, "sid_001");
    assert_eq!(read.version, Some(3));
}

#[test]
fn read_header_fails_on_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nonexistent.jsonl");
    let r = read_header(&path);
    assert!(r.is_err());
}

#[test]
fn read_header_fails_on_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.jsonl");
    std::fs::write(&path, "").unwrap();
    let r = read_header(&path);
    assert!(r.is_err());
}
