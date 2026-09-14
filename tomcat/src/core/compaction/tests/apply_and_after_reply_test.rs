use super::super::apply::{
    apply_and_emit_boundary, check_after_reply, check_before_request, BoundaryEnv,
};
use super::super::layer0_persist_large_results;
use super::mocks::*;
use crate::core::agent_loop::{execute_tool_for_cross_module_test, ToolCallInfo};
use crate::core::compaction::preheat::Preheat;
use crate::core::llm::{ChatMessageRole, MessageKind};
use crate::core::permission::{DefaultPermissionGate, GateConfig, PermissionGate, SessionGrants};
use crate::core::session::manager::{compound_turn_id, estimate_msg_chars};
use crate::core::session::transcript::{
    append_entry, read_header, write_header, BranchSummaryEntry, SessionHeader, TranscriptEntry,
};
use crate::core::tools::pipeline::read_state::{ReadFileState, ReadStamp};
use crate::core::tools::primitive::{DefaultPrimitiveExecutor, PrimitiveExecutor};
use crate::core::AllowAllConfirmation;
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;
use crate::infra::{
    wire, DefaultEventBus, EventBus, EventContext, PrimitiveConfig, TracingAuditRecorder,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn boundary_env<'a>(
    config: &'a ContextConfig,
    work_dir: &'a Path,
    read_file_state: &'a ReadFileState,
) -> BoundaryEnv<'a> {
    BoundaryEnv {
        config,
        work_dir,
        session_id: "s-apply-test",
        read_file_state,
    }
}

fn make_read_executor(dir: &Path) -> Arc<dyn PrimitiveExecutor> {
    let gate: Arc<dyn PermissionGate> = DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: dir.to_path_buf(),
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    )
    .into_arc();
    Arc::new(DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        gate,
    ))
}

fn l0_test_config() -> ContextConfig {
    ContextConfig {
        keep_recent_turns: 1,
        layer0_single_result_max_chars: 500,
        layer0_placeholder_threshold_chars: 100,
        ..Default::default()
    }
}

fn l0_test_state() -> crate::core::session::manager::ContextState {
    let mut state = make_state(0, 10_000, 1_000);
    state.messages = vec![
        user_msg_with_id("covered-start", "covered start"),
        user_msg_with_id("covered-end", "covered end"),
        user_msg_with_id("old-tool-turn", "old tool turn"),
        tool_msg("old-tool-call", &"o".repeat(250)),
        user_msg_with_id("latest-tool-turn", "latest tool turn"),
        tool_msg("latest-tool-call", &"n".repeat(600)),
    ];
    state.estimate_context_chars = state
        .messages
        .iter()
        .filter_map(|message| message.text_content())
        .map(str::len)
        .sum();
    state
}

fn l0_test_result() -> crate::core::session::manager::CompactionResult {
    crate::core::session::manager::CompactionResult {
        summary_text: "boundary summary".into(),
        covered_start_id: "covered-start".into(),
        covered_end_id: "covered-end".into(),
        covered_count: 2,
        transcript_compaction_entry_id: Some(compound_turn_id("covered-start", "covered-end")),
        estimated_covered_tokens_before: Some(50),
        estimated_summary_tokens: Some(5),
        estimated_tokens_saved: Some(45),
        preheat_elapsed_ms: 0,
    }
}

fn install_read_stamp(state: &ReadFileState, path: &str, tool_call_id: &str) {
    state.put(
        PathBuf::from(path),
        ReadStamp {
            mtime_ms: 1,
            size: 1,
            content_hash: 1,
            offset: None,
            limit: None,
            is_partial_view: false,
            render_mode: crate::core::tools::pipeline::read_state::ReadRenderMode::Plain,
            covered_lines: Some((1, 1)),
            reached_eof: true,
            tool_call_id: Some(tool_call_id.to_string()),
        },
    );
}

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
    let mut turn_start = state.messages.len();
    state.apply_boundary(result, &mut turn_start).unwrap();

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
    let mut turn_start = state.messages.len();
    let res = state.apply_boundary(result, &mut turn_start);
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
    let mut turn_start = state.messages.len();
    state.apply_boundary(result, &mut turn_start).unwrap();
    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.messages[0].kind, MessageKind::CompactionSummary);
    assert_eq!(state.messages[0].text_content(), Some("merged"));
}

#[test]
fn apply_boundary_shifts_turn_start_for_history_boundary_and_tail_boundary() {
    let result_for = |covered_end_id: &str| crate::core::session::manager::CompactionResult {
        summary_text: "summary".into(),
        covered_start_id: "m0".into(),
        covered_end_id: covered_end_id.into(),
        covered_count: 1,
        transcript_compaction_entry_id: None,
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        preheat_elapsed_ms: 0,
    };
    let state_for = || {
        let mut state = make_state(0, 100_000, 25_000);
        state.messages = vec![
            user_msg_with_id("m0", "zero"),
            user_msg_with_id("m1", "one"),
            user_msg_with_id("m2", "two"),
            user_msg_with_id("m3", "three"),
        ];
        state.estimate_context_chars = state.messages.iter().map(estimate_msg_chars).sum();
        state
    };

    // The boundary ends before the current turn. Only the trailing current-turn message moves.
    let mut state = state_for();
    let mut turn_start = 3;
    state
        .apply_boundary(result_for("m1"), &mut turn_start)
        .unwrap();
    assert_eq!(turn_start, 2);
    assert_eq!(state.messages[turn_start].msg_id.as_deref(), Some("m3"));

    // The boundary ends immediately before the current turn.
    let mut state = state_for();
    let mut turn_start = 2;
    state
        .apply_boundary(result_for("m1"), &mut turn_start)
        .unwrap();
    assert_eq!(turn_start, 1);
    assert_eq!(state.messages[turn_start].msg_id.as_deref(), Some("m2"));

    // The boundary consumes the first current-turn message. The surviving tail starts after S.
    let mut state = state_for();
    let mut turn_start = 2;
    state
        .apply_boundary(result_for("m2"), &mut turn_start)
        .unwrap();
    assert_eq!(turn_start, 1);
    assert_eq!(state.messages[turn_start].msg_id.as_deref(), Some("m3"));
}

#[test]
fn check_after_reply_skips_below_085() {
    use crate::infra::event_bus::DefaultEventBus;
    let eb = std::sync::Arc::new(DefaultEventBus::new());
    let emitter = crate::infra::ScopedEventEmitter::new(eb, "s-apply-test");
    let mut state = make_state(0, 0, 1000);
    state.update_api_usage(500, 0);
    let config = ContextConfig::default();
    let read_file_state = ReadFileState::default();
    let dir = tempfile::tempdir().unwrap();
    let env = boundary_env(&config, dir.path(), &read_file_state);
    let switched = check_after_reply(&mut state, &emitter, &env);
    assert!(!switched, "ratio 0.50 should not trigger check_after_reply");
}

#[test]
fn check_after_reply_skips_when_no_preheat() {
    use crate::infra::event_bus::DefaultEventBus;
    let eb = std::sync::Arc::new(DefaultEventBus::new());
    let emitter = crate::infra::ScopedEventEmitter::new(eb, "s-apply-test");
    let mut state = make_state(0, 0, 1000);
    state.update_api_usage(900, 0);
    let config = ContextConfig::default();
    let read_file_state = ReadFileState::default();
    let dir = tempfile::tempdir().unwrap();
    let env = boundary_env(&config, dir.path(), &read_file_state);
    let switched = check_after_reply(&mut state, &emitter, &env);
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

    let config = ContextConfig::default();
    let read_file_state = ReadFileState::default();
    let dir = tempfile::tempdir().unwrap();
    let env = boundary_env(&config, dir.path(), &read_file_state);
    let switched = check_after_reply(&mut state, &emitter, &env);
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
fn successful_boundary_runs_both_layer0_steps_invalidates_read_stamps_and_emits_once() {
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let layer0_events = Arc::new(AtomicUsize::new(0));
    let layer0_events_cb = Arc::clone(&layer0_events);
    bus.on(
        wire::WIRE_LAYER0_CONTEXT_RELEASE,
        Box::new(move |_| {
            layer0_events_cb.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    );
    let emitter = crate::infra::ScopedEventEmitter::new(bus, "s-apply-test");
    let config = l0_test_config();
    let read_file_state = ReadFileState::default();
    install_read_stamp(&read_file_state, "old-tool.txt", "old-tool-call");
    install_read_stamp(&read_file_state, "latest-tool.txt", "latest-tool-call");
    let dir = tempfile::tempdir().unwrap();
    let env = boundary_env(&config, dir.path(), &read_file_state);
    let mut state = l0_test_state();
    let chars_before = state.estimate_context_chars;
    let mut turn_start = state.messages.len();

    assert!(apply_and_emit_boundary(
        &mut state,
        l0_test_result(),
        0.85,
        false,
        &emitter,
        &env,
        &mut turn_start,
    ));

    assert_eq!(
        state
            .messages
            .iter()
            .filter(|message| {
                message.role == ChatMessageRole::Tool
                    && message.text_content()
                        == Some("[Previous tool result replaced to save context space]")
            })
            .count(),
        1,
        "the old, compactable tool result must become exactly one placeholder"
    );
    assert_eq!(
        state
            .messages
            .iter()
            .filter(|message| {
                message.role == ChatMessageRole::Tool
                    && message
                        .text_content()
                        .is_some_and(|text| text.starts_with("[Tool result persisted:"))
            })
            .count(),
        1,
        "the large result in the latest completed turn must be persisted exactly once"
    );
    assert!(
        dir.path()
            .join("tool-results")
            .join("s-apply-test")
            .join("latest-tool-call.txt")
            .is_file(),
        "persist step must leave the original large result on disk"
    );
    assert!(
        read_file_state.is_empty(),
        "all read stamps pointing at evicted results must be invalidated"
    );
    assert!(
        state.estimate_context_chars < chars_before,
        "boundary plus L0 must reduce the fallback context estimate"
    );
    assert_eq!(
        layer0_events.load(Ordering::SeqCst),
        1,
        "the successful boundary emits one aggregate L0 release event"
    );
}

#[tokio::test]
async fn boundary_eviction_forces_a_real_reread_instead_of_an_unchanged_stub() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("evicted-read.txt");
    std::fs::write(&file, "alpha\nbeta\n").unwrap();
    let primitive = make_read_executor(dir.path());
    let read_file_state = Arc::new(ReadFileState::new());

    // First read creates a stamp owned by the same tool result that Layer 0 will evict below.
    let first_call = ToolCallInfo {
        id: "old-tool-call".to_string(),
        name: "read".to_string(),
        arguments: serde_json::json!({
            "path": file.to_string_lossy(),
            "line_numbers": false
        })
        .to_string(),
    };
    let (first, first_error, _) =
        execute_tool_for_cross_module_test(&primitive, &read_file_state, &first_call).await;
    assert!(!first_error, "{first}");
    assert_eq!(read_file_state.len(), 1, "first read must create a stamp");

    // This exercises the production apply -> Layer 0 cleanup path rather than calling
    // invalidate_tool_call directly. l0_test_state makes "old-tool-call" a compactable result.
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let emitter = crate::infra::ScopedEventEmitter::new(bus, "s-reread-after-eviction");
    let config = l0_test_config();
    let env = boundary_env(&config, dir.path(), read_file_state.as_ref());
    let mut state = l0_test_state();
    let mut turn_start = state.messages.len();
    assert!(apply_and_emit_boundary(
        &mut state,
        l0_test_result(),
        0.85,
        false,
        &emitter,
        &env,
        &mut turn_start,
    ));
    assert!(
        read_file_state.is_empty(),
        "evicting the visible tool result must also invalidate its read stamp"
    );

    let reread_call = ToolCallInfo {
        id: "reread-after-eviction".to_string(),
        name: "read".to_string(),
        arguments: serde_json::json!({
            "path": file.to_string_lossy(),
            "line_numbers": false
        })
        .to_string(),
    };
    let (reread, reread_error, _) =
        execute_tool_for_cross_module_test(&primitive, &read_file_state, &reread_call).await;
    assert!(!reread_error, "{reread}");
    assert!(
        reread.contains("alpha") && !reread.contains("File unchanged"),
        "an evicted read must be fetched again, not replaced with a dangling stub: {reread}"
    );
}

#[test]
fn boundary_without_layer0_savings_does_not_emit_release_event() {
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let layer0_events = Arc::new(AtomicUsize::new(0));
    let layer0_events_cb = Arc::clone(&layer0_events);
    bus.on(
        wire::WIRE_LAYER0_CONTEXT_RELEASE,
        Box::new(move |_| {
            layer0_events_cb.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    );
    let emitter = crate::infra::ScopedEventEmitter::new(bus, "s-apply-test");
    let config = ContextConfig::default();
    let read_file_state = ReadFileState::default();
    let dir = tempfile::tempdir().unwrap();
    let env = boundary_env(&config, dir.path(), &read_file_state);
    let mut state = make_state(100, 4_000, 1_000);
    state.messages = vec![
        user_msg_with_id("covered-start", "covered start"),
        user_msg_with_id("covered-end", "covered end"),
        user_msg_with_id("small-tail", "small tail"),
    ];
    let mut turn_start = state.messages.len();

    assert!(apply_and_emit_boundary(
        &mut state,
        l0_test_result(),
        0.85,
        false,
        &emitter,
        &env,
        &mut turn_start,
    ));
    assert_eq!(
        layer0_events.load(Ordering::SeqCst),
        0,
        "a successful boundary with no persisted or placeholdered text emits no empty release"
    );
    assert_eq!(
        state.session_obs.tool_result_chars_persisted, 0,
        "no L0 persistence should be recorded"
    );
}

#[tokio::test]
async fn all_boundary_entry_paths_inherit_layer0_cleanup() {
    for prompt_tokens in [700, 980] {
        let config = l0_test_config();
        let read_file_state = ReadFileState::default();
        let dir = tempfile::tempdir().unwrap();
        let env = boundary_env(&config, dir.path(), &read_file_state);
        let emitter =
            crate::infra::ScopedEventEmitter::new(Arc::new(DefaultEventBus::new()), "s-apply-test");
        let mut state = l0_test_state();
        state.update_api_usage(prompt_tokens, 0);
        state.preheat.restore_completed(l0_test_result());

        assert!(
            check_before_request(&mut state, &emitter, &env).await,
            "timing② must apply the ready boundary at ratio {}",
            prompt_tokens as f64 / 1000.0
        );
        assert_eq!(
            state
                .messages
                .iter()
                .filter(|message| {
                    message.text_content()
                        == Some("[Previous tool result replaced to save context space]")
                })
                .count(),
            1,
            "timing② must inherit one placeholder cleanup"
        );
        let persisted_dir = dir.path().join("tool-results").join("s-apply-test");
        assert_eq!(
            std::fs::read_dir(&persisted_dir)
                .expect("timing② persist directory")
                .count(),
            1,
            "timing② must inherit one persisted result"
        );
    }

    let config = l0_test_config();
    let read_file_state = ReadFileState::default();
    let dir = tempfile::tempdir().unwrap();
    let env = boundary_env(&config, dir.path(), &read_file_state);
    let emitter =
        crate::infra::ScopedEventEmitter::new(Arc::new(DefaultEventBus::new()), "s-apply-test");
    let mut state = l0_test_state();
    state.update_api_usage(850, 0);
    state.preheat.restore_completed(l0_test_result());

    assert!(
        check_after_reply(&mut state, &emitter, &env),
        "timing⑤ must apply its ready boundary"
    );
    assert_eq!(
        state
            .messages
            .iter()
            .filter(|message| {
                message.text_content()
                    == Some("[Previous tool result replaced to save context space]")
            })
            .count(),
        1,
        "timing⑤ must inherit one placeholder cleanup"
    );
    let persisted_dir = dir.path().join("tool-results").join("s-apply-test");
    assert_eq!(
        std::fs::read_dir(&persisted_dir)
            .expect("timing⑤ persist directory")
            .count(),
        1,
        "timing⑤ must inherit one persisted result"
    );
}

#[test]
fn skipped_or_stale_boundary_never_rewrites_layer0_history() {
    let config = l0_test_config();
    let read_file_state = ReadFileState::default();
    let dir = tempfile::tempdir().unwrap();
    let env = boundary_env(&config, dir.path(), &read_file_state);
    let emitter =
        crate::infra::ScopedEventEmitter::new(Arc::new(DefaultEventBus::new()), "s-apply-test");

    let mut below_threshold = l0_test_state();
    let below_original = serde_json::to_vec(&below_threshold.messages).unwrap();
    below_threshold.update_api_usage(840, 0);
    below_threshold.preheat.restore_completed(l0_test_result());
    assert!(!check_after_reply(&mut below_threshold, &emitter, &env));
    assert_eq!(
        serde_json::to_vec(&below_threshold.messages).unwrap(),
        below_original,
        "below 0.85 must not rewrite history"
    );

    let mut no_preheat = l0_test_state();
    let no_preheat_original = serde_json::to_vec(&no_preheat.messages).unwrap();
    no_preheat.update_api_usage(850, 0);
    assert!(!check_after_reply(&mut no_preheat, &emitter, &env));
    assert_eq!(
        serde_json::to_vec(&no_preheat.messages).unwrap(),
        no_preheat_original,
        "an idle preheat must not rewrite history"
    );

    let mut stale = l0_test_state();
    let stale_original = serde_json::to_vec(&stale.messages).unwrap();
    stale.update_api_usage(850, 0);
    let mut stale_result = l0_test_result();
    stale_result.covered_end_id = "missing-end".to_string();
    stale.preheat.restore_completed(stale_result);
    assert!(!check_after_reply(&mut stale, &emitter, &env));
    assert_eq!(
        serde_json::to_vec(&stale.messages).unwrap(),
        stale_original,
        "a stale boundary must not run L0"
    );
    assert_eq!(
        stale.session_obs.compaction_count, 0,
        "a failed apply must not increment boundary compaction accounting"
    );
    assert_eq!(
        stale.session_obs.compaction_tokens_freed, 0,
        "a failed apply must not record any L2 or L0 release"
    );
    assert!(
        !dir.path().join("tool-results").exists(),
        "no skipped or stale path may write a persisted tool result"
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
fn applying_the_same_preheat_result_twice_writes_one_linked_summary_body() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("apply.jsonl");
    write_header(
        &path,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(4),
            id: "sid".to_string(),
            timestamp: "2026-09-10T00:00:00.000Z".to_string(),
            cwd: None,
            project_root: None,
        },
    )
    .unwrap();
    let result = l0_test_result();
    let marker_id = result.transcript_compaction_entry_id.clone().unwrap();
    append_entry(
        &path,
        &TranscriptEntry::BranchSummary(BranchSummaryEntry {
            id: Some(marker_id.clone()),
            parent_id: None,
            timestamp: "2026-09-10T00:00:01.000Z".to_string(),
            summary: None,
            covered_start_id: Some(result.covered_start_id.clone()),
            covered_end_id: Some(result.covered_end_id.clone()),
            covered_count: Some(result.covered_count),
            is_boundary: Some(true),
            preheat_compaction_id: Some(marker_id.clone()),
            estimated_covered_tokens_before: None,
            estimated_summary_tokens: None,
            estimated_tokens_saved: None,
            error: None,
            attempts: None,
        }),
    )
    .unwrap();

    let mut state = l0_test_state();
    state.transcript_path = path.clone();
    let config = l0_test_config();
    let read_file_state = ReadFileState::default();
    let env = boundary_env(&config, dir.path(), &read_file_state);
    let emitter =
        crate::infra::ScopedEventEmitter::new(Arc::new(DefaultEventBus::new()), "s-apply-test");

    let mut turn_start = state.messages.len();
    assert!(apply_and_emit_boundary(
        &mut state,
        result.clone(),
        0.85,
        false,
        &emitter,
        &env,
        &mut turn_start,
    ));
    // Model the retry/restart boundary where memory still contains the raw covered range, but
    // the durable body was already appended. The tail guard must make the second apply idempotent.
    let mut retry_state = l0_test_state();
    retry_state.transcript_path = path.clone();
    let mut retry_turn_start = retry_state.messages.len();
    assert!(
        apply_and_emit_boundary(
            &mut retry_state,
            result,
            0.85,
            false,
            &emitter,
            &env,
            &mut retry_turn_start,
        ),
        "a retry with raw memory should reuse the existing durable body"
    );

    let entries = crate::core::session::transcript::read_entries_tail(&path, 8).unwrap();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| {
                matches!(entry, TranscriptEntry::BranchSummaryText(body) if body.for_id == marker_id)
            })
            .count(),
        1,
        "the same marker must never receive two body rows"
    );
    assert!(entries
        .iter()
        .all(|entry| !matches!(entry, TranscriptEntry::Message(_))));
}

#[test]
fn check_after_reply_stale_apply_keeps_history_and_preheat_idle() {
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
            project_root: None,
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
    let config = ContextConfig::default();
    let read_file_state = ReadFileState::default();
    let dir = tempfile::tempdir().unwrap();
    let env = boundary_env(&config, dir.path(), &read_file_state);
    let switched = check_after_reply(&mut state, &emitter, &env);
    assert!(!switched, "stale apply should not emit boundary switched");
    assert!(
        state.preheat.is_idle(),
        "stale path must not restore_pending_result → stay idle"
    );
    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        raw.lines().count(),
        2,
        "a stale marker is harmless during reload and must not trigger a costly transcript rewrite"
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
    let history_end = state.messages.len();
    let (results, _) =
        layer0_persist_large_results(&mut state, &config, dir.path(), "test", history_end);
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
    let history_end = state2.messages.len();
    let (results2, _) =
        layer0_persist_large_results(&mut state2, &config2, dir.path(), "test", history_end);
    assert_eq!(results2.len(), 1, "60K > 50K threshold should persist");
}
