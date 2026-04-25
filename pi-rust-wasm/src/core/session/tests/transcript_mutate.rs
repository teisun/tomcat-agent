//! # 原地改写 / 插入 / 删除条目
//!
//! 覆盖三组写路径：
//!
//! - `set_branch_summary_entry_is_boundary_true_*`：把 `BranchSummary` 行的
//!   `is_boundary` 字段原地翻转，保留其它字段；非目标行保留前导空白
//!   （`preserve_unmatched_line_whitespace`）。
//! - `insert_entry_after_message_id_*`：在指定锚点 message 之后插入新条目，
//!   并保持原有更晚消息的相对顺序。
//! - `remove_branch_summary_entry_by_id_*`：按 id 删除 BranchSummary 行；
//!   重复 id 全部删除；id 不存在时返回 `Err`。

use super::super::transcript::*;

#[test]
fn set_branch_summary_entry_is_boundary_true_updates_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("inplace.jsonl");
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
    let c = TranscriptEntry::BranchSummary(BranchSummaryEntry {
        id: Some("cmp_inplace".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:01.000Z".to_string(),
        summary: Some("s".to_string()),
        covered_start_id: Some("a".to_string()),
        covered_end_id: Some("b".to_string()),
        covered_count: Some(2),
        is_boundary: Some(false),
        preheat_compaction_id: Some("cmp_inplace".to_string()),
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
    });
    append_entry(&path, &c).unwrap();
    set_branch_summary_entry_is_boundary_true(&path, "cmp_inplace").unwrap();
    let e = get_entry(&path, "cmp_inplace").unwrap().unwrap();
    match e {
        TranscriptEntry::BranchSummary(ce) => {
            assert_eq!(ce.is_boundary, Some(true));
            assert_eq!(ce.summary.as_deref(), Some("s"));
        }
        _ => panic!("expected branch_summary"),
    }
}

#[test]
fn set_branch_summary_entry_is_boundary_true_preserves_unmatched_line_whitespace() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preserve_ws.jsonl");
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

    let msg = TranscriptEntry::Message(MessageEntry {
        id: Some("msg_ws".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:01.000Z".to_string(),
        message: serde_json::json!({"role": "user", "content": "hi"}),
    });
    let indented = format!("   {}", serde_json::to_string(&msg).unwrap());
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(f, "{}", indented).unwrap();

    let c = TranscriptEntry::BranchSummary(BranchSummaryEntry {
        id: Some("cmp_ws".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:02.000Z".to_string(),
        summary: Some("sum".to_string()),
        covered_start_id: Some("a".to_string()),
        covered_end_id: Some("b".to_string()),
        covered_count: Some(2),
        is_boundary: Some(false),
        preheat_compaction_id: None,
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
    });
    append_entry(&path, &c).unwrap();

    set_branch_summary_entry_is_boundary_true(&path, "cmp_ws").unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(
        raw.lines()
            .any(|l| l.starts_with("   ") && l.contains("\"type\":\"message\"")),
        "leading spaces on non-target lines must be preserved:\n{raw}"
    );

    let e = get_entry(&path, "cmp_ws").unwrap().unwrap();
    match e {
        TranscriptEntry::BranchSummary(ce) => assert_eq!(ce.is_boundary, Some(true)),
        _ => panic!("expected branch_summary"),
    }
}

#[test]
fn insert_entry_after_message_id_inserts_before_later_messages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("insert_anchor.jsonl");
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

    let m_anchor = TranscriptEntry::Message(MessageEntry {
        id: Some("mid_anchor".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:01.000Z".to_string(),
        message: serde_json::json!({"role": "user", "content": "u"}),
    });
    append_entry(&path, &m_anchor).unwrap();

    let m_later = TranscriptEntry::Message(MessageEntry {
        id: Some("mid_later".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:02.000Z".to_string(),
        message: serde_json::json!({"role": "assistant", "content": "a"}),
    });
    append_entry(&path, &m_later).unwrap();

    let c = TranscriptEntry::BranchSummary(BranchSummaryEntry {
        id: Some("S::E".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:03.000Z".to_string(),
        summary: Some("sum".to_string()),
        covered_start_id: Some("S".to_string()),
        covered_end_id: Some("mid_anchor".to_string()),
        covered_count: Some(1),
        is_boundary: Some(false),
        preheat_compaction_id: None,
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
    });
    insert_entry_after_message_id(&path, "mid_anchor", &c).unwrap();

    let entries = read_entries_tail(&path, 10).unwrap();
    assert_eq!(entries.len(), 3);
    assert!(matches!(&entries[0], TranscriptEntry::Message(_)));
    assert!(matches!(&entries[1], TranscriptEntry::BranchSummary(_)));
    assert!(matches!(&entries[2], TranscriptEntry::Message(_)));
}

#[test]
fn remove_branch_summary_entry_by_id_removes_matching_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rm_branch.jsonl");
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
    let msg = TranscriptEntry::Message(MessageEntry {
        id: Some("m1".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:01.000Z".to_string(),
        message: serde_json::json!({"role": "user", "content": "hi"}),
    });
    append_entry(&path, &msg).unwrap();
    let b = TranscriptEntry::BranchSummary(BranchSummaryEntry {
        id: Some("rm_target".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:02.000Z".to_string(),
        summary: Some("s".to_string()),
        covered_start_id: Some("a".to_string()),
        covered_end_id: Some("b".to_string()),
        covered_count: Some(2),
        is_boundary: Some(false),
        preheat_compaction_id: None,
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
    });
    append_entry(&path, &b).unwrap();
    remove_branch_summary_entry_by_id(&path, "rm_target").unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(raw.lines().count(), 2, "header + message only");
    assert!(get_entry(&path, "rm_target").unwrap().is_none());
}

#[test]
fn remove_branch_summary_entry_by_id_removes_all_duplicate_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rm_dup.jsonl");
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
    let mk = |ts: &str| {
        TranscriptEntry::BranchSummary(BranchSummaryEntry {
            id: Some("dup_id".to_string()),
            parent_id: None,
            timestamp: ts.to_string(),
            summary: Some("x".to_string()),
            covered_start_id: None,
            covered_end_id: None,
            covered_count: None,
            is_boundary: Some(false),
            preheat_compaction_id: None,
            estimated_covered_tokens_before: None,
            estimated_summary_tokens: None,
            estimated_tokens_saved: None,
        })
    };
    append_entry(&path, &mk("2025-01-01T00:00:01.000Z")).unwrap();
    append_entry(&path, &mk("2025-01-01T00:00:02.000Z")).unwrap();
    remove_branch_summary_entry_by_id(&path, "dup_id").unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(raw.lines().count(), 1);
}

#[test]
fn remove_branch_summary_entry_by_id_not_found_returns_err() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rm_missing.jsonl");
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
    let r = remove_branch_summary_entry_by_id(&path, "nope");
    assert!(r.is_err());
}
