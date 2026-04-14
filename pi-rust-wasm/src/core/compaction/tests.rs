use super::truncation::{floor_char_boundary, TOOL_RESULT_PLACEHOLDER};
use super::*;
use crate::core::compaction::preheat::Preheat;
use crate::core::llm::{ChatMessage, ChatMessageRole, MessageKind};
use crate::core::session::manager::{
    build_context_from_state, compound_turn_id, CompactionResult, ContextState,
};
use crate::core::session::transcript::{
    append_entry, read_header, write_header, BranchSummaryEntry, SessionHeader, TranscriptEntry,
};
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;

const TS: &str = "2026-04-04T12:00:00Z";

// ---------------------------------------------------------------------------
// Helper factories
// ---------------------------------------------------------------------------

fn user_msg_with_id(id: &str, text: &str) -> ChatMessage {
    let mut m = ChatMessage::user(text);
    m.msg_id = Some(id.to_string());
    m.timestamp = Some(TS.to_string());
    m
}

fn tool_msg(tcid: &str, content: &str) -> ChatMessage {
    ChatMessage::tool(tcid, content)
}

fn tool_msg_with_id(id: &str, tcid: &str, content: &str) -> ChatMessage {
    let mut m = ChatMessage::tool(tcid, content);
    m.msg_id = Some(id.to_string());
    m.timestamp = Some(TS.to_string());
    m
}

fn user_msg(text: &str) -> ChatMessage {
    let mut m = ChatMessage::user(text);
    m.timestamp = Some(TS.to_string());
    m
}

fn dummy_compaction_result() -> CompactionResult {
    CompactionResult {
        summary_text: "summary".into(),
        covered_start_id: "start".into(),
        covered_end_id: "end".into(),
        covered_count: 1,
        transcript_compaction_entry_id: None,
        estimated_covered_tokens_before: Some(10),
        estimated_summary_tokens: Some(2),
        estimated_tokens_saved: Some(8),
        preheat_elapsed_ms: 0,
    }
}

fn make_state(chars: usize, budget_chars: usize, budget_tokens: usize) -> ContextState {
    ContextState {
        messages: vec![],
        estimate_context_chars: chars,
        context_budget_chars: budget_chars,
        context_budget_tokens: budget_tokens,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn preheat_restore_pending_result_keeps_non_idle_until_consumed() {
    let mut p = Preheat::new();
    assert!(p.is_idle());
    p.restore_pending_result(dummy_compaction_result());
    assert!(!p.is_idle());
    assert!(p.is_finished());
}

#[test]
fn preheat_warmup_active_vs_result_pending() {
    let mut p = Preheat::new();
    assert!(!p.is_warmup_task_active());
    assert!(!p.preheat_result_pending());
    p.restore_completed(dummy_compaction_result());
    assert!(!p.is_warmup_task_active());
    assert!(p.preheat_result_pending());
}

#[test]
fn floor_char_boundary_ascii() {
    let s = "hello world";
    assert_eq!(floor_char_boundary(s, 5), 5);
    assert_eq!(floor_char_boundary(s, 100), s.len());
    assert_eq!(floor_char_boundary(s, 0), 0);
}

#[test]
fn floor_char_boundary_multibyte() {
    let s = "你好世界"; // 4 chars, 12 bytes
    assert_eq!(floor_char_boundary(s, 3), 3);
    assert_eq!(floor_char_boundary(s, 4), 3);
    assert_eq!(floor_char_boundary(s, 5), 3);
    assert_eq!(floor_char_boundary(s, 6), 6);
}

#[test]
fn compact_tool_results_reduces_budget() {
    let mut state = make_state(11_000, 5_000, 1_250);
    // Turn 1: [user, large tool result]  Turn 2: [user] — m=1 protects turn 2
    state.messages = vec![
        user_msg("q"),
        tool_msg("c1", &"x".repeat(25_000)),
        user_msg("q2"),
    ];
    let reduced = compact_tool_results(&mut state, &ContextConfig::default(), 1);
    assert!(reduced > 0);
}

#[test]
fn compact_tool_results_protects_recent() {
    let tool_content = "x".repeat(25_000);
    let mut state = make_state(25_000, 5_000, 1_250);
    // Only one turn (one user message), m=1 → everything protected
    state.messages = vec![
        user_msg("q"),
        tool_msg("c1", &tool_content),
    ];
    let reduced = compact_tool_results(&mut state, &ContextConfig::default(), 1);
    assert_eq!(reduced, 0);
}

#[test]
fn compact_tool_results_skips_small() {
    let mut state = make_state(5_000, 3_000, 750);
    // Small tool result (1000 < 10_000 threshold) → not replaced
    state.messages = vec![
        user_msg("q"),
        tool_msg("c1", &"x".repeat(1_000)),
        user_msg("q2"),
    ];
    let reduced = compact_tool_results(&mut state, &ContextConfig::default(), 1);
    assert_eq!(reduced, 0);
}

#[test]
fn force_drop_oldest_to_target_below_half() {
    let mut state = make_state(4000, 4000, 1000);
    state.messages = vec![
        user_msg(&"x".repeat(2000)),
        user_msg(&"y".repeat(1000)),
        user_msg(&"z".repeat(500)),
    ];
    force_drop_oldest_to_target(&mut state);
    assert!(state.usage_ratio() < 0.50);
}

#[test]
fn is_context_overflow_error_matches() {
    assert!(is_context_overflow_error(
        "context length exceeded: 500000 tokens"
    ));
    assert!(is_context_overflow_error(
        "maximum context token limit reached"
    ));
    assert!(!is_context_overflow_error("API error 429: rate limit"));
}

#[test]
fn context_state_on_message_appended() {
    let mut state = make_state(100, 1000, 250);
    state.on_message_appended(500);
    assert_eq!(state.estimate_context_chars, 600);
    assert_eq!(state.post_usage_appended_chars, 500);
    assert!(!state.is_over_budget());
    state.on_message_appended(500);
    assert!(state.is_over_budget());
}

#[test]
fn context_state_messages_push() {
    let mut state = make_state(0, 1000, 250);
    // on_message_appended is called when a message arrives; messages are pushed after
    state.on_message_appended(5);
    state.messages.push(user_msg("hello"));
    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.estimate_context_chars, 5);
}

#[test]
fn layer0_persist_creates_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = make_state(60_000, 100_000, 25_000);
    let big_content = "x".repeat(60_000);
    // Layer 0 persists tool results from the last turn (after the last user message)
    state.messages = vec![
        user_msg("question"),
        tool_msg_with_id("tc_1_msg", "tc_1", &big_content),
    ];
    let config = ContextConfig::default();
    let (results, _) =
        layer0_persist_large_results(&mut state, &config, dir.path(), "test_session");
    assert_eq!(results.len(), 1);
    assert!(std::path::Path::new(&results[0].persisted_path).exists());
    assert!(state.estimate_context_chars < 60_000);
    // Check the tool message content was replaced
    let tool = state.messages.iter().find(|m| m.role == ChatMessageRole::Tool).unwrap();
    assert!(tool.text_content().unwrap_or("").starts_with("[Tool result persisted:"));
}

#[test]
fn layer0_persist_skips_small() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = make_state(1_000, 100_000, 25_000);
    state.messages = vec![
        user_msg("q"),
        tool_msg("tc_2", "small"),
    ];
    let config = ContextConfig::default();
    let (results, _) =
        layer0_persist_large_results(&mut state, &config, dir.path(), "test_session");
    assert!(results.is_empty());
}

// --- V2 新增测试 ---

#[test]
fn estimated_token_count_with_usage() {
    let mut state = make_state(0, 0, 1000);
    state.update_api_usage(500, 100);
    assert_eq!(state.estimated_token_count(), 600);
    state.on_message_appended(400);
    assert_eq!(state.estimated_token_count(), 700);
}

#[test]
fn estimated_token_count_fallback_without_usage() {
    let state = make_state(4000, 10000, 1000);
    assert_eq!(state.estimated_token_count(), 1000);
}

#[test]
fn usage_ratio_various_levels() {
    let mut state = make_state(0, 0, 1000);
    state.update_api_usage(700, 0);
    let r = state.usage_ratio();
    assert!((r - 0.70).abs() < 0.001);

    state.update_api_usage(850, 0);
    assert!((state.usage_ratio() - 0.85).abs() < 0.001);
}

#[test]
fn usage_ratio_zero_budget_returns_max() {
    let state = make_state(100, 100, 0);
    assert_eq!(state.usage_ratio(), f64::MAX);
}

#[test]
fn invalidate_api_usage_resets_to_fallback() {
    let mut state = make_state(2000, 10000, 1000);
    state.update_api_usage(800, 0);
    assert_eq!(state.estimated_token_count(), 800);
    state.invalidate_api_usage();
    assert_eq!(state.estimated_token_count(), 500);
}

#[test]
fn compact_tool_results_skips_already_persisted() {
    let mut state = make_state(30_000, 5_000, 1_250);
    state.messages = vec![
        user_msg("q"),
        tool_msg(
            "c1",
            "[Tool result persisted: /tmp/foo.txt (50000 chars)]\nPreview: ...",
        ),
        user_msg("q2"),
    ];
    let reduced = compact_tool_results(&mut state, &ContextConfig::default(), 1);
    assert_eq!(
        reduced, 0,
        "already persisted results should not be replaced"
    );
}

#[test]
fn compact_tool_results_skips_placeholder() {
    let mut state = make_state(30_000, 5_000, 1_250);
    state.messages = vec![
        user_msg("q"),
        tool_msg("c1", TOOL_RESULT_PLACEHOLDER),
        user_msg("q2"),
    ];
    let reduced = compact_tool_results(&mut state, &ContextConfig::default(), 1);
    assert_eq!(
        reduced, 0,
        "already replaced results should not be re-replaced"
    );
}

#[test]
fn compact_tool_results_respects_placeholder_threshold_from_config() {
    let big = "x".repeat(25_000);
    let mut state = make_state(30_000, 5_000, 1_250);
    state.messages = vec![
        user_msg("q"),
        tool_msg("c1", &big),
        user_msg("q2"),
    ];
    let high_threshold = ContextConfig {
        layer0_placeholder_threshold_chars: 30_000,
        ..Default::default()
    };
    let reduced = compact_tool_results(&mut state, &high_threshold, 1);
    assert_eq!(
        reduced, 0,
        "content below custom threshold should not be replaced"
    );
    let tool = state.messages.iter().find(|m| m.role == ChatMessageRole::Tool).unwrap();
    assert_eq!(tool.text_content().unwrap_or("").len(), 25_000);
}

#[test]
fn layer0_persist_skips_below_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = make_state(200_000, 500_000, 125_000);
    let medium = "x".repeat(20_000);
    state.messages = vec![
        user_msg("q"),
        tool_msg("tc_a", &medium),
    ];
    let config = ContextConfig::default();
    let (results, _) =
        layer0_persist_large_results(&mut state, &config, dir.path(), "test_session");
    assert!(
        results.is_empty(),
        "20K < 50K threshold should NOT trigger persistence"
    );
}

#[test]
fn layer0_persist_file_readable() {
    let dir = tempfile::tempdir().unwrap();
    let original = "hello world content for persistence test ".repeat(2000);
    let mut state = make_state(original.len(), 100_000, 25_000);
    state.messages = vec![
        user_msg("q"),
        tool_msg("tc_read", &original),
    ];
    let config = ContextConfig::default();
    let (results, _) = layer0_persist_large_results(&mut state, &config, dir.path(), "sess1");
    assert_eq!(results.len(), 1);
    let content = std::fs::read_to_string(&results[0].persisted_path).unwrap();
    assert_eq!(
        content, original,
        "persisted file should contain original content"
    );
}

#[test]
fn force_drop_oldest_to_target_invalidates_usage() {
    let mut state = make_state(4000, 4000, 1000);
    state.update_api_usage(900, 0);
    state.messages = vec![
        user_msg(&"x".repeat(3000)),
        user_msg(&"y".repeat(500)),
    ];
    force_drop_oldest_to_target(&mut state);
    assert!(
        state.last_api_usage.is_none(),
        "usage should be invalidated after force drop"
    );
}

/// 回归：上一轮 `last_api_usage` 很大时，L3 仍应按**字符估算**与 messages 同步删 oldest，
/// 不得因 ratio 长期虚高而删空 `messages`（否则 `build_context_from_state` 为空 → API `messages: []`）。
#[test]
fn force_drop_oldest_respects_chars_not_stale_api_usage() {
    let t_big = user_msg(&"a".repeat(30_000));
    let t_small = user_msg(&"b".repeat(15_000));
    let mut state = make_state(45_000, 200_000, 20_000);
    state.messages = vec![t_big, t_small];
    state.update_api_usage(500_000, 0);
    force_drop_oldest_to_target(&mut state);
    assert_eq!(
        state.messages.len(),
        1,
        "should drop only oldest turn(s) until char-based ratio < 0.5, not drain all"
    );
    let flat = build_context_from_state(&state);
    assert!(
        !flat.is_empty(),
        "non-empty messages must rebuild non-empty context"
    );
}

#[test]
fn is_context_overflow_comprehensive() {
    assert!(is_context_overflow_error("context length exceeded"));
    assert!(is_context_overflow_error("maximum context token limit"));
    assert!(is_context_overflow_error("Context limit exceeded"));
    assert!(!is_context_overflow_error("rate limit exceeded"));
    assert!(!is_context_overflow_error("authentication failed"));
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
    state.apply_boundary(result).unwrap();

    assert_eq!(state.messages.len(), 2);
    assert_eq!(state.messages[0].kind, MessageKind::CompactionSummary);
    assert_eq!(state.messages[0].text_content(), Some("short summary"));
    assert_eq!(state.messages[0].msg_id.as_deref(), Some(compound_turn_id("m0", "m1").as_str()));
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
    let eb = DefaultEventBus::new();
    let mut state = make_state(0, 0, 1000);
    state.update_api_usage(500, 0);
    let switched = super::apply::check_after_reply(&mut state, &eb);
    assert!(!switched, "ratio 0.50 should not trigger check_after_reply");
}

#[test]
fn check_after_reply_skips_when_no_preheat() {
    use crate::infra::event_bus::DefaultEventBus;
    let eb = DefaultEventBus::new();
    let mut state = make_state(0, 0, 1000);
    state.update_api_usage(900, 0);
    let switched = super::apply::check_after_reply(&mut state, &eb);
    assert!(!switched, "idle preheat should skip");
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
    });
    append_entry(&path, &branch).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2);

    let eb = DefaultEventBus::new();
    let mut state = make_state(0, 0, 1000);
    state.transcript_path = path.clone();
    state.update_api_usage(900, 0);
    // "still_end" is not the covered_end_id "stale_end" → stale apply
    state.messages = vec![user_msg_with_id("still_end", "x")];
    let stale_result = CompactionResult {
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
    let switched = super::apply::check_after_reply(&mut state, &eb);
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
    state.messages = vec![
        user_msg("q"),
        tool_msg("tc_cfg", &big_content),
    ];

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
    state2.messages = vec![
        user_msg("q"),
        tool_msg("tc_cfg2", &"y".repeat(60_000)),
    ];
    let (results2, _) = layer0_persist_large_results(&mut state2, &config2, dir.path(), "test");
    assert_eq!(results2.len(), 1, "60K > 50K threshold should persist");
}
