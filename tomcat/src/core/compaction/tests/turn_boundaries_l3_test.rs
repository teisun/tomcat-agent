use super::super::{compact_tool_results, force_drop_oldest_after_confirmed_overflow};
use super::mocks::*;
use crate::core::compaction::test_support::{assert_tail_invariant, durable_ids};
use crate::core::llm::MessageKind;
use crate::core::session::manager::estimate_msg_chars;
use crate::infra::config::ContextConfig;

#[test]
fn l1_turn_boundary_with_steering_messages() {
    // 8 turns with steering injected — steering has role=User but kind=Steering,
    // should NOT count as a turn boundary for L1 protected zone calculation.
    let big = "x".repeat(25_000);
    let mut msgs = Vec::new();

    // Turns 0-2: user + big tool + assistant (compactable zone if total >= M_PROTECTED+1)
    for i in 0..3 {
        msgs.push(user_msg_with_id(&format!("u{i}"), &format!("q{i}")));
        msgs.push(tool_msg_with_id(&format!("t{i}"), &format!("tc{i}"), &big));
        msgs.push(assistant_msg(&format!("a{i}")));
    }

    // Inject a steering message between turns — role=User, kind=Steering
    msgs.push(steering_msg("internal steering"));

    // Turns 3-7: user + small tool + assistant (protected zone)
    for i in 3..8 {
        msgs.push(user_msg_with_id(&format!("u{i}"), &format!("q{i}")));
        msgs.push(tool_msg_with_id(&format!("t{i}"), &format!("tc{i}"), "ok"));
        msgs.push(assistant_msg(&format!("a{i}")));
    }

    let total: usize = msgs.iter().map(estimate_msg_chars).sum();
    let mut state = make_state(total, total, total / 4);
    let mut messages = msgs;

    let config = ContextConfig {
        keep_recent_turns: 5,
        ..Default::default()
    };
    let history_end = messages.len();
    let reduced = compact_tool_results(&mut state, &mut messages, &config, history_end).chars_freed;

    // Turns 0-2 are in compactable zone (3 real turns before the protected 5)
    // Their tool results should be replaced
    assert!(reduced > 0, "should compact tool results in turns 0-2");

    // The steering message itself should not have been touched
    let steering = messages.iter().find(|m| m.kind == MessageKind::Steering);
    assert!(steering.is_some(), "steering message should still exist");
    assert_eq!(
        steering.unwrap().text_content(),
        Some("internal steering"),
        "steering content should be unchanged"
    );
}

#[test]
fn l1_keep_recent_turns_reads_config_value() {
    let big = "x".repeat(25_000);
    let mut msgs = Vec::new();
    for i in 0..4 {
        msgs.push(user_msg_with_id(&format!("u{i}"), &format!("q{i}")));
        msgs.push(tool_msg_with_id(&format!("t{i}"), &format!("tc{i}"), &big));
        msgs.push(assistant_msg(&format!("a{i}")));
    }

    let total: usize = msgs.iter().map(estimate_msg_chars).sum();
    let mut state = make_state(total, total, total / 4);
    let mut messages = msgs;

    let config = ContextConfig {
        keep_recent_turns: 2,
        ..Default::default()
    };
    let history_end = messages.len();
    let reduced = compact_tool_results(&mut state, &mut messages, &config, history_end).chars_freed;
    assert!(
        reduced > 0,
        "older turns should become compactable once keep_recent_turns shrinks"
    );

    let tool_texts: Vec<_> = messages
        .iter()
        .filter(|msg| msg.role == crate::core::llm::ChatMessageRole::Tool)
        .map(|msg| msg.text_content())
        .collect();
    assert!(tool_texts[0] == Some(crate::core::compaction::TOOL_RESULT_PLACEHOLDER));
    assert!(tool_texts[1] == Some(crate::core::compaction::TOOL_RESULT_PLACEHOLDER));
    assert!(tool_texts[2] == Some(&big));
    assert!(tool_texts[3] == Some(&big));
}

#[test]
fn l3_drop_oldest_with_compaction_summary_as_first() {
    // Use a small budget so ratio is high enough to trigger dropping.
    // estimate_context_chars / 4 = estimated tokens; ratio = tokens / budget_tokens
    // Need ratio >= 0.50, so tokens >= budget_tokens * 0.50
    let summary_text = "previous summary ".repeat(200);
    let asst_text = "assistant response ".repeat(200);
    let tool_text = "tool output data ".repeat(200);
    let user_text = "new question text ".repeat(100);

    let mut state = make_state(0, 100_000, 1_000);
    let mut messages = vec![
        summary_msg(&summary_text), // turn 0 start (CompactionSummary)
        assistant_msg(&asst_text),  // turn 0 body
        tool_msg_with_id("t0", "tc0", &tool_text),
        user_msg_with_id("u1", &user_text), // turn 1 start
        assistant_msg("new answer"),        // turn 1 body
    ];
    let total: usize = messages.iter().map(estimate_msg_chars).sum();
    state.estimate_context_chars = total;

    // Verify ratio is high enough before dropping
    assert!(
        state.usage_ratio() >= 0.50,
        "ratio should be >= 0.50 to trigger L3, got {}",
        state.usage_ratio()
    );

    let mut history_end = messages.len();
    let (turns_removed, chars_removed) =
        force_drop_oldest_after_confirmed_overflow(&mut state, &mut messages, &mut history_end);

    assert!(turns_removed >= 1, "should drop at least one turn");
    assert!(chars_removed > 0, "should free some chars");
    assert!(!messages.is_empty(), "should not drain all messages");

    // CompactionSummary turn was the oldest — it should have been dropped
    let has_summary = messages
        .iter()
        .any(|m| m.kind == MessageKind::CompactionSummary);
    assert!(
        !has_summary,
        "CompactionSummary (oldest turn) should have been dropped"
    );
}

#[test]
fn l3_can_drop_entire_history_when_tail_exists() {
    let history = user_msg_with_id("history", &"h".repeat(8_000));
    let tail = user_msg_with_id("tail", "current request must survive");
    let mut messages = vec![history, tail.clone()];
    let mut state = make_state(
        messages.iter().map(estimate_msg_chars).sum(),
        100_000,
        1_000,
    );
    let mut turn_start = 1;
    let before_tail_ids = durable_ids(&messages, turn_start);

    let (turns_removed, chars_removed) =
        force_drop_oldest_after_confirmed_overflow(&mut state, &mut messages, &mut turn_start);

    assert_eq!(turns_removed, 1);
    assert!(chars_removed > 0);
    assert_eq!(turn_start, 0);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].msg_id, tail.msg_id);
    assert_tail_invariant(&before_tail_ids, &messages, turn_start);
}

#[test]
fn apply_boundary_with_msg_id_matching() {
    let mut state = make_state(0, 100_000, 25_000);
    state.messages = vec![
        user_msg_with_id("m1", "first"),
        user_msg_with_id("m2", "second"),
        user_msg_with_id("m3", "third"),
        user_msg_with_id("m4", "fourth"),
        user_msg_with_id("m5", "fifth"),
    ];
    let mut messages = std::mem::take(&mut state.messages);
    state.estimate_context_chars = messages.iter().map(estimate_msg_chars).sum();

    let result = crate::core::session::manager::CompactionResult {
        summary_text: "summary of m1-m3".into(),
        covered_start_id: "m1".into(),
        covered_end_id: "m3".into(),
        covered_count: 3,
        transcript_compaction_entry_id: Some("cid_m1_m3".to_string()),
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        preheat_elapsed_ms: 0,
    };
    let mut turn_start = messages.len();
    state
        .apply_boundary(&mut messages, result, &mut turn_start)
        .unwrap();

    assert_eq!(messages.len(), 3, "summary + m4 + m5");
    assert_eq!(
        messages[0].kind,
        crate::core::llm::MessageKind::CompactionSummary
    );
    assert_eq!(messages[0].text_content(), Some("summary of m1-m3"));
    assert_eq!(messages[1].msg_id.as_deref(), Some("m4"));
    assert_eq!(messages[2].msg_id.as_deref(), Some("m5"));
}
