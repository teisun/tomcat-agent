//! 单元测试：`preheat` 模块的 `snapshot_message_bounds_for_preheat` 纯函数。

use super::snapshot_message_bounds_for_preheat;
use crate::core::llm::{ChatMessage, ChatMessageContent, ChatMessageRole, MessageKind};

fn normal_msg(id: &str) -> ChatMessage {
    ChatMessage {
        role: ChatMessageRole::User,
        content: Some(ChatMessageContent::Text("text".into())),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        finish_reason: None,
        error_message: None,
        error_code: None,
        msg_id: Some(id.to_string()),
        kind: MessageKind::Normal,
        timestamp: Some("ts".into()),
    }
}

fn summary_msg(id: &str) -> ChatMessage {
    let mut m = ChatMessage::compaction_summary("prev");
    m.msg_id = Some(id.to_string());
    m
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
