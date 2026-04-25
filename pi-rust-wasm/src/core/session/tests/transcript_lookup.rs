//! # 单条 / 子节点 / 分支查询
//!
//! 覆盖只读查询 API 的常见与异常输入：
//!
//! - `get_entry_finds_by_id`：通过 id 命中并返回原始 variant；不存在返回 `None`。
//! - `get_leaf_entry_returns_last`：返回写入的最后一条 entry。
//! - `get_branch_single_entry` / `get_branch_unknown_leaf_returns_empty`：
//!   单节点链路与 leaf 不存在场景。
//! - `get_children_empty_when_no_match`：parent id 不存在时返回空 `Vec`。

use super::super::transcript::*;

#[test]
fn get_entry_finds_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s3.jsonl");
    write_header(
        &path,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "sid_003".to_string(),
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();
    let e1 = TranscriptEntry::Message(MessageEntry {
        id: Some("ent-1".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:01.000Z".to_string(),
        message: serde_json::json!({"role":"user"}),
    });
    let e2 = TranscriptEntry::Message(MessageEntry {
        id: Some("ent-2".to_string()),
        parent_id: Some("ent-1".to_string()),
        timestamp: "2025-01-01T00:00:02.000Z".to_string(),
        message: serde_json::json!({"role":"assistant"}),
    });
    append_entry(&path, &e1).unwrap();
    append_entry(&path, &e2).unwrap();
    let found = get_entry(&path, "ent-2").unwrap().unwrap();
    assert!(matches!(found, TranscriptEntry::Message(_)));
    let none = get_entry(&path, "nonexistent").unwrap();
    assert!(none.is_none());
}

#[test]
fn get_leaf_entry_returns_last() {
    let dir = std::env::temp_dir().join("pi_wasm_transcript_get_leaf");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s4.jsonl");
    write_header(
        &path,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "sid_004".to_string(),
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();
    append_entry(
        &path,
        &TranscriptEntry::Message(MessageEntry {
            id: Some("last".to_string()),
            parent_id: None,
            timestamp: "2025-01-01T00:00:01.000Z".to_string(),
            message: serde_json::json!({"role":"user"}),
        }),
    )
    .unwrap();
    let leaf = get_leaf_entry(&path).unwrap().unwrap();
    assert!(matches!(leaf, TranscriptEntry::Message(_)));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn get_branch_single_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("branch.jsonl");
    write_header(
        &path,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "sid".to_string(),
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();
    let e = TranscriptEntry::Message(MessageEntry {
        id: Some("e1".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:01.000Z".to_string(),
        message: serde_json::json!({"role":"user"}),
    });
    append_entry(&path, &e).unwrap();
    let branch = get_branch(&path, "e1", 100).unwrap();
    assert_eq!(branch.len(), 1);
}

#[test]
fn get_branch_unknown_leaf_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("branch2.jsonl");
    write_header(
        &path,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "sid".to_string(),
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();
    let branch = get_branch(&path, "nonexistent", 100).unwrap();
    assert!(branch.is_empty());
}

#[test]
fn get_children_empty_when_no_match() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("children.jsonl");
    write_header(
        &path,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "sid".to_string(),
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();
    let children = get_children(&path, "no_such_parent", 10).unwrap();
    assert!(children.is_empty());
}
