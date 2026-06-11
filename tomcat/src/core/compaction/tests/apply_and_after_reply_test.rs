use super::super::apply::check_after_reply;
use super::super::layer0_persist_large_results;
use super::mocks::*;
use crate::core::compaction::preheat::Preheat;
use crate::core::llm::MessageKind;
use crate::core::session::manager::compound_turn_id;
use crate::core::session::transcript::{
    append_entry, read_header, write_header, BranchSummaryEntry, SessionHeader, TranscriptEntry,
};
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;
use crate::infra::{wire, DefaultEventBus, EventBus, EventContext};

// --- TASK-20 新增测试 ---

#[test]
fn abort_preheat_idle_is_noop() {
    let mut state = make_state(100, 1000, 250);
    assert!(state.preheat.is_idle());
    state.preheat.abort();
    assert!(state.preheat.is_idle());
}

#[test]
fn apply_boundary_replaces_covered_range() {
    let mut state = make_state(0, 100_000, 25_000);
    let m0 = user_msg_with_id("m0", &"a".repeat(5000));
    let m1 = user_msg_with_id("m1", &"b".repeat(3000));
    let m2 = user_msg_with_id("m2", &"c".repeat(2000));
    state.messages = vec![m0, m1, m2];
    state.estimate_context_chars = 10_000;

    let result = crate::core::session::manager::CompactionResult {
        summary_text: "short summary".into(),
        covered_start_id: "m0".into(),
        covered_end_id: "m1".into(),
        covered_count: 2,
        transcript_compaction_entry_id: Some(compound_turn_id("m0", "m1")),
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        preheat_elapsed_ms: 0,
    };
    let old_ratio = state.usage_ratio();
    state.apply_boundary(result).unwrap();

    assert_eq!(state.messages.len(), 2);
    assert_eq!(state.messages[0].kind, MessageKind::CompactionSummary);
    assert_eq!(state.messages[0].text_content(), Some("short summary"));
    assert_eq!(
        state.messages[0].msg_id.as_deref(),
        Some(compound_turn_id("m0", "m1").as_str())
    );
    assert_eq!(state.messages[1].msg_id.as_deref(), Some("m2"));
    assert!(state.last_api_usage.is_none());
    let new_ratio = state.usage_ratio();
    assert!(
        new_ratio < old_ratio,
        "ratio should decrease after boundary"
    );
}

#[test]
fn apply_boundary_not_found_returns_err() {
    let mut state = make_state(1000, 10_000, 2_500);
    state.messages = vec![user_msg("x")]; // no msg_id set → won't match

    let result = crate::core::session::manager::CompactionResult {
        summary_text: "summary".into(),
        covered_start_id: "nonexistent".into(),
        covered_end_id: "also_nonexistent".into(),
        covered_count: 1,
        transcript_compaction_entry_id: None,
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        preheat_elapsed_ms: 0,
    };
    let res = state.apply_boundary(result);
    assert!(matches!(
        res,
        Err(AppError::ApplyBoundaryStale { covered_end_id }) if covered_end_id == "also_nonexistent"
    ));
}

#[test]
fn apply_boundary_missing_start_id_splices_from_zero_to_end() {
    let mut state = make_state(0, 100_000, 25_000);
    let m = user_msg_with_id("still_end", &"b".repeat(1000));
    state.messages = vec![m];
    state.estimate_context_chars = 5_000;

    let result = crate::core::session::manager::CompactionResult {
        summary_text: "merged".into(),
        covered_start_id: "gone_start".into(),
        covered_end_id: "still_end".into(),
        covered_count: 2,
        transcript_compaction_entry_id: None,
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        preheat_elapsed_ms: 0,
    };
    state.apply_boundary(result).unwrap();
    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.messages[0].kind, MessageKind::CompactionSummary);
    assert_eq!(state.messages[0].text_content(), Some("merged"));
}

#[test]
fn check_after_reply_skips_below_085() {
    use crate::infra::event_bus::DefaultEventBus;
    let eb = std::sync::Arc::new(DefaultEventBus::new());
    let emitter = crate::infra::ScopedEventEmitter::new(eb, "s-apply-test");
    let mut state = make_state(0, 0, 1000);
    state.update_api_usage(500, 0);
    let switched = check_after_reply(&mut state, &emitter);
    assert!(!switched, "ratio 0.50 should not trigger check_after_reply");
}

#[test]
fn check_after_reply_skips_when_no_preheat() {
    use crate::infra::event_bus::DefaultEventBus;
    let eb = std::sync::Arc::new(DefaultEventBus::new());
    let emitter = crate::infra::ScopedEventEmitter::new(eb, "s-apply-test");
    let mut state = make_state(0, 0, 1000);
    state.update_api_usage(900, 0);
    let switched = check_after_reply(&mut state, &emitter);
    assert!(!switched, "idle preheat should skip");
}

#[test]
fn check_after_reply_boundary_switched_event_carries_session_id() {
    let bus: std::sync::Arc<dyn EventBus> = std::sync::Arc::new(DefaultEventBus::new());
    let captured = std::sync::Arc::new(std::sync::Mutex::new(None::<EventContext>));
    let captured_cb = std::sync::Arc::clone(&captured);
    bus.on(
        wire::WIRE_BOUNDARY_SWITCHED,
        Box::new(move |ctx: EventContext| {
            *captured_cb.lock().unwrap() = Some(ctx);
            Ok(())
        }),
    );
    let emitter = crate::infra::ScopedEventEmitter::new(bus, "sid-apply-boundary");
    let mut state = make_state(0, 1_000, 250);
    state.update_api_usage(900, 0);
    state.messages = vec![
        user_msg_with_id("start", "a"),
        user_msg_with_id("end", "b"),
        user_msg_with_id("tail", "c"),
    ];
    state
        .preheat
        .restore_completed(crate::core::session::manager::CompactionResult {
            summary_text: "summary".into(),
            covered_start_id: "start".into(),
            covered_end_id: "end".into(),
            covered_count: 2,
            transcript_compaction_entry_id: None,
            estimated_covered_tokens_before: Some(10),
            estimated_summary_tokens: Some(2),
            estimated_tokens_saved: Some(8),
            preheat_elapsed_ms: 0,
        });

    let switched = check_after_reply(&mut state, &emitter);
    assert!(switched, "预热完成时应应用 boundary");

    let ctx = captured
        .lock()
        .unwrap()
        .clone()
        .expect("应捕获到 boundary_switched");
    assert_eq!(ctx.session_id.as_deref(), Some("sid-apply-boundary"));
    assert_eq!(
        ctx.payload.get("sessionId").and_then(|v| v.as_str()),
        Some("sid-apply-boundary")
    );
}

#[test]
fn preheat_discard_cached_completed_only_clears_cached() {
    let mut p = Preheat::new();
    p.restore_completed(dummy_compaction_result());
    assert!(p.is_finished());
    p.discard_cached_completed();
    assert!(p.is_idle());
    p.discard_cached_completed();
    assert!(p.is_idle());
}

#[test]
fn check_after_reply_stale_apply_removes_branch_summary_and_keeps_preheat_idle() {
    use crate::infra::event_bus::DefaultEventBus;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stale_apply.jsonl");
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
    let entry_id = compound_turn_id("gone_start", "stale_end");
    let branch = TranscriptEntry::BranchSummary(BranchSummaryEntry {
        id: Some(entry_id.clone()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:01.000Z".to_string(),
        summary: Some("pending sum".to_string()),
        covered_start_id: Some("gone_start".to_string()),
        covered_end_id: Some("stale_end".to_string()),
        covered_count: Some(1),
        is_boundary: Some(false),
        preheat_compaction_id: Some(entry_id.clone()),
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        error: None,
        attempts: None,
    });
    append_entry(&path, &branch).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2);

    let eb = std::sync::Arc::new(DefaultEventBus::new());
    let emitter = crate::infra::ScopedEventEmitter::new(eb, "s-apply-test");
    let mut state = make_state(0, 0, 1000);
    state.transcript_path = path.clone();
    state.update_api_usage(900, 0);
    // "still_end" is not the covered_end_id "stale_end" → stale apply
    state.messages = vec![user_msg_with_id("still_end", "x")];
    let stale_result = crate::core::session::manager::CompactionResult {
        summary_text: "sum".into(),
        covered_start_id: "gone_start".into(),
        covered_end_id: "stale_end".into(),
        covered_count: 2,
        transcript_compaction_entry_id: Some(entry_id),
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        preheat_elapsed_ms: 0,
    };
    state.preheat.restore_completed(stale_result);
    let switched = check_after_reply(&mut state, &emitter);
    assert!(!switched, "stale apply should not emit boundary switched");
    assert!(
        state.preheat.is_idle(),
        "stale path must not restore_pending_result → stay idle"
    );
    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        raw.lines().count(),
        1,
        "branch_summary line should be removed; only header remains"
    );
    read_header(&path).unwrap();
}

#[test]
fn layer0_threshold_from_config() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = make_state(60_000, 100_000, 25_000);
    let big_content = "x".repeat(60_000);
    state.messages = vec![user_msg("q"), tool_msg("tc_cfg", &big_content)];

    let config = ContextConfig {
        layer0_single_result_max_chars: 100_000,
        ..Default::default()
    };
    let (results, _) = layer0_persist_large_results(&mut state, &config, dir.path(), "test");
    assert!(
        results.is_empty(),
        "60K < 100K threshold should NOT persist"
    );

    let config2 = ContextConfig {
        layer0_single_result_max_chars: 50_000,
        ..Default::default()
    };
    let mut state2 = make_state(60_000, 100_000, 25_000);
    state2.messages = vec![user_msg("q"), tool_msg("tc_cfg2", &"y".repeat(60_000))];
    let (results2, _) = layer0_persist_large_results(&mut state2, &config2, dir.path(), "test");
    assert_eq!(results2.len(), 1, "60K > 50K threshold should persist");
}
