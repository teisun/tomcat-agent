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
    assert!(dts.contains("export interface WireModelView {"));
    assert!(dts.contains("export interface SetProviderKeyResponse {"));
    assert!(dts.contains("export interface ProviderKeyView {"));
    assert!(dts.contains("export interface ListModelsPayload {"));
    assert!(!dts.contains("export type ServeCommand = unknown;"));
    assert!(!dts.contains("export type OutFrame = unknown;"));
}

#[test]
fn serve_dts_preserves_wire_event_session_id() {
    let dts = serve_dts();
    assert!(dts.contains("export type WireEvent = "));
    assert!(dts.contains("sessionId?: null | string;"));
    assert!(dts.contains("type: \"agent_idle\";"));
    assert!(dts.contains("type: \"message_update\";"));
    assert!(dts.contains("type: \"message_start\";"));
    assert!(dts.contains("type: \"message_end\";"));
    assert!(dts.matches("assistantMessageId: string;").count() >= 3);
    assert!(dts.contains("assistantMessageId?: null | string;"));
}

#[test]
fn serve_dts_includes_file_display_diff_fields() {
    let dts = serve_dts();
    assert!(dts.contains("export type ToolDisplay = "));
    assert!(dts.contains("added?: null | number;"));
    assert!(dts.contains("removed?: null | number;"));
    assert!(dts.contains("export type DiffTag = "));
    assert!(dts.contains("export interface FileDiffLine {"));
    assert!(dts.contains("diff?: FileDiffLine[] | null;"));
}

#[test]
fn serve_dts_includes_user_message_id_on_message_params() {
    let dts = serve_dts();
    assert!(dts.contains("export interface ServeMessageParams {"));
    assert!(dts.contains("segments?: ServeContentSegment[];"));
    assert!(dts.contains("userMessageId?: null | string;"));
}

#[test]
fn serve_dts_includes_checkpoint_commands() {
    let dts = serve_dts();
    assert!(dts.contains("type: \"list_checkpoints\";"));
    assert!(dts.contains("type: \"restore_checkpoint\";"));
    assert!(dts.contains("checkpointId: string;"));
    assert!(dts.contains("revertFiles: boolean;"));
    assert!(dts.contains("dryRun?: boolean | null;"));
}

#[test]
fn serve_dts_includes_context_reference_types() {
    let dts = serve_dts();
    assert!(dts.contains("export type ServeContextRefKind = \"selection\" | \"file\";"));
    assert!(dts.contains("export type ServeContentSegment = "));
    assert!(dts.contains("kind: ServeContextRefKind;"));
    assert!(dts.contains("label: string;"));
    assert!(dts.contains("path: string;"));
    assert!(dts.contains("type: \"reference\";"));
    assert!(dts.contains("type: \"text\";"));
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
            event: AgentEvent::ToolExecutionEnd {
                tool_call_id: "call_2".to_string(),
                tool_name: "write".to_string(),
                result: ToolOutput(json!({"path": "demo.txt"})),
                display: Some(ToolDisplay::File {
                    file: "demo.txt".to_string(),
                    added: Some(545),
                    removed: Some(0),
                    diff: Some(vec![
                        crate::core::tools::primitive::FileDiffLine {
                            tag: crate::core::tools::primitive::DiffTag::Ctx,
                            old_line: Some(1),
                            new_line: Some(1),
                            text: "before".to_string(),
                        },
                        crate::core::tools::primitive::FileDiffLine {
                            tag: crate::core::tools::primitive::DiffTag::Add,
                            old_line: None,
                            new_line: Some(2),
                            text: "after".to_string(),
                        },
                    ]),
                }),
                is_error: false,
            },
        })
        .expect("tool_execution_end file sample"),
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
        serde_json::to_value(WireEvent {
            session_id: Some("s1".to_string()),
            event: AgentEvent::AgentIdle,
        })
        .expect("agent_idle sample"),
    ];

    for sample in samples {
        assert!(
            validator.is_valid(&sample),
            "sample should validate against generated schema: {sample}"
        );
    }
}

#[test]
fn serve_command_schema_validates_checkpoint_commands() {
    let bundle = build_schema_bundle();
    let bundle_value = serde_json::to_value(&bundle).expect("serialize schema bundle");
    let schema = bundle_value
        .get("serve_command")
        .cloned()
        .expect("serve_command schema should exist");
    let validator = jsonschema::validator_for(&schema).expect("compile serve command schema");
    let samples = [
        json!({
            "type": "list_checkpoints",
            "id": "list-1",
            "sessionId": "s1"
        }),
        json!({
            "type": "restore_checkpoint",
            "id": "restore-1",
            "sessionId": "s1",
            "checkpointId": "ck-1",
            "revertFiles": false,
            "dryRun": true
        }),
    ];

    for sample in samples {
        assert!(
            validator.is_valid(&sample),
            "checkpoint serve command should validate: {sample}"
        );
    }
}

#[test]
fn response_frame_schema_validates_get_messages_payload_with_error_entries() {
    let bundle = build_schema_bundle();
    let bundle_value = serde_json::to_value(&bundle).expect("serialize schema bundle");
    let schema = bundle_value
        .get("response_frame")
        .cloned()
        .expect("response_frame schema should exist");
    let validator = jsonschema::validator_for(&schema).expect("compile response frame schema");
    let sample = json!({
        "type": "response",
        "id": "messages-1",
        "success": true,
        "sessionId": "s1",
        "payload": {
            "sessionId": "s1",
            "header": {
                "type": "session",
                "version": 3,
                "id": "s1",
                "timestamp": "2026-07-17T05:00:00.000Z",
                "cwd": null
            },
            "messages": [
                {
                    "type": "message",
                    "id": "m1",
                    "parentId": null,
                    "timestamp": "2026-07-17T05:00:01.000Z",
                    "message": {
                        "role": "user",
                        "content": "retry",
                        "superseded": true,
                        "turn_failed": true
                    }
                },
                {
                    "type": "error",
                    "id": "err_1",
                    "parentId": null,
                    "timestamp": "2026-07-17T05:00:02.000Z",
                    "phase": "Llm",
                    "provider": "openai",
                    "model": "gpt-5.4",
                    "apiFamily": "responses",
                    "statusCode": 403,
                    "requestId": "req_123",
                    "summary": "API 错误 403 · PS-SHA-01JfN78 · Request-Id req_123",
                    "detail": "API 错误 403 原文"
                }
            ],
            "nextCursor": null,
            "hasMore": false,
            "upToSeq": null
        }
    });

    assert!(
        validator.is_valid(&sample),
        "get_messages response with error entry should validate: {sample}"
    );
}
