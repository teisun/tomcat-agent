use super::super::run_layer0_cleanup;
use super::super::truncation::TOOL_RESULT_PLACEHOLDER;
use super::mocks::*;
use crate::core::llm::{ChatMessage, ChatMessageRole};
use crate::core::session::manager::estimate_msg_chars;
use crate::infra::config::ContextConfig;

// ===========================================================================
// Group A: run_layer0_cleanup 组合测试
// ===========================================================================

/// 构造 N 个 turn，每个 turn 含 [user, tool(content), assistant]
fn build_turns(n: usize, tool_content: &str) -> Vec<ChatMessage> {
    let mut msgs = Vec::new();
    for i in 0..n {
        msgs.push(user_msg_with_id(&format!("u{i}"), &format!("q{i}")));
        msgs.push(tool_msg_with_id(
            &format!("t{i}"),
            &format!("tc{i}"),
            tool_content,
        ));
        msgs.push(assistant_msg(&format!("a{i}")));
    }
    msgs
}

#[test]
fn run_layer0_cleanup_persists_then_compacts() {
    let dir = tempfile::tempdir().unwrap();
    let big = "x".repeat(60_000);

    let msgs = build_turns(8, &big);
    let total: usize = msgs.iter().map(estimate_msg_chars).sum();
    let mut state = make_state(total, total * 2, total / 2);
    state.messages = msgs;

    let config = ContextConfig::default();
    let history_end = state.messages.len();
    let outcome = run_layer0_cleanup(&mut state, &config, dir.path(), "sess_a1", history_end);

    assert!(
        !outcome.persisted.is_empty(),
        "should persist at least one large tool result"
    );

    let compactable_tools: Vec<_> = state
        .messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == ChatMessageRole::Tool)
        .collect();
    let protected_start_turn = 8 - 5; // M_PROTECTED_TURNS=5 → turns 3..7 protected
    let protected_start_idx = protected_start_turn * 3; // 3 msgs per turn
    for (idx, m) in &compactable_tools {
        let text = m.text_content().unwrap_or("");
        if *idx < protected_start_idx {
            assert_eq!(
                text, TOOL_RESULT_PLACEHOLDER,
                "tool at index {} in compactable zone should be placeholder",
                idx
            );
        }
    }

    assert!(
        outcome.persist_chars_freed > 0,
        "persist should free some chars"
    );
    assert!(
        outcome.placeholder_chars_freed > 0,
        "placeholder should free some chars"
    );
}

#[test]
fn run_layer0_cleanup_no_tool_results_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = make_state(500, 10_000, 2_500);
    state.messages = vec![
        user_msg("hello"),
        assistant_msg("hi"),
        user_msg("bye"),
        assistant_msg("see ya"),
    ];

    let before = state.estimate_context_chars;
    let history_end = state.messages.len();
    let outcome = run_layer0_cleanup(
        &mut state,
        &ContextConfig::default(),
        dir.path(),
        "sess_a2",
        history_end,
    );

    assert!(outcome.persisted.is_empty());
    assert_eq!(outcome.persist_chars_freed, 0);
    assert_eq!(outcome.placeholder_chars_freed, 0);
    assert_eq!(state.estimate_context_chars, before);
}

#[test]
fn layer0_persists_previous_tool_turn_when_fresh_user_turn_has_no_results() {
    let dir = tempfile::tempdir().unwrap();
    let large = "x".repeat(60_000);
    let mut state = make_state(0, 100_000, 25_000);
    state.messages = vec![
        user_msg_with_id("completed-turn", "inspect source"),
        tool_msg_with_id("completed-tool", "read-source", &large),
        assistant_msg("inspection complete"),
        user_msg_with_id("fresh-turn", "now answer the user"),
    ];
    state.estimate_context_chars = state.messages.iter().map(estimate_msg_chars).sum();

    let history_end = state.messages.len();
    let outcome = run_layer0_cleanup(
        &mut state,
        &ContextConfig::default(),
        dir.path(),
        "sess_a2b",
        history_end,
    );

    assert_eq!(outcome.persisted.len(), 1);
    assert!(
        state.messages[1]
            .text_content()
            .is_some_and(|text| text.starts_with("[Tool result persisted:")),
        "a fresh request must not hide the most recent completed tool result from Layer 0"
    );
}

#[test]
fn run_layer0_cleanup_never_mutates_messages_after_history_end() {
    let dir = tempfile::tempdir().unwrap();
    let historical_tool = "h".repeat(20_000);
    let tail_tool = "t".repeat(60_000);
    let mut state = make_state(0, 100_000, 25_000);
    state.messages = vec![
        user_msg_with_id("historical-user", "completed turn"),
        tool_msg_with_id("historical-tool", "historical-call", &historical_tool),
        user_msg_with_id("tail-user", "active turn"),
        tool_msg_with_id("tail-tool", "tail-call", &tail_tool),
    ];
    state.estimate_context_chars = state.messages.iter().map(estimate_msg_chars).sum();
    let tail_before = state.messages[2..].to_vec();
    let config = ContextConfig {
        keep_recent_turns: 0,
        ..Default::default()
    };

    let outcome = run_layer0_cleanup(&mut state, &config, dir.path(), "sess_history_end", 2);

    assert_eq!(
        state.messages[1].text_content(),
        Some(TOOL_RESULT_PLACEHOLDER),
        "the compactable historical result should still be eligible for Layer 0"
    );
    assert_eq!(
        serde_json::to_vec(&state.messages[2..]).unwrap(),
        serde_json::to_vec(&tail_before).unwrap(),
        "Layer 0 must not persist or placeholder any active-turn message"
    );
    assert!(outcome.persisted.is_empty());
    assert!(
        !dir.path()
            .join("tool-results")
            .join("sess_history_end")
            .exists(),
        "the tail's 60K result must not be persisted by the history-only cleanup"
    );
}

#[test]
fn run_layer0_cleanup_mixed_sizes() {
    let dir = tempfile::tempdir().unwrap();
    let big = "x".repeat(60_000);
    let medium = "y".repeat(15_000);
    let small = "z".repeat(5_000);

    // 8 turns total so that first 3 are in compactable zone (8 - M_PROTECTED=5 = 3)
    // L0 persist only scans the LAST turn, so put big result in the last turn.
    // L1 compact scans the compactable zone (turns before protected).
    let mut msgs = vec![
        user_msg_with_id("u0", "q0"),
        tool_msg_with_id("t0", "tc0", &medium), // 15K in compactable zone
        assistant_msg("a0"),
        user_msg_with_id("u1", "q1"),
        tool_msg_with_id("t1", "tc1", &small), // 5K in compactable zone
        assistant_msg("a1"),
    ];
    for i in 2..7 {
        msgs.push(user_msg_with_id(&format!("u{i}"), &format!("q{i}")));
        msgs.push(tool_msg_with_id(&format!("t{i}"), &format!("tc{i}"), "ok"));
        msgs.push(assistant_msg(&format!("a{i}")));
    }
    // Last turn (turn 7): big tool result for L0 persist
    msgs.push(user_msg_with_id("u7", "q7"));
    msgs.push(tool_msg_with_id("t7", "tc7", &big));
    msgs.push(assistant_msg("a7"));

    let total: usize = msgs.iter().map(estimate_msg_chars).sum();
    let mut state = make_state(total, total * 2, total / 2);
    state.messages = msgs;

    let history_end = state.messages.len();
    let outcome = run_layer0_cleanup(
        &mut state,
        &ContextConfig::default(),
        dir.path(),
        "sess_a3",
        history_end,
    );

    // Turn 0 medium (15K > 10K placeholder threshold, in compactable zone): L1 placeholder
    let t0_tool = &state.messages[1];
    assert_eq!(
        t0_tool.text_content().unwrap_or(""),
        TOOL_RESULT_PLACEHOLDER,
        "15K tool result in compactable zone should be placeholder (L1)"
    );

    // Turn 1 small (5K < 10K): unchanged
    let t1_tool = &state.messages[4];
    assert_eq!(
        t1_tool.text_content().unwrap_or("").len(),
        5_000,
        "5K tool result should be unchanged"
    );

    // Last turn big (60K > 50K): L0 persisted (but in protected zone, so not L1 placeholder)
    let last_tool = state
        .messages
        .iter()
        .rev()
        .find(|m| m.role == ChatMessageRole::Tool)
        .unwrap();
    assert!(
        last_tool
            .text_content()
            .unwrap_or("")
            .starts_with("[Tool result persisted:"),
        "60K tool result in last turn should be L0 persisted"
    );

    assert!(
        !outcome.persisted.is_empty(),
        "at least the 60K result should be persisted to disk"
    );
}

#[test]
fn run_layer0_cleanup_freed_values_consistent_with_estimate() {
    let dir = tempfile::tempdir().unwrap();
    let big = "x".repeat(60_000);
    let msgs = build_turns(8, &big);
    let total: usize = msgs.iter().map(estimate_msg_chars).sum();
    let mut state = make_state(total, total * 2, total / 2);
    state.messages = msgs;

    let before = state.estimate_context_chars;
    let history_end = state.messages.len();
    let outcome = run_layer0_cleanup(
        &mut state,
        &ContextConfig::default(),
        dir.path(),
        "sess_a4",
        history_end,
    );

    let reported_freed = outcome.persist_chars_freed + outcome.placeholder_chars_freed;
    assert!(reported_freed > 0, "should report freed chars");
    assert!(
        state.estimate_context_chars < before,
        "estimate should decrease"
    );
}
