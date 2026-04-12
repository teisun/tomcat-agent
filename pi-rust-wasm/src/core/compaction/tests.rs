use super::truncation::{floor_char_boundary, TOOL_RESULT_PLACEHOLDER};
use super::*;
use crate::core::agent_loop::AgentMessage;
use crate::core::compaction::preheat::Preheat;
use crate::core::session::manager::{
    build_context_from_state, compound_turn_id, CompactionResult, ContextState, TurnEntry,
};
use crate::core::session::transcript::{
    append_entry, read_header, write_header, BranchSummaryEntry, SessionHeader, TranscriptEntry,
};
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;

const TS: &str = "2026-04-04T12:00:00Z";

fn make_user_turn_with_span(
    start_id: &str,
    end_id: &str,
    messages: Vec<AgentMessage>,
) -> TurnEntry {
    let start_id = start_id.to_string();
    let end_id = end_id.to_string();
    let id = compound_turn_id(&start_id, &end_id);
    TurnEntry::UserTurn {
        id,
        start_id,
        end_id,
        messages,
        timestamp: TS.to_string(),
    }
}

/// 不关心 id 语义时的占位（单测 compaction 行为）。
fn make_user_turn(messages: Vec<AgentMessage>) -> TurnEntry {
    make_user_turn_with_span("m_legacy", "m_legacy", messages)
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
        session_obs: Default::default(),
        live: Default::default(),
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
    // 与 chat + agent_loop 一致：内容先经 on_message_appended，再登记 turn（避免双重计入）。
    state.on_message_appended(5);
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
    let (results, _) =
        layer0_persist_large_results(&mut state, &config, dir.path(), "test_session");
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
    state.user_turns_list = vec![make_user_turn(vec![AgentMessage::ToolResult {
        tool_call_id: "tc_read".into(),
        content: original.clone(),
        is_error: false,
    }])];
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

/// 回归：上一轮 `last_api_usage` 很大时，L3 仍应按**字符估算**与 turns 同步删 oldest，
/// 不得因 ratio 长期虚高而删空 `user_turns_list`（否则 `build_context_from_state` 为空 → API `messages: []`）。
#[test]
fn force_drop_oldest_respects_chars_not_stale_api_usage() {
    let t_big = make_user_turn(vec![AgentMessage::User {
        text: "a".repeat(30_000),
    }]);
    let t_small = make_user_turn(vec![AgentMessage::User {
        text: "b".repeat(15_000),
    }]);
    let mut state = make_state(45_000, 200_000, 20_000);
    state.user_turns_list = vec![t_big, t_small];
    state.update_api_usage(500_000, 0);
    force_drop_oldest_to_target(&mut state);
    assert_eq!(
        state.user_turns_list.len(),
        1,
        "should drop only oldest turn(s) until char-based ratio < 0.5, not drain all"
    );
    let flat = build_context_from_state(&state);
    assert!(
        !flat.is_empty(),
        "non-empty turns must rebuild non-empty context"
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
    let t0 = make_user_turn_with_span(
        "m0",
        "m0",
        vec![AgentMessage::User {
            text: "a".repeat(5000),
        }],
    );
    let t1 = make_user_turn_with_span(
        "m1",
        "m1",
        vec![AgentMessage::User {
            text: "b".repeat(3000),
        }],
    );
    let t2 = make_user_turn_with_span(
        "m2",
        "m2",
        vec![AgentMessage::User {
            text: "c".repeat(2000),
        }],
    );
    state.user_turns_list = vec![t0, t1, t2];
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

    assert_eq!(state.user_turns_list.len(), 2);
    assert!(
        matches!(&state.user_turns_list[0], TurnEntry::SummaryTurn { summary, .. } if summary == "short summary")
    );
    assert_eq!(state.user_turns_list[1].id(), compound_turn_id("m2", "m2"));
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
    state.user_turns_list = vec![make_user_turn(vec![AgentMessage::User {
        text: "x".into(),
    }])];

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
    let t1 = make_user_turn_with_span(
        "still_end",
        "still_end",
        vec![AgentMessage::User {
            text: "b".repeat(1000),
        }],
    );
    state.user_turns_list = vec![t1];
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
    assert_eq!(state.user_turns_list.len(), 1);
    assert!(
        matches!(&state.user_turns_list[0], TurnEntry::SummaryTurn { summary, id, .. } if summary == "merged" && id == &compound_turn_id("gone_start", "still_end"))
    );
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
    state.user_turns_list = vec![make_user_turn_with_span(
        "still_end",
        "still_end",
        vec![AgentMessage::User {
            text: "x".into(),
        }],
    )];
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
    state.user_turns_list = vec![make_user_turn(vec![AgentMessage::ToolResult {
        tool_call_id: "tc_cfg".into(),
        content: big_content,
        is_error: false,
    }])];

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
    state2.user_turns_list = vec![make_user_turn(vec![AgentMessage::ToolResult {
        tool_call_id: "tc_cfg2".into(),
        content: "y".repeat(60_000),
        is_error: false,
    }])];
    let (results2, _) = layer0_persist_large_results(&mut state2, &config2, dir.path(), "test");
    assert_eq!(results2.len(), 1, "60K > 50K threshold should persist");
}
