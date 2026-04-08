use super::*;

fn temp_transcript_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("pi_wasm_transcript_test")
}

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

#[test]
fn set_compaction_entry_is_boundary_true_updates_line() {
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
    let c = TranscriptEntry::Compaction(CompactionEntry {
        id: Some("cmp_inplace".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:01.000Z".to_string(),
        summary: Some("s".to_string()),
        covered_start_id: Some("a".to_string()),
        covered_end_id: Some("b".to_string()),
        covered_count: Some(2),
        is_boundary: Some(false),
        preheat_compaction_id: Some("cmp_inplace".to_string()),
    });
    append_entry(&path, &c).unwrap();
    set_compaction_entry_is_boundary_true(&path, "cmp_inplace").unwrap();
    let e = get_entry(&path, "cmp_inplace").unwrap().unwrap();
    match e {
        TranscriptEntry::Compaction(ce) => {
            assert_eq!(ce.is_boundary, Some(true));
            assert_eq!(ce.summary.as_deref(), Some("s"));
        }
        _ => panic!("expected compaction"),
    }
}

#[test]
fn set_compaction_entry_is_boundary_true_preserves_unmatched_line_whitespace() {
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

    let c = TranscriptEntry::Compaction(CompactionEntry {
        id: Some("cmp_ws".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:02.000Z".to_string(),
        summary: Some("sum".to_string()),
        covered_start_id: Some("a".to_string()),
        covered_end_id: Some("b".to_string()),
        covered_count: Some(2),
        is_boundary: Some(false),
        preheat_compaction_id: None,
    });
    append_entry(&path, &c).unwrap();

    set_compaction_entry_is_boundary_true(&path, "cmp_ws").unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(
        raw.lines().any(|l| l.starts_with("   ") && l.contains("\"type\":\"message\"")),
        "leading spaces on non-compaction lines must be preserved:\n{raw}"
    );

    let e = get_entry(&path, "cmp_ws").unwrap().unwrap();
    match e {
        TranscriptEntry::Compaction(ce) => assert_eq!(ce.is_boundary, Some(true)),
        _ => panic!("expected compaction"),
    }
}
