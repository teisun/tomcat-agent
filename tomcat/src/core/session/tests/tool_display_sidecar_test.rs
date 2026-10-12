use super::*;
use crate::core::tools::primitive::{DiffTag, FileDiffLine};

fn file_display(text: &str) -> ToolDisplay {
    ToolDisplay::File {
        file: "src/lib.rs".to_string(),
        added: Some(1),
        removed: Some(0),
        diff: Some(vec![FileDiffLine {
            tag: DiffTag::Add,
            old_line: None,
            new_line: Some(1),
            skipped_lines: None,
            text: text.to_string(),
        }]),
        diff_truncated: false,
        expired: false,
    }
}

#[test]
fn compaction_expires_only_old_diffs_and_is_idempotent() {
    let temp = tempfile::TempDir::new().unwrap();
    let transcript = temp.path().join("session.jsonl");
    std::fs::write(&transcript, "").unwrap();
    let now = Utc::now();
    append_tool_display(
        &transcript,
        "old",
        &(now - chrono::Duration::days(8)).to_rfc3339(),
        &file_display("old change"),
    )
    .unwrap();
    append_tool_display(
        &transcript,
        "fresh",
        &(now - chrono::Duration::days(6)).to_rfc3339(),
        &file_display("fresh change"),
    )
    .unwrap();

    assert_eq!(
        compact_tool_display_sidecar(&transcript, now)
            .unwrap()
            .entries_compacted,
        1
    );
    let sidecar = tool_display_sidecar_path(&transcript);
    let rows: Vec<ToolDisplaySidecarEntry> = BufReader::new(std::fs::File::open(&sidecar).unwrap())
        .lines()
        .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
        .collect();
    let ToolDisplay::File {
        diff: old_diff,
        expired: old_expired,
        ..
    } = &rows[0].display
    else {
        panic!("expected file display");
    };
    assert!(old_expired);
    assert!(old_diff.is_none());
    let ToolDisplay::File {
        diff: fresh_diff,
        expired: fresh_expired,
        ..
    } = &rows[1].display
    else {
        panic!("expected file display");
    };
    assert!(!fresh_expired);
    assert_eq!(fresh_diff.as_ref().map(Vec::len), Some(1));

    let once = std::fs::read(&sidecar).unwrap();
    let modified_before_noop = std::fs::metadata(&sidecar).unwrap().modified().unwrap();
    assert_eq!(
        compact_tool_display_sidecar(&transcript, now)
            .unwrap()
            .entries_compacted,
        0
    );
    assert_eq!(std::fs::read(&sidecar).unwrap(), once);
    assert_eq!(
        std::fs::metadata(&sidecar).unwrap().modified().unwrap(),
        modified_before_noop,
        "a no-op compaction must not atomically replace the sidecar"
    );
}

#[test]
fn reverse_read_stops_before_rows_older_than_requested_page() {
    let temp = tempfile::TempDir::new().unwrap();
    let transcript = temp.path().join("session.jsonl");
    std::fs::write(&transcript, "").unwrap();
    let old = "2026-09-04T00:00:00Z";
    let page = "2026-09-04T00:01:00Z";
    append_tool_display(&transcript, "old-sentinel", old, &file_display("old")).unwrap();
    append_tool_display(&transcript, "shown", page, &file_display("shown")).unwrap();
    let wanted = HashSet::from(["shown".to_string(), "no-display".to_string()]);
    let oldest_page_message_at = DateTime::parse_from_rfc3339(page)
        .unwrap()
        .with_timezone(&Utc);
    let mut scanned = Vec::new();
    let displays = read_tool_displays_for_calls_with_observer(
        &transcript,
        &wanted,
        Some(oldest_page_message_at),
        |entry| scanned.push(entry.tool_call_id.clone()),
    )
    .unwrap();
    assert_eq!(displays.len(), 1);
    assert!(displays.contains_key("shown"));
    assert_eq!(scanned, vec!["shown"]);
    assert!(
        !scanned.iter().any(|id| id == "old-sentinel"),
        "the old sidecar row must not be read after crossing the page boundary"
    );
}
