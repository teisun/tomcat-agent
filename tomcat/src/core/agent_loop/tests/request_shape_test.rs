//! Request-shape validation at the agent-loop/provider boundary.

use crate::core::agent_loop::reasoning_loop::validate_request_shape;
use crate::core::llm::ChatMessage;

#[test]
fn orphan_tool_result_is_dropped_before_request() {
    let mut messages = vec![
        ChatMessage::tool("call_missing", "stale result"),
        ChatMessage::user("continue"),
    ];

    validate_request_shape(&mut messages).expect("orphaned result is safely removable");

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text_content(), Some("continue"));
}

#[test]
fn dangling_tool_call_rejects_request() {
    let mut declaration = ChatMessage::assistant("");
    declaration.tool_calls = Some(vec![serde_json::json!({
        "id": "call_unfinished",
        "type": "function",
        "function": {"name": "read", "arguments": "{}"},
    })]);
    let mut messages = vec![ChatMessage::user("inspect"), declaration];

    let error = validate_request_shape(&mut messages).expect_err("unpaired calls are unsafe");

    assert!(matches!(
        error,
        crate::core::agent_loop::LoopError::Fatal(error)
            if error.to_string().contains("unpaired tool calls")
    ));
}

#[test]
fn assistant_tailed_request_is_rejected() {
    let mut messages = vec![
        ChatMessage::user("inspect"),
        ChatMessage::assistant("partial"),
    ];

    let error = validate_request_shape(&mut messages).expect_err("assistant tail is invalid");

    assert!(matches!(
        error,
        crate::core::agent_loop::LoopError::Fatal(error)
            if error.to_string().contains("tail is not a user input or completed tool result")
    ));
}
