use super::super::manager::{PlanEventKind, SessionManager};
use super::super::resume_index::{
    load_or_rebuild_resume_index, rebuild_resume_index, resume_index_path,
    take_last_inline_rebuild_stats_for_tests, ResumeIndexSource,
};
use super::super::transcript::{
    append_line, insert_entry_after_message_id, mark_message_entries_after_anchor_superseded,
    rewrite_message_text_entries_by_id, set_branch_summary_entry_is_boundary_true, write_header,
    BranchSummaryEntry, MessageTextRewrite, SessionHeader, TranscriptEntry,
};

fn setup_mgr() -> (tempfile::TempDir, SessionManager) {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let key = mgr.current_session_key().to_string();
    mgr.create_session(&key, None).unwrap();
    (dir, mgr)
}

fn append_json_line(path: &std::path::Path, value: serde_json::Value) {
    append_line(path, &value.to_string()).unwrap();
}

#[test]
fn sidecar_incremental_append_tracks_last_id_and_turn_anchors() {
    let (_dir, mgr) = setup_mgr();
    let mut user_ids = Vec::new();
    let mut last_entry_id = None;
    for idx in 0..12 {
        let user_id = mgr
            .append_message(serde_json::json!({"role":"user","content": format!("q-{idx}")}))
            .unwrap();
        user_ids.push(user_id);
        last_entry_id = Some(
            mgr.append_message(
                serde_json::json!({"role":"assistant","content": format!("a-{idx}")}),
            )
            .unwrap(),
        );
    }

    let transcript_path = mgr.current_transcript_path().unwrap().unwrap();
    let load = load_or_rebuild_resume_index(&transcript_path).unwrap();
    assert_eq!(load.index.total_entries, 24);
    assert_eq!(load.index.last_entry_id, last_entry_id);
    assert_eq!(load.index.recent_turn_starts.len(), 12);
    assert_eq!(
        load.index
            .recent_turn_starts
            .first()
            .and_then(|anchor| anchor.entry_id.as_deref()),
        Some(user_ids[0].as_str())
    );
    assert_eq!(
        load.index
            .recent_turn_starts
            .last()
            .and_then(|anchor| anchor.entry_id.as_deref()),
        Some(user_ids[11].as_str())
    );
}

#[test]
fn sidecar_records_latest_boundary_and_plan_event() {
    let (_dir, mgr) = setup_mgr();
    let transcript_path = mgr.current_transcript_path().unwrap().unwrap();
    mgr.append_message(serde_json::json!({"role":"user","content":"q1"}))
        .unwrap();
    let boundary = TranscriptEntry::BranchSummary(BranchSummaryEntry {
        id: Some("boundary_1".to_string()),
        parent_id: None,
        timestamp: "2025-01-02T00:00:00.000Z".to_string(),
        summary: Some("summary".to_string()),
        covered_start_id: None,
        covered_end_id: None,
        covered_count: Some(1),
        is_boundary: Some(true),
        preheat_compaction_id: None,
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        error: None,
        attempts: None,
    });
    super::super::transcript::append_entry(&transcript_path, &boundary).unwrap();
    mgr.append_custom_entry(serde_json::json!({
        "event": crate::infra::wire::WIRE_PLAN_BUILD,
        "plan_id": "plan_latest",
        "path": transcript_path.with_extension("plan.md").to_string_lossy(),
        "state": "executing",
    }))
    .unwrap();

    let load = load_or_rebuild_resume_index(&transcript_path).unwrap();
    let boundary = load.index.latest_boundary.clone().expect("latest boundary");
    assert_eq!(boundary.entry_id.as_deref(), Some("boundary_1"));
    let plan = load.index.latest_plan_event_ref().expect("latest plan");
    assert_eq!(plan.kind, PlanEventKind::Build);
    assert_eq!(plan.plan_id, "plan_latest");
}

#[test]
fn sidecar_missing_rebuilds_equivalent_to_full_scan() {
    let (_dir, mgr) = setup_mgr();
    for idx in 0..5 {
        mgr.append_message(serde_json::json!({"role":"user","content": format!("q-{idx}")}))
            .unwrap();
        mgr.append_message(serde_json::json!({"role":"assistant","content": format!("a-{idx}")}))
            .unwrap();
    }
    let transcript_path = mgr.current_transcript_path().unwrap().unwrap();
    let before = load_or_rebuild_resume_index(&transcript_path)
        .unwrap()
        .index;
    std::fs::remove_file(resume_index_path(&transcript_path)).unwrap();

    let rebuilt = load_or_rebuild_resume_index(&transcript_path).unwrap();
    assert_eq!(rebuilt.source, ResumeIndexSource::Rebuilt);
    assert_eq!(rebuilt.index.total_entries, before.total_entries);
    assert_eq!(rebuilt.index.last_entry_id, before.last_entry_id);
    assert_eq!(
        rebuilt.index.recent_turn_starts.len(),
        before.recent_turn_starts.len()
    );
}

#[test]
fn sidecar_schema_version_mismatch_falls_back_and_rebuilds() {
    let (_dir, mgr) = setup_mgr();
    mgr.append_message(serde_json::json!({"role":"user","content":"q1"}))
        .unwrap();
    let transcript_path = mgr.current_transcript_path().unwrap().unwrap();
    let sidecar_path = resume_index_path(&transcript_path);
    let _ = load_or_rebuild_resume_index(&transcript_path).unwrap();

    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sidecar_path).unwrap()).unwrap();
    json["schema_version"] = serde_json::json!(999);
    std::fs::write(&sidecar_path, serde_json::to_vec_pretty(&json).unwrap()).unwrap();

    let rebuilt = load_or_rebuild_resume_index(&transcript_path).unwrap();
    assert_eq!(rebuilt.source, ResumeIndexSource::Rebuilt);
    assert_eq!(rebuilt.index.schema_version, 1);
}

#[test]
fn sidecar_fingerprint_mismatch_on_size_or_last_id_rebuilds() {
    let (_dir, mgr) = setup_mgr();
    mgr.append_message(serde_json::json!({"role":"user","content":"q1"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"a1"}))
        .unwrap();
    let transcript_path = mgr.current_transcript_path().unwrap().unwrap();
    let sidecar_path = resume_index_path(&transcript_path);
    let _ = load_or_rebuild_resume_index(&transcript_path).unwrap();

    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sidecar_path).unwrap()).unwrap();
    json["transcript_size"] = serde_json::json!(1);
    json["last_entry_id"] = serde_json::json!("stale-id");
    std::fs::write(&sidecar_path, serde_json::to_vec_pretty(&json).unwrap()).unwrap();

    let rebuilt = load_or_rebuild_resume_index(&transcript_path).unwrap();
    assert_eq!(rebuilt.source, ResumeIndexSource::Rebuilt);
    assert_ne!(rebuilt.index.last_entry_id.as_deref(), Some("stale-id"));
}

#[test]
fn sidecar_incremental_append_rebuilds_when_cache_skips_out_of_band_entry() {
    let (_dir, mgr) = setup_mgr();
    let first_id = mgr
        .append_message(serde_json::json!({"role":"user","content":"first"}))
        .unwrap();
    let transcript_path = mgr.current_transcript_path().unwrap().unwrap();
    let _ = load_or_rebuild_resume_index(&transcript_path).unwrap();

    append_json_line(
        &transcript_path,
        serde_json::json!({
            "type": "message",
            "id": "external_second",
            "timestamp": "2025-01-02T00:00:00.000Z",
            "message": { "role": "user", "content": "second" }
        }),
    );

    let third_id = mgr
        .append_message(serde_json::json!({"role":"assistant","content":"third"}))
        .unwrap();

    let rebuilt = load_or_rebuild_resume_index(&transcript_path).unwrap();
    assert_eq!(rebuilt.index.total_entries, 3);
    assert_eq!(rebuilt.index.last_entry_id.as_deref(), Some(third_id.as_str()));
    assert_eq!(rebuilt.index.recent_turn_starts.len(), 2);
    assert_eq!(
        rebuilt.index.recent_turn_starts[0].entry_id.as_deref(),
        Some(first_id.as_str())
    );
    assert_eq!(
        rebuilt.index.recent_turn_starts[1].entry_id.as_deref(),
        Some("external_second")
    );
}

#[test]
fn sidecar_inline_rebuilt_after_rewrite_stays_valid() {
    let (_dir, mgr) = setup_mgr();
    let anchor_id = mgr
        .append_message(serde_json::json!({"role":"user","content":"anchor"}))
        .unwrap();
    let later_id = mgr
        .append_message(serde_json::json!({"role":"assistant","content":"later"}))
        .unwrap();
    let transcript_path = mgr.current_transcript_path().unwrap().unwrap();

    let inserted = TranscriptEntry::BranchSummary(BranchSummaryEntry {
        id: Some("cmp_1".to_string()),
        parent_id: None,
        timestamp: "2025-01-03T00:00:00.000Z".to_string(),
        summary: Some("summary".to_string()),
        covered_start_id: None,
        covered_end_id: None,
        covered_count: Some(1),
        is_boundary: Some(false),
        preheat_compaction_id: None,
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        error: None,
        attempts: None,
    });
    insert_entry_after_message_id(&transcript_path, &anchor_id, &inserted).unwrap();
    set_branch_summary_entry_is_boundary_true(&transcript_path, "cmp_1").unwrap();
    let changed = rewrite_message_text_entries_by_id(
        &transcript_path,
        &[MessageTextRewrite {
            message_id: later_id.clone(),
            new_content: "rewritten".to_string(),
        }],
    )
    .unwrap();
    assert_eq!(changed, 1);
    mark_message_entries_after_anchor_superseded(&transcript_path, &anchor_id).unwrap();
    let inline_stats = take_last_inline_rebuild_stats_for_tests()
        .expect("rewrite path should record inline rebuild stats");
    assert_eq!(
        inline_stats.bytes_scanned, 0,
        "inline rebuild should reuse in-memory lines instead of rereading transcript"
    );

    let load = load_or_rebuild_resume_index(&transcript_path).unwrap();
    assert_eq!(load.source, ResumeIndexSource::Existing);
    assert!(load.index.latest_boundary.is_some());
    assert_eq!(load.index.total_entries, 3);
}

#[test]
fn cold_rebuild_streams_without_loading_whole_file() {
    let (_dir, mgr) = setup_mgr();
    let transcript_path = mgr.current_transcript_path().unwrap().unwrap();
    write_header(
        &transcript_path,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "sid_resume_rebuild".to_string(),
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();
    for idx in 0..5_000 {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("u_{idx}"),
                "timestamp": "2025-01-01T00:00:01.000Z",
                "message": { "role": "user", "content": format!("q-{idx}") }
            }),
        );
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("a_{idx}"),
                "timestamp": "2025-01-01T00:00:02.000Z",
                "message": { "role": "assistant", "content": format!("a-{idx}") }
            }),
        );
    }
    let sidecar_path = resume_index_path(&transcript_path);
    let _ = load_or_rebuild_resume_index(&transcript_path).unwrap();
    std::fs::remove_file(&sidecar_path).unwrap();

    let file_len = std::fs::metadata(&transcript_path).unwrap().len() as usize;
    let (rebuilt, stats) = rebuild_resume_index(&transcript_path).unwrap();
    assert_eq!(rebuilt.total_entries, 10_000);
    assert!(
        stats.max_live_bytes < file_len / 4,
        "forward rebuild should not buffer the whole file: max_live_bytes={} file_len={}",
        stats.max_live_bytes,
        file_len
    );
}
