use super::mocks::*;
use crate::core::llm::ChatMessage;

// ===========================================================================
// Group E: messages_to_text 格式验证
// ===========================================================================

#[test]
fn messages_to_text_format_all_roles() {
    let previous = "之前的摘要";
    let user = "你好";
    let assistant = "你好啊";
    let msgs = vec![
        ChatMessage::compaction_summary(previous, "s0"),
        ChatMessage::user(user),
        ChatMessage::assistant(assistant),
        ChatMessage::tool("tc1", "ok"),
        ChatMessage::tool("tc2", &"x".repeat(250)),
    ];

    let text = messages_to_text(&msgs);

    assert!(
        text.contains(&format!("[Previous Summary]\n{previous}\n")),
        "should contain summary with correct format"
    );
    assert!(
        text.contains(&format!("[User] {user}\n")),
        "should contain user message"
    );
    assert!(
        text.contains(&format!("[Assistant] {assistant}\n")),
        "should contain assistant message"
    );
    assert!(
        text.contains("[ToolResult] ok\n"),
        "should contain short tool result"
    );
    // The 250-char tool result should be truncated to ~200
    let tool_result_lines: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("[ToolResult]"))
        .collect();
    assert_eq!(
        tool_result_lines.len(),
        2,
        "should have 2 tool result lines"
    );
    let long_tool_line = tool_result_lines[1];
    assert!(
        long_tool_line.len() < 250,
        "long tool result should be truncated, got {} chars",
        long_tool_line.len()
    );
}
