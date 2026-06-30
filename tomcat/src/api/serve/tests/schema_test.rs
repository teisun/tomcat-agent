use super::*;
use serde_json::json;

use crate::infra::events::{
    AgentEvent, AssistantMessageEvent, Message, ToolDisplay, ToolOutput, WireEvent,
};

#[test]
fn serve_dts_emits_named_types_not_unknown() {
    let dts = serve_dts();
    assert!(dts.contains("export type ServeCommand = "));
    assert!(dts.contains("export type WireEvent = "));
    assert!(!dts.contains("export type ServeCommand = unknown;"));
    assert!(!dts.contains("export type OutFrame = unknown;"));
}

#[test]
fn serve_dts_preserves_wire_event_session_id() {
    let dts = serve_dts();
    assert!(dts.contains("export type WireEvent = "));
    assert!(dts.contains("sessionId?: null | string;"));
    assert!(dts.contains("type: \"message_update\";"));
    assert!(dts.contains("type: \"message_start\";"));
    assert!(dts.contains("type: \"message_end\";"));
    assert!(dts.matches("assistantMessageId: string;").count() >= 3);
    assert!(dts.contains("assistantMessageId?: null | string;"));
}

#[test]
fn build_schema_bundle_includes_wire_event() {
    let value = serde_json::to_value(build_schema_bundle()).expect("serialize schema bundle");
    assert!(value.get("wire_event").is_some(), "wire_event root missing");
}

#[test]
fn serve_emitted_event_validates_against_generated_schema() {
    let bundle = build_schema_bundle();
    let bundle_value = serde_json::to_value(&bundle).expect("serialize schema bundle");
    let schema = bundle_value
        .get("wire_event")
        .cloned()
        .expect("wire_event schema should exist");
    let validator = jsonschema::validator_for(&schema).expect("compile wire event schema");
    let samples = vec![
        serde_json::to_value(WireEvent {
            session_id: Some("s1".to_string()),
            event: AgentEvent::MessageUpdate {
                assistant_message_id: "a1".to_string(),
                message: Message(json!({
                    "role": "assistant",
                    "content": "partial"
                })),
                assistant_message_event: AssistantMessageEvent(json!({
                    "kind": "content_delta",
                    "delta": "partial"
                })),
            },
        })
        .expect("message_update sample"),
        serde_json::to_value(WireEvent {
            session_id: Some("s1".to_string()),
            event: AgentEvent::ToolExecutionStart {
                tool_call_id: "call_1".to_string(),
                tool_name: "ask_question".to_string(),
                args: json!({"questions": [{"id": "q1"}]}),
            },
        })
        .expect("tool_execution_start sample"),
        serde_json::to_value(WireEvent {
            session_id: Some("s1".to_string()),
            event: AgentEvent::ToolExecutionEnd {
                tool_call_id: "call_1".to_string(),
                tool_name: "ask_question".to_string(),
                result: ToolOutput(json!({"cancelled": true})),
                display: Some(ToolDisplay::Text {
                    text: "cancelled".to_string(),
                }),
                is_error: false,
            },
        })
        .expect("tool_execution_end sample"),
        serde_json::to_value(WireEvent {
            session_id: Some("s1".to_string()),
            event: AgentEvent::AgentEnd {
                messages: vec![Message(json!({
                    "role": "assistant",
                    "content": "done"
                }))],
                error: None,
            },
        })
        .expect("agent_end sample"),
    ];

    for sample in samples {
        assert!(
            validator.is_valid(&sample),
            "sample should validate against generated schema: {sample}"
        );
    }
}
