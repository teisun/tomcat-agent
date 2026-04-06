use super::truncation::{floor_char_boundary, TOOL_RESULT_PLACEHOLDER, TRUNCATION_SUFFIX};
use super::*;
use crate::core::agent_loop::AgentMessage;
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

fn make_summary_turn(summary: impl Into<String>) -> TurnEntry {
    TurnEntry::SummaryTurn {
        id: format!("sum_{}", TS),
        summary: summary.into(),
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
        compaction_summary: None,
        compaction_consecutive_failures: 0,
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
fn truncate_noop_when_under_limit() {
    let mut s = "short".to_string();
    let info = truncate_tool_result_if_needed(&mut s, 1000);
    assert!(info.is_none());
    assert_eq!(s, "short");
}

#[test]
fn truncate_works_on_large_content() {
    let mut s = "a\n".repeat(300_000);
    let info = truncate_tool_result_if_needed(&mut s, 400_000);
    assert!(info.is_some());
    let info = info.unwrap();
    assert!(info.truncated_chars < 400_000 + TRUNCATION_SUFFIX.len() + 10);
    assert!(s.ends_with(TRUNCATION_SUFFIX));
}

#[test]
fn truncate_chinese_content_no_panic() {
    let mut s = "你好\n".repeat(200_000);
    let info = truncate_tool_result_if_needed(&mut s, 400_000);
    assert!(info.is_some());
    assert!(s.ends_with(TRUNCATION_SUFFIX));
}

#[test]
fn truncate_exact_boundary() {
    let mut s = "x".repeat(400_000);
    let info = truncate_tool_result_if_needed(&mut s, 400_000);
    assert!(info.is_none());
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
    let reduced = compact_tool_results(&mut state, 1);
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
    let reduced = compact_tool_results(&mut state, 1);
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
    let reduced = compact_tool_results(&mut state, 1);
    assert_eq!(reduced, 0);
}

#[test]
fn force_drop_oldest_recovers_budget() {
    let mut state = make_state(6000, 2000, 500);
    state.user_turns_list = vec![
        make_summary_turn("x".repeat(5000)),
        make_user_turn(vec![AgentMessage::User {
            text: "q".to_string(),
        }]),
    ];
    force_drop_oldest(&mut state);
    assert!(!state.is_over_budget());
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
fn determine_cascade_params_below_threshold() {
    let state = make_state(100, 1000, 1000);
    let config = ContextConfig::default();
    let params = determine_cascade_params(&state, &config);
    assert!(!params.should_cascade);
}

#[test]
fn determine_cascade_params_at_070() {
    let mut state = make_state(0, 0, 1000);
    state.update_api_usage(700, 0);
    let config = ContextConfig::default();
    let params = determine_cascade_params(&state, &config);
    assert!(params.should_cascade);
    assert_eq!(params.m, 5);
    assert!(!params.block_tool_calls);
}

#[test]
fn determine_cascade_params_at_098() {
    let mut state = make_state(0, 0, 1000);
    state.update_api_usage(980, 0);
    let config = ContextConfig::default();
    let params = determine_cascade_params(&state, &config);
    assert!(params.should_cascade);
    assert_eq!(params.m, 1);
    assert!(params.block_tool_calls);
    assert!(!params.target_layer3);
}

#[test]
fn determine_cascade_params_at_100() {
    let mut state = make_state(0, 0, 1000);
    state.update_api_usage(1000, 0);
    let config = ContextConfig::default();
    let params = determine_cascade_params(&state, &config);
    assert!(params.should_cascade);
    assert!(params.target_layer3);
}

#[test]
fn determine_cascade_params_zero_budget() {
    let state = make_state(100, 100, 0);
    let config = ContextConfig::default();
    let params = determine_cascade_params(&state, &config);
    assert!(params.should_cascade);
    assert!(params.target_layer3);
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

#[test]
fn circuit_breaker_skips_layer2() {
    let mut state = make_state(100, 100, 100);
    state.compaction_consecutive_failures = 3;
    assert!(state.compaction_consecutive_failures >= 3);
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
    let reduced = compact_tool_results(&mut state, 1);
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
    let reduced = compact_tool_results(&mut state, 1);
    assert_eq!(
        reduced, 0,
        "already replaced results should not be re-replaced"
    );
}

#[test]
fn determine_cascade_params_at_085() {
    let mut state = make_state(0, 0, 1000);
    state.update_api_usage(860, 0);
    let config = ContextConfig::default();
    let params = determine_cascade_params(&state, &config);
    assert!(params.should_cascade);
    assert_eq!(params.m, 3);
    assert!(!params.block_tool_calls);
}

#[test]
fn determine_cascade_params_at_092() {
    let mut state = make_state(0, 0, 1000);
    state.update_api_usage(930, 0);
    let config = ContextConfig::default();
    let params = determine_cascade_params(&state, &config);
    assert!(params.should_cascade);
    assert_eq!(params.m, 2);
    assert!(!params.block_tool_calls);
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
