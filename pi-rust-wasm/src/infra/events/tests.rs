use super::*;

#[test]
fn agent_event_serialize_type_snake_case() {
    let e = AgentEvent::ExtensionError {
        extension_id: Some("ext-1".to_string()),
        event: wire::WIRE_TOOL_CALL.to_string(),
        error: "test".to_string(),
    };
    let j = serde_json::to_string(&e).unwrap();
    assert!(j.contains(wire::WIRE_EXTENSION_ERROR));
    assert!(j.contains("extensionId"));
}

#[test]
fn agent_event_tool_execution_uses_pi_mono_wire_names() {
    let start = AgentEvent::ToolExecutionStart {
        tool_call_id: "c1".into(),
        tool_name: "read_file".into(),
        args: serde_json::json!({}),
    };
    let end = AgentEvent::ToolExecutionEnd {
        tool_call_id: "c1".into(),
        tool_name: "read_file".into(),
        result: ToolOutput(serde_json::json!({})),
        is_error: false,
    };
    assert_eq!(
        serde_json::to_value(&start).unwrap()["type"]
            .as_str()
            .unwrap(),
        wire::WIRE_TOOL_EXECUTION_START
    );
    assert_eq!(
        serde_json::to_value(&end).unwrap()["type"]
            .as_str()
            .unwrap(),
        wire::WIRE_TOOL_EXECUTION_END
    );
}

#[test]
fn extension_event_tool_hooks_use_tool_call_tool_result_wire_names() {
    let call = ExtensionEvent::ToolCall {
        tool_name: "read_file".into(),
        tool_call_id: "c1".into(),
        input: serde_json::json!({}),
    };
    let result = ExtensionEvent::ToolResult {
        tool_name: "read_file".into(),
        tool_call_id: "c1".into(),
        input: serde_json::json!({}),
        content: vec![ContentBlock(serde_json::json!({"text": "ok"}))],
        details: None,
        is_error: false,
    };
    assert_eq!(
        serde_json::to_value(&call).unwrap()["type"]
            .as_str()
            .unwrap(),
        wire::WIRE_TOOL_CALL
    );
    assert_eq!(
        serde_json::to_value(&result).unwrap()["type"]
            .as_str()
            .unwrap(),
        wire::WIRE_TOOL_RESULT
    );
}

#[test]
fn agent_event_compaction_error_serializes() {
    let e = AgentEvent::CompactionError {
        batch_index: 2,
        error: "LLM timeout".to_string(),
    };
    let j = serde_json::to_string(&e).unwrap();
    assert!(j.contains(wire::WIRE_COMPACTION_ERROR));
    assert!(j.contains("batchIndex"));
}

#[test]
fn agent_event_tool_result_truncated_serializes() {
    let e = AgentEvent::ToolResultTruncated {
        tool_name: "read_file".to_string(),
        original_chars: 600_000,
        truncated_chars: 400_000,
    };
    let j = serde_json::to_string(&e).unwrap();
    assert!(j.contains(wire::WIRE_TOOL_RESULT_TRUNCATED));
    assert!(j.contains("toolName"));
    assert!(j.contains("originalChars"));
    assert!(j.contains("truncatedChars"));
}

#[test]
fn extension_event_serialize_camel_case() {
    let e = ExtensionEvent::Startup {
        version: "1.0".to_string(),
        session_file: None,
    };
    let j = serde_json::to_string(&e).unwrap();
    assert!(j.contains(wire::WIRE_STARTUP));
    assert!(j.contains("sessionFile"));
}
