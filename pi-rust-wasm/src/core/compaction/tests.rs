use super::truncation::{floor_char_boundary, TOOL_RESULT_PLACEHOLDER};
use super::*;
use crate::core::agent_loop::AgentMessage;
use crate::core::compaction::preheat::Preheat;
use crate::core::session::manager::{ContextState, TurnEntry};
use crate::infra::config::ContextConfig;

const TS: &str = "2026-04-04T12:00:00Z";

fn make_user_turn(messages: Vec<AgentMessage>) -> TurnEntry {
    TurnEntry::UserTurn {
        id: format!("test_{}", TS),
        messages,
        timestamp: TS.to_string(),
    }
}

fn make_state(chars: usize, budget_chars: usize, budget_tokens: usize) -> ContextState {
    ContextState {
        user_turns_list: vec![],
        estimate_context_chars: chars,
        context_budget_chars: budget_chars,
        context_budget_tokens: budget_tokens,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        preheat: Preheat::new(),
    }
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
    state.user_turns_list = vec![
        make_user_turn(vec![
            AgentMessage::User {
                text: "q".to_string(),
            },
            AgentMessage::ToolResult {
                tool_call_id: "c1".into(),
                content: "x".repeat(25_000),
                is_error: false,
            },
        ]),
        make_user_turn(vec![AgentMessage::User {
            text: "q2".to_string(),
        }]),
    ];
    let reduced = compact_tool_results(&mut state, &ContextConfig::default(), 1);
    assert!(reduced > 0);
}

#[test]
fn compact_tool_results_protects_recent() {
    let tool_content = "x".repeat(25_000);
    let mut state = make_state(25_000, 5_000, 1_250);
    state.user_turns_list = vec![make_user_turn(vec![AgentMessage::ToolResult {
        tool_call_id: "c1".into(),
        content: tool_content.clone(),
        is_error: false,
    }])];
    let reduced = compact_tool_results(&mut state, &ContextConfig::default(), 1);
    assert_eq!(reduced, 0);
}

#[test]
fn compact_tool_results_skips_small() {
    let mut state = make_state(5_000, 3_000, 750);
    state.user_turns_list = vec![
        make_user_turn(vec![AgentMessage::ToolResult {
            tool_call_id: "c1".into(),
            content: "x".repeat(1_000),
            is_error: false,
        }]),
        make_user_turn(vec![AgentMessage::User {
            text: "q".to_string(),
        }]),
    ];
    let reduced = compact_tool_results(&mut state, &ContextConfig::default(), 1);
    assert_eq!(reduced, 0);
}

#[test]
fn force_drop_oldest_to_target_below_half() {
    let mut state = make_state(4000, 4000, 1000);
    state.user_turns_list = vec![
        make_user_turn(vec![AgentMessage::User {
            text: "x".repeat(2000),
        }]),
        make_user_turn(vec![AgentMessage::User {
            text: "y".repeat(1000),
        }]),
        make_user_turn(vec![AgentMessage::User {
            text: "z".repeat(500),
        }]),
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
fn context_state_on_new_user_turn() {
    let mut state = make_state(0, 1000, 250);
    let turn = make_user_turn(vec![AgentMessage::User {
        text: "hello".to_string(),
    }]);
    state.on_new_user_turn(turn);
    assert_eq!(state.user_turns_list.len(), 1);
    assert_eq!(state.estimate_context_chars, 5);
}

#[test]
fn layer0_persist_creates_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = make_state(60_000, 100_000, 25_000);
    let big_content = "x".repeat(60_000);
    state.user_turns_list = vec![make_user_turn(vec![AgentMessage::ToolResult {
        tool_call_id: "tc_1".into(),
        content: big_content,
        is_error: false,
    }])];
    let config = ContextConfig::default();
    let results = layer0_persist_large_results(&mut state, &config, dir.path(), "test_session");
    assert_eq!(results.len(), 1);
    assert!(std::path::Path::new(&results[0].persisted_path).exists());
    assert!(state.estimate_context_chars < 60_000);
    if let TurnEntry::UserTurn { messages, .. } = &state.user_turns_list[0] {
        if let AgentMessage::ToolResult { content, .. } = &messages[0] {
            assert!(content.starts_with("[Tool result persisted:"));
        }
    }
}

#[test]
fn layer0_persist_skips_small() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = make_state(1_000, 100_000, 25_000);
    state.user_turns_list = vec![make_user_turn(vec![AgentMessage::ToolResult {
        tool_call_id: "tc_2".into(),
        content: "small".to_string(),
        is_error: false,
    }])];
    let config = ContextConfig::default();
    let results = layer0_persist_large_results(&mut state, &config, dir.path(), "test_session");
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
    state.user_turns_list = vec![
        make_user_turn(vec![AgentMessage::ToolResult {
            tool_call_id: "c1".into(),
            content: "[Tool result persisted: /tmp/foo.txt (50000 chars)]\nPreview: ..."
                .to_string(),
            is_error: false,
        }]),
        make_user_turn(vec![AgentMessage::User {
            text: "q".to_string(),
        }]),
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
    state.user_turns_list = vec![
        make_user_turn(vec![AgentMessage::ToolResult {
            tool_call_id: "c1".into(),
            content: TOOL_RESULT_PLACEHOLDER.to_string(),
            is_error: false,
        }]),
        make_user_turn(vec![AgentMessage::User {
            text: "q".to_string(),
        }]),
    ];
    let reduced = compact_tool_results(&mut state, &ContextConfig::default(), 1);
    assert_eq!(
        reduced, 0,
        "already replaced results should not be re-replaced"
    );
}

#[test]
fn compact_tool_results_respects_placeholder_threshold_from_config() {
    let mut state = make_state(30_000, 5_000, 1_250);
    let big = "x".repeat(25_000);
    state.user_turns_list = vec![
        make_user_turn(vec![AgentMessage::ToolResult {
            tool_call_id: "c1".into(),
            content: big.clone(),
            is_error: false,
        }]),
        make_user_turn(vec![AgentMessage::User {
            text: "q".to_string(),
        }]),
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
    if let TurnEntry::UserTurn { messages, .. } = &state.user_turns_list[0] {
        if let AgentMessage::ToolResult { content, .. } = &messages[0] {
            assert_eq!(content.len(), 25_000);
        } else {
            panic!("expected ToolResult");
        }
    } else {
        panic!("expected UserTurn");
    }
}

#[test]
fn layer0_persist_skips_below_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = make_state(200_000, 500_000, 125_000);
    let medium = "x".repeat(20_000);
    state.user_turns_list = vec![make_user_turn(vec![AgentMessage::ToolResult {
        tool_call_id: "tc_a".into(),
        content: medium,
        is_error: false,
    }])];
    let config = ContextConfig::default();
    let results = layer0_persist_large_results(&mut state, &config, dir.path(), "test_session");
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
    state.user_turns_list = vec![make_user_turn(vec![AgentMessage::ToolResult {
        tool_call_id: "tc_read".into(),
        content: original.clone(),
        is_error: false,
    }])];
    let config = ContextConfig::default();
    let results = layer0_persist_large_results(&mut state, &config, dir.path(), "sess1");
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
    state.user_turns_list = vec![
        make_user_turn(vec![AgentMessage::User {
            text: "x".repeat(3000),
        }]),
        make_user_turn(vec![AgentMessage::User {
            text: "y".repeat(500),
        }]),
    ];
    force_drop_oldest_to_target(&mut state);
    assert!(
        state.last_api_usage.is_none(),
        "usage should be invalidated after force drop"
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
    let t0 = TurnEntry::UserTurn {
        id: "t0".into(),
        messages: vec![AgentMessage::User { text: "a".repeat(5000) }],
        timestamp: "2026-01-01T00:00:00Z".into(),
    };
    let t1 = TurnEntry::UserTurn {
        id: "t1".into(),
        messages: vec![AgentMessage::User { text: "b".repeat(3000) }],
        timestamp: "2026-01-01T00:01:00Z".into(),
    };
    let t2 = TurnEntry::UserTurn {
        id: "t2".into(),
        messages: vec![AgentMessage::User { text: "c".repeat(2000) }],
        timestamp: "2026-01-01T00:02:00Z".into(),
    };
    state.user_turns_list = vec![t0, t1, t2];
    state.estimate_context_chars = 10_000;

    let result = crate::core::session::manager::CompactionResult {
        summary_text: "short summary".into(),
        covered_start_id: "t0".into(),
        covered_end_id: "t1".into(),
        covered_count: 2,
    };
    let old_ratio = state.usage_ratio();
    state.apply_boundary(result).unwrap();

    assert_eq!(state.user_turns_list.len(), 2);
    assert!(matches!(&state.user_turns_list[0], TurnEntry::SummaryTurn { summary, .. } if summary == "short summary"));
    assert_eq!(state.user_turns_list[1].id(), "t2");
    assert!(state.last_api_usage.is_none());
    let new_ratio = state.usage_ratio();
    assert!(new_ratio < old_ratio, "ratio should decrease after boundary");
}

#[test]
fn apply_boundary_not_found_returns_err() {
    let mut state = make_state(1000, 10_000, 2_500);
    state.user_turns_list = vec![make_user_turn(vec![AgentMessage::User { text: "x".into() }])];

    let result = crate::core::session::manager::CompactionResult {
        summary_text: "summary".into(),
        covered_start_id: "nonexistent".into(),
        covered_end_id: "also_nonexistent".into(),
        covered_count: 1,
    };
    let res = state.apply_boundary(result);
    assert!(res.is_err());
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
fn layer0_threshold_from_config() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = make_state(60_000, 100_000, 25_000);
    let big_content = "x".repeat(60_000);
    state.user_turns_list = vec![make_user_turn(vec![AgentMessage::ToolResult {
        tool_call_id: "tc_cfg".into(),
        content: big_content,
        is_error: false,
    }])];

    let config = ContextConfig {
        layer0_single_result_max_chars: 100_000,
        ..Default::default()
    };
    let results = layer0_persist_large_results(&mut state, &config, dir.path(), "test");
    assert!(results.is_empty(), "60K < 100K threshold should NOT persist");

    let config2 = ContextConfig {
        layer0_single_result_max_chars: 50_000,
        ..Default::default()
    };
    let mut state2 = make_state(60_000, 100_000, 25_000);
    state2.user_turns_list = vec![make_user_turn(vec![AgentMessage::ToolResult {
        tool_call_id: "tc_cfg2".into(),
        content: "y".repeat(60_000),
        is_error: false,
    }])];
    let results2 = layer0_persist_large_results(&mut state2, &config2, dir.path(), "test");
    assert_eq!(results2.len(), 1, "60K > 50K threshold should persist");
}
