use super::super::truncation::TOOL_RESULT_PLACEHOLDER;
use super::super::{
    compact_tool_results, force_drop_oldest_after_confirmed_overflow, layer0_persist_large_results,
};
use super::mocks::*;
use crate::core::llm::ChatMessageRole;
use crate::infra::config::ContextConfig;
use crate::infra::error::is_context_overflow_text;

#[test]
fn layer0_persist_skips_small() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = make_state(1_000, 100_000, 25_000);
    let mut messages = vec![user_msg("q"), tool_msg("tc_2", "small")];
    let config = ContextConfig::default();
    let history_end = messages.len();
    let (results, _) = layer0_persist_large_results(
        &mut state,
        &mut messages,
        &config,
        dir.path(),
        "test_session",
        history_end,
    );
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
fn anthropic_total_input_usage_produces_the_actual_context_ratio() {
    let mut state = make_state(0, 0, 872_000);
    state.update_api_usage(209_600, 1_257);

    assert!((state.usage_ratio() - 0.2418).abs() < 0.0001);
}

#[test]
fn assistant_append_is_not_counted_twice_after_usage() {
    let mut state = make_state(1_000, 10_000, 2_500);
    state.update_api_usage(500, 100);
    state.on_assistant_message_appended(400);

    assert_eq!(state.estimate_context_chars, 1_400);
    assert_eq!(
        state.post_usage_appended_chars, 0,
        "provider completion_tokens already account for the assistant response"
    );
    assert_eq!(state.estimated_token_count(), 600);

    state.invalidate_api_usage();
    assert_eq!(
        state.estimated_token_count(),
        350,
        "the fallback estimate must retain the assistant response"
    );
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
    let mut messages = vec![
        user_msg("q"),
        tool_msg(
            "c1",
            "[Tool result persisted: /tmp/foo.txt (50000 chars)]\nPreview: ...",
        ),
        user_msg("q2"),
    ];
    let config = ContextConfig {
        keep_recent_turns: 1,
        ..Default::default()
    };
    let history_end = messages.len();
    let reduced = compact_tool_results(&mut state, &mut messages, &config, history_end).chars_freed;
    assert_eq!(
        reduced, 0,
        "already persisted results should not be replaced"
    );
}

#[test]
fn compact_tool_results_skips_placeholder() {
    let mut state = make_state(30_000, 5_000, 1_250);
    let mut messages = vec![
        user_msg("q"),
        tool_msg("c1", TOOL_RESULT_PLACEHOLDER),
        user_msg("q2"),
    ];
    let config = ContextConfig {
        keep_recent_turns: 1,
        ..Default::default()
    };
    let history_end = messages.len();
    let reduced = compact_tool_results(&mut state, &mut messages, &config, history_end).chars_freed;
    assert_eq!(
        reduced, 0,
        "already replaced results should not be re-replaced"
    );
}

#[test]
fn evicted_bash_result_keeps_log_path() {
    let payload = format!("Full log: /tmp/tomcat-build.log\n{}", "x".repeat(20_000));
    let mut state = make_state(30_000, 5_000, 1_250);
    let mut messages = vec![user_msg("q"), tool_msg("c1", &payload), user_msg("q2")];
    let config = ContextConfig {
        keep_recent_turns: 1,
        ..Default::default()
    };

    let history_end = messages.len();
    compact_tool_results(&mut state, &mut messages, &config, history_end);
    let text = messages[1].text_content().unwrap_or("");
    assert_eq!(
        text,
        "[Previous tool result replaced; full log at /tmp/tomcat-build.log]"
    );
}

#[test]
fn evicted_json_result_keeps_log_path_for_both_field_spellings() {
    for (field, path) in [
        ("logPath", "/tmp/tomcat-build-camel.log"),
        ("log_path", "/tmp/tomcat-build-snake.log"),
    ] {
        let payload = format!(
            r#"{{"{field}":"{path}","output":"{}"}}"#,
            "x".repeat(20_000)
        );
        let mut state = make_state(30_000, 5_000, 1_250);
        let mut messages = vec![user_msg("q"), tool_msg("c1", &payload), user_msg("q2")];
        let config = ContextConfig {
            keep_recent_turns: 1,
            ..Default::default()
        };

        let history_end = messages.len();
        compact_tool_results(&mut state, &mut messages, &config, history_end);
        let expected = format!("[Previous tool result replaced; full log at {path}]");
        assert_eq!(messages[1].text_content(), Some(expected.as_str()));
    }
}

#[test]
fn read_result_without_path_stays_bare_placeholder() {
    let mut state = make_state(30_000, 5_000, 1_250);
    let mut messages = vec![
        user_msg("q"),
        tool_msg("c1", &"source text ".repeat(2_000)),
        user_msg("q2"),
    ];
    let config = ContextConfig {
        keep_recent_turns: 1,
        ..Default::default()
    };

    let history_end = messages.len();
    compact_tool_results(&mut state, &mut messages, &config, history_end);
    assert_eq!(messages[1].text_content(), Some(TOOL_RESULT_PLACEHOLDER));
}

#[test]
fn placeholder_text_is_stable_across_rewrites() {
    let mut state = make_state(30_000, 5_000, 1_250);
    let mut messages = vec![
        user_msg("q"),
        tool_msg("c1", &"payload ".repeat(3_000)),
        user_msg("q2"),
    ];
    let config = ContextConfig {
        keep_recent_turns: 1,
        ..Default::default()
    };

    let history_end = messages.len();
    compact_tool_results(&mut state, &mut messages, &config, history_end);
    let once = messages[1].text_content().unwrap_or("").to_string();
    let history_end = messages.len();
    compact_tool_results(&mut state, &mut messages, &config, history_end);
    assert_eq!(messages[1].text_content(), Some(once.as_str()));
}

#[test]
fn compact_tool_results_respects_placeholder_threshold_from_config() {
    let big = "x".repeat(25_000);
    let mut state = make_state(30_000, 5_000, 1_250);
    let mut messages = vec![user_msg("q"), tool_msg("c1", &big), user_msg("q2")];
    let high_threshold = ContextConfig {
        keep_recent_turns: 1,
        layer0_placeholder_threshold_chars: 30_000,
        ..Default::default()
    };
    let history_end = messages.len();
    let reduced =
        compact_tool_results(&mut state, &mut messages, &high_threshold, history_end).chars_freed;
    assert_eq!(
        reduced, 0,
        "content below custom threshold should not be replaced"
    );
    let tool = messages
        .iter()
        .find(|m| m.role == ChatMessageRole::Tool)
        .unwrap();
    assert_eq!(tool.text_content().unwrap_or("").len(), 25_000);
}

#[test]
fn layer0_persist_skips_below_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = make_state(200_000, 500_000, 125_000);
    let medium = "x".repeat(20_000);
    let mut messages = vec![user_msg("q"), tool_msg("tc_a", &medium)];
    let config = ContextConfig::default();
    let history_end = messages.len();
    let (results, _) = layer0_persist_large_results(
        &mut state,
        &mut messages,
        &config,
        dir.path(),
        "test_session",
        history_end,
    );
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
    let mut messages = vec![user_msg("q"), tool_msg("tc_read", &original)];
    let config = ContextConfig::default();
    let history_end = messages.len();
    let (results, _) = layer0_persist_large_results(
        &mut state,
        &mut messages,
        &config,
        dir.path(),
        "sess1",
        history_end,
    );
    assert_eq!(results.len(), 1);
    let content = std::fs::read_to_string(&results[0].persisted_path).unwrap();
    assert_eq!(
        content, original,
        "persisted file should contain original content"
    );
    assert!(
        results[0]
            .persisted_path
            .contains(&format!("tool-results{}sess1", std::path::MAIN_SEPARATOR)),
        "tool result should be stored under runtime tool-results/session path"
    );
    assert!(
        !results[0].persisted_path.contains(&format!(
            "workspace{}sess1{}tool-results",
            std::path::MAIN_SEPARATOR,
            std::path::MAIN_SEPARATOR
        )),
        "legacy workspace/session/tool-results path must not be used"
    );
}

#[test]
fn confirmed_overflow_drop_invalidates_usage() {
    let mut state = make_state(4000, 4000, 1000);
    state.update_api_usage(900, 0);
    let mut messages = vec![user_msg(&"x".repeat(3000)), user_msg(&"y".repeat(500))];
    let mut history_end = messages.len();
    force_drop_oldest_after_confirmed_overflow(&mut state, &mut messages, &mut history_end);
    assert!(
        state.last_api_usage.is_none(),
        "usage should be invalidated after force drop"
    );
}

/// 回归：上一轮 `last_api_usage` 很大时，L3 仍应按**字符估算**与 messages 同步删 oldest，
/// 不得因 ratio 长期虚高而删空 `messages`（否则请求会变成 `messages: []`）。
#[test]
fn confirmed_overflow_drop_respects_chars_not_stale_api_usage() {
    let t_big = user_msg(&"a".repeat(30_000));
    let t_small = user_msg(&"b".repeat(15_000));
    let mut state = make_state(45_000, 200_000, 20_000);
    let mut messages = vec![t_big, t_small];
    state.update_api_usage(500_000, 0);
    let mut history_end = messages.len();
    force_drop_oldest_after_confirmed_overflow(&mut state, &mut messages, &mut history_end);
    assert_eq!(
        messages.len(),
        1,
        "should drop only oldest turn(s) until char-based ratio < 0.5, not drain all"
    );
    let flat = messages.clone();
    assert!(
        !flat.is_empty(),
        "non-empty messages must rebuild non-empty context"
    );
}

#[test]
fn is_context_overflow_comprehensive() {
    assert!(is_context_overflow_text("context length exceeded"));
    assert!(is_context_overflow_text("maximum context token limit"));
    assert!(is_context_overflow_text("Context limit exceeded"));
    assert!(!is_context_overflow_text("rate limit exceeded"));
    assert!(!is_context_overflow_text("authentication failed"));
}
