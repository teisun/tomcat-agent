//! `read_entries_tail` / ordinal slice reader 的底层行为。

use super::super::transcript::*;

fn make_header(id: &str) -> SessionHeader {
    SessionHeader {
        r#type: "session".to_string(),
        version: Some(3),
        id: id.to_string(),
        timestamp: "2025-01-01T00:00:00.000Z".to_string(),
        cwd: None,
    }
}

fn make_msg(id: &str, ts: &str, role: &str, content: String) -> TranscriptEntry {
    TranscriptEntry::Message(MessageEntry {
        id: Some(id.to_string()),
        parent_id: None,
        timestamp: ts.to_string(),
        message: serde_json::json!({
            "role": role,
            "content": content,
        }),
    })
}

#[test]
fn append_and_read_entries_tail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s2.jsonl");
    write_header(&path, &make_header("sid_002")).unwrap();
    append_entry(
        &path,
        &make_msg(
            "e1",
            "2025-01-01T00:00:01.000Z",
            "user",
            "hello".to_string(),
        ),
    )
    .unwrap();
    let entries = read_entries_tail(&path, 10).unwrap();
    assert_eq!(entries.len(), 1);
}

#[test]
fn tail_reader_returns_exact_cap_from_large_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.jsonl");
    write_header(&path, &make_header("sid_large")).unwrap();
    for idx in 1..=10_000 {
        append_entry(
            &path,
            &make_msg(
                &format!("m_{idx}"),
                "2025-01-01T00:00:01.000Z",
                "user",
                format!("msg-{idx}"),
            ),
        )
        .unwrap();
    }

    let entries = read_entries_tail(&path, 50).unwrap();
    assert_eq!(entries.len(), 50);
    match &entries[0] {
        TranscriptEntry::Message(me) => assert_eq!(me.id.as_deref(), Some("m_9951")),
        _ => panic!("expected message entry"),
    }
    match entries.last().unwrap() {
        TranscriptEntry::Message(me) => assert_eq!(me.id.as_deref(), Some("m_10000")),
        _ => panic!("expected message entry"),
    }
}

#[test]
fn tail_reader_smaller_than_cap_returns_all() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("small.jsonl");
    write_header(&path, &make_header("sid_small")).unwrap();
    for idx in 1..=3 {
        append_entry(
            &path,
            &make_msg(
                &format!("m_{idx}"),
                "2025-01-01T00:00:01.000Z",
                "user",
                format!("msg-{idx}"),
            ),
        )
        .unwrap();
    }
    let entries = read_entries_tail(&path, 50).unwrap();
    assert_eq!(entries.len(), 3);
}

#[test]
fn tail_reader_skips_header_blank_and_corrupt_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skip_unknown.jsonl");
    write_header(&path, &make_header("sid_skip")).unwrap();
    append_entry(
        &path,
        &make_msg(
            "m_ok",
            "2025-01-01T00:00:01.000Z",
            "user",
            "keep".to_string(),
        ),
    )
    .unwrap();
    append_line(&path, "").unwrap();
    append_line(
        &path,
        r#"{"type":"totally_unknown_variant","timestamp":"2025-01-01T00:00:02.000Z","id":"bad"}"#,
    )
    .unwrap();
    append_entry(
        &path,
        &make_msg(
            "m_ok_2",
            "2025-01-01T00:00:03.000Z",
            "assistant",
            "keep2".to_string(),
        ),
    )
    .unwrap();

    let entries = read_entries_tail(&path, 10).unwrap();
    assert_eq!(entries.len(), 2);
    match &entries[0] {
        TranscriptEntry::Message(me) => assert_eq!(me.id.as_deref(), Some("m_ok")),
        _ => panic!("expected message entry"),
    }
    match &entries[1] {
        TranscriptEntry::Message(me) => assert_eq!(me.id.as_deref(), Some("m_ok_2")),
        _ => panic!("expected message entry"),
    }
}

#[test]
fn read_entries_tail_header_only_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("header_only.jsonl");
    write_header(&path, &make_header("h1")).unwrap();
    let entries = read_entries_tail(&path, 10).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn tail_reader_scans_bounded_bytes_not_whole_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bounded_tail.jsonl");
    write_header(&path, &make_header("sid_bounded")).unwrap();
    for idx in 0..20_000 {
        append_entry(
            &path,
            &make_msg(
                &format!("m_{idx}"),
                "2025-01-01T00:00:01.000Z",
                "user",
                format!("payload-{idx:05}"),
            ),
        )
        .unwrap();
    }
    let file_len = std::fs::metadata(&path).unwrap().len();
    let (_entries, stats) = read_entries_tail_with_stats(&path, 50).unwrap();
    assert!(
        stats.bytes_scanned < file_len / 4,
        "tail reader should not scan the whole file: scanned={} file_len={}",
        stats.bytes_scanned,
        file_len
    );
}

#[test]
fn tail_reader_handles_cross_chunk_line_splice() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cross_chunk.jsonl");
    write_header(&path, &make_header("sid_cross")).unwrap();
    append_entry(
        &path,
        &make_msg(
            "m_1",
            "2025-01-01T00:00:01.000Z",
            "user",
            "short".to_string(),
        ),
    )
    .unwrap();
    append_entry(
        &path,
        &make_msg(
            "m_big",
            "2025-01-01T00:00:02.000Z",
            "assistant",
            "x".repeat(80_000),
        ),
    )
    .unwrap();
    let entries = read_entries_tail(&path, 2).unwrap();
    assert_eq!(entries.len(), 2);
    match &entries[1] {
        TranscriptEntry::Message(me) => assert_eq!(me.id.as_deref(), Some("m_big")),
        _ => panic!("expected message entry"),
    }
}

#[test]
fn slice_reader_by_ordinal_range_returns_expected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("range.jsonl");
    write_header(&path, &make_header("sid_range")).unwrap();
    for idx in 0..10 {
        append_entry(
            &path,
            &make_msg(
                &format!("m_{idx}"),
                "2025-01-01T00:00:01.000Z",
                "user",
                format!("msg-{idx}"),
            ),
        )
        .unwrap();
    }
    let (entries, stats) = read_entries_range_by_ordinal_with_stats(&path, 3, 7).unwrap();
    assert_eq!(entries.len(), 4);
    assert!(stats.bytes_scanned > 0);
    match &entries[0] {
        TranscriptEntry::Message(me) => assert_eq!(me.id.as_deref(), Some("m_3")),
        _ => panic!("expected message entry"),
    }
    match &entries[3] {
        TranscriptEntry::Message(me) => assert_eq!(me.id.as_deref(), Some("m_6")),
        _ => panic!("expected message entry"),
    }
}
