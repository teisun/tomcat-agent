//! 单元测试：`preheat` 模块的 `snapshot_message_bounds_for_preheat` 纯函数。

use super::{messages_to_text, snapshot_message_bounds_for_preheat, summary_projection};
use crate::core::llm::{ChatMessage, ChatMessageContent, ChatMessageRole, MessageKind};

#[test]
fn summary_projection_filters_all_tool_results_only_when_oversized() {
    let messages = [
        ChatMessage::user("keep [ToolResult] in real user text"),
        ChatMessage::tool("short", "SHORT_TOOL"),
        ChatMessage::assistant("keep assistant text"),
        ChatMessage::tool("long", &"LONG_TOOL".repeat(500)),
    ];
    let full = messages_to_text(&messages);
    let prompt = "first summary template including previous summary text";
    let exact = (full.len() + prompt.len()) / 4;
    assert_eq!(summary_projection(&messages, prompt, Some(exact)), full);
    assert!(full.contains("SHORT_TOOL") && full.contains("LONG_TOOL"));
    for budget in [exact - 1, 1] {
        assert_eq!(
            summary_projection(&messages, prompt, Some(budget)),
            "[User] keep [ToolResult] in real user text\n[Assistant] keep assistant text\n"
        );
    }
    assert_eq!(
        messages_to_text(&messages),
        full,
        "main history is not modified"
    );
}

fn normal_msg(id: &str) -> ChatMessage {
    ChatMessage {
        role: ChatMessageRole::User,
        content: Some(ChatMessageContent::Text("text".into())),
        annotations: None,
        name: None,
        tool_calls: None,
        tool_call_id: None,
        finish_reason: None,
        error_message: None,
        error_code: None,
        thinking_text: None,
        reasoning_continuation: None,
        continuity: None,
        usage: None,
        summary_title: None,
        tool_display: None,
        is_error: None,
        msg_id: Some(id.to_string()),
        kind: MessageKind::Normal,
        timestamp: Some("ts".into()),
    }
}

fn summary_msg(id: &str) -> ChatMessage {
    ChatMessage::compaction_summary("prev", id)
}

#[test]
fn skips_leading_summary_turn() {
    let messages = vec![
        summary_msg("batch_S::batch_E"),
        normal_msg("m0"),
        normal_msg("m1"),
        normal_msg("m2"),
        normal_msg("m3"),
    ];
    let (s, e) = snapshot_message_bounds_for_preheat(&messages).unwrap();
    assert_eq!(s, "m0");
    assert_eq!(e, "m3");
}

#[test]
fn skips_trailing_summary_turn() {
    let messages = vec![normal_msg("a"), normal_msg("b"), summary_msg("x::y")];
    let (s, e) = snapshot_message_bounds_for_preheat(&messages).unwrap();
    assert_eq!(s, "a");
    assert_eq!(e, "b");
}

#[test]
fn none_when_no_normal_message() {
    let messages = vec![summary_msg("only::summary")];
    assert!(snapshot_message_bounds_for_preheat(&messages).is_none());
}
