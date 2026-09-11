use super::*;

fn result(marker_id: &str, text: &str) -> CompactionResult {
    CompactionResult {
        summary_text: text.to_string(),
        covered_start_id: "u1".to_string(),
        covered_end_id: "a1".to_string(),
        covered_count: 2,
        transcript_compaction_entry_id: Some(marker_id.to_string()),
        estimated_covered_tokens_before: Some(100),
        estimated_summary_tokens: Some(10),
        estimated_tokens_saved: Some(90),
        preheat_elapsed_ms: 140_000,
    }
}

#[test]
fn cache_is_atomically_replaced_and_round_trips_one_pending_result() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("session.jsonl");

    write_preheat_cache(&transcript, &result("marker-1", "first")).unwrap();
    write_preheat_cache(&transcript, &result("marker-2", "second")).unwrap();

    assert_eq!(
        read_preheat_cache(&transcript).unwrap().summary_text,
        "second",
        "a new preheat replaces stale pending work instead of growing an event log"
    );
    let raw = std::fs::read_to_string(preheat_cache_path(&transcript)).unwrap();
    assert_eq!(raw.lines().count(), 1);
    assert!(raw.contains("marker-2"));
    assert!(!raw.contains("marker-1"));
}

#[test]
fn invalid_or_missing_marker_id_is_not_cached() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("session.jsonl");
    let mut invalid = result("marker-1", "summary");
    invalid.transcript_compaction_entry_id = None;

    assert!(write_preheat_cache(&transcript, &invalid).is_err());
    assert!(read_preheat_cache(&transcript).is_none());
}
