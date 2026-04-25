use super::super::truncation::floor_char_boundary;
use super::super::{
    compact_tool_results, force_drop_oldest_to_target, is_context_overflow_error,
    layer0_persist_large_results,
};
use super::mocks::*;
use crate::core::compaction::preheat::Preheat;
use crate::core::llm::ChatMessageRole;
use crate::infra::config::ContextConfig;

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
    state.messages = vec![user_msg("q"), tool_msg("c1", &tool_content)];
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
    let tool = state
        .messages
        .iter()
        .find(|m| m.role == ChatMessageRole::Tool)
        .unwrap();
    assert!(tool
        .text_content()
        .unwrap_or("")
        .starts_with("[Tool result persisted:"));
}
