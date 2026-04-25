//! # `read_entries_tail` 读取尾部条目
//!
//! 覆盖 transcript 顺序读路径：
//!
//! - `append_and_read_entries_tail`：写一条 message，tail 读出 1 条。
//! - `read_entries_tail_skips_unknown_type_line` (E2E-CLI-093)：未知 `type`
//!   字符串不会让 tail 读取 panic，前向兼容性保证只跳过该行。
//! - `read_entries_tail_header_only_returns_empty`：仅 header 时返回空 `Vec`。

use super::super::transcript::*;

fn temp_transcript_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("pi_wasm_transcript_test")
}

#[test]
fn append_and_read_entries_tail() {
    let dir = temp_transcript_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s2.jsonl");
    let header = SessionHeader {
        r#type: "session".to_string(),
        version: Some(3),
        id: "sid_002".to_string(),
        timestamp: "2025-01-01T00:00:00.000Z".to_string(),
        cwd: None,
    };
    write_header(&path, &header).unwrap();
    let msg = TranscriptEntry::Message(MessageEntry {
        id: Some("e1".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:01.000Z".to_string(),
        message: serde_json::json!({"role":"user","content":"hello"}),
    });
    append_entry(&path, &msg).unwrap();
    let entries = read_entries_tail(&path, 10).unwrap();
    assert_eq!(entries.len(), 1);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

/// E2E-CLI-093：`type` 非 `TranscriptEntry` 成员时 tail 读入跳过该行、不 panic。
#[test]
fn read_entries_tail_skips_unknown_type_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skip_unknown.jsonl");
    let header = SessionHeader {
        r#type: "session".to_string(),
        version: Some(3),
        id: "sid_skip".to_string(),
        timestamp: "2025-01-01T00:00:00.000Z".to_string(),
        cwd: None,
    };
    write_header(&path, &header).unwrap();
    let msg = TranscriptEntry::Message(MessageEntry {
        id: Some("m_ok".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:01.000Z".to_string(),
        message: serde_json::json!({"role":"user","content":"keep"}),
    });
    append_entry(&path, &msg).unwrap();
    append_line(
        &path,
        r#"{"type":"totally_unknown_variant","timestamp":"2025-01-01T00:00:02.000Z","id":"bad"}"#,
    )
    .unwrap();
    let entries = read_entries_tail(&path, 10).unwrap();
    assert_eq!(entries.len(), 1);
    match &entries[0] {
        TranscriptEntry::Message(me) => assert_eq!(me.id.as_deref(), Some("m_ok")),
        _ => panic!("expected sole surviving message"),
    }
}

#[test]
fn read_entries_tail_header_only_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("header_only.jsonl");
    write_header(
        &path,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "h1".to_string(),
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();
    let entries = read_entries_tail(&path, 10).unwrap();
    assert!(entries.is_empty());
}
