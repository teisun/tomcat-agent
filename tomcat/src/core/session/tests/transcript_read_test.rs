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
fn read_entries_tail_before_header_only_returns_empty_page() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("header_only_before.jsonl");
    write_header(&path, &make_header("h2")).unwrap();
    let page = read_entries_tail_before(&path, 10, None).unwrap();
    assert!(page.entries.is_empty());
    assert!(!page.has_more);
    assert_eq!(page.next_cursor_offset, None);
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
fn tail_reader_before_cost_scales_with_page_size_not_file_size() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bounded_before.jsonl");
    write_header(&path, &make_header("sid_bounded_before")).unwrap();
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
    let (_page_10, stats_10) = read_entries_tail_before_with_stats(&path, 10, None).unwrap();
    let (_page_100, stats_100) = read_entries_tail_before_with_stats(&path, 100, None).unwrap();
    assert!(stats_10.bytes_scanned > 0);
    assert!(stats_100.bytes_scanned >= stats_10.bytes_scanned);
    assert!(
        stats_100.bytes_scanned < stats_10.bytes_scanned * 20,
        "expected sublinear constant-factor growth: stats_10={} stats_100={}",
        stats_10.bytes_scanned,
        stats_100.bytes_scanned
    );
    assert!(
        stats_100.bytes_scanned < file_len / 2,
        "before reader should not scan half the file for one page: scanned={} file_len={}",
        stats_100.bytes_scanned,
        file_len
    );
}

#[test]
fn tail_reader_before_pages_without_overlap_until_head() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("paged_before.jsonl");
    write_header(&path, &make_header("sid_paged")).unwrap();
    for idx in 1..=10 {
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

    let page1 = read_entries_tail_before(&path, 3, None).unwrap();
    assert!(page1.has_more);
    assert_eq!(page1.entries.len(), 3);
    match &page1.entries[0] {
        TranscriptEntry::Message(me) => assert_eq!(me.id.as_deref(), Some("m_8")),
        _ => panic!("expected message entry"),
    }
    let page2 = read_entries_tail_before(&path, 3, page1.next_cursor_offset).unwrap();
    let page3 = read_entries_tail_before(&path, 3, page2.next_cursor_offset).unwrap();
    let page4 = read_entries_tail_before(&path, 3, page3.next_cursor_offset).unwrap();

    let ids = |entries: &[TranscriptEntry]| {
        entries
            .iter()
            .map(|entry| match entry {
                TranscriptEntry::Message(me) => me.id.clone().unwrap(),
                _ => panic!("expected message entry"),
            })
            .collect::<Vec<_>>()
    };

    assert_eq!(ids(&page2.entries), vec!["m_5", "m_6", "m_7"]);
    assert_eq!(ids(&page3.entries), vec!["m_2", "m_3", "m_4"]);
    assert_eq!(ids(&page4.entries), vec!["m_1"]);
    assert!(!page4.has_more);
    assert_eq!(page4.next_cursor_offset, None);
}

#[test]
fn tail_reader_before_discards_midline_fragment_for_stale_offset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("midline_before.jsonl");
    write_header(&path, &make_header("sid_midline")).unwrap();
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

    let offset = find_entry_line_offset(&path, "m_3")
        .unwrap()
        .expect("offset for m_3");
    let page = read_entries_tail_before(&path, 2, Some(offset + 5)).unwrap();
    let ids = page
        .entries
        .iter()
        .map(|entry| match entry {
            TranscriptEntry::Message(me) => me.id.clone().unwrap(),
            _ => panic!("expected message entry"),
        })
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["m_1", "m_2"]);
}

#[test]
fn tail_reader_before_next_cursor_points_to_oldest_entry_line_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("next_cursor_line_start.jsonl");
    write_header(&path, &make_header("sid_cursor")).unwrap();
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
    append_entry(
        &path,
        &make_msg(
            "m_3",
            "2025-01-01T00:00:03.000Z",
            "user",
            "tail".to_string(),
        ),
    )
    .unwrap();

    let page = read_entries_tail_before(&path, 2, None).unwrap();
    let cursor = page.next_cursor_offset.expect("next cursor");
    let entry = read_entry_at_offset(&path, cursor)
        .unwrap()
        .expect("entry at next cursor");
    match entry {
        TranscriptEntry::Message(me) => assert_eq!(me.id.as_deref(), Some("m_big")),
        _ => panic!("expected message entry"),
    }
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
