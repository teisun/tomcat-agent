use super::super::*;

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
        tool_name: "read".into(),
        args: serde_json::json!({}),
    };
    let streaming = AgentEvent::ToolCallStreaming {
        tool_call_id: "c1".into(),
        tool_name: "write".into(),
        args_preview: serde_json::json!({"path": "~/demo.txt"}),
    };
    let end = AgentEvent::ToolExecutionEnd {
        tool_call_id: "c1".into(),
        tool_name: "read".into(),
        result: ToolOutput(serde_json::json!({})),
        display: Some(ToolDisplay::File {
            file: "~/demo.txt".into(),
        }),
        is_error: false,
    };
    assert_eq!(
        serde_json::to_value(&start).unwrap()["type"]
            .as_str()
            .unwrap(),
        wire::WIRE_TOOL_EXECUTION_START
    );
    assert_eq!(
        serde_json::to_value(&streaming).unwrap()["type"]
            .as_str()
            .unwrap(),
        wire::WIRE_TOOL_CALL_STREAMING
    );
    assert_eq!(
        serde_json::to_value(&end).unwrap()["type"]
            .as_str()
            .unwrap(),
        wire::WIRE_TOOL_EXECUTION_END
    );
    let streaming_payload = serde_json::to_value(&streaming).unwrap();
    assert_eq!(streaming_payload["toolCallId"].as_str(), Some("c1"));
    assert_eq!(streaming_payload["toolName"].as_str(), Some("write"));
    assert_eq!(
        streaming_payload["argsPreview"]["path"].as_str(),
        Some("~/demo.txt")
    );
    let payload = serde_json::to_value(&end).unwrap();
    assert_eq!(payload["display"]["kind"].as_str(), Some("file"));
    assert_eq!(payload["display"]["file"].as_str(), Some("~/demo.txt"));
}

#[test]
fn wire_envelope_flattens_session_id_and_agent_event_fields() {
    let event = AgentEvent::ToolExecutionStart {
        tool_call_id: "c1".into(),
        tool_name: "read".into(),
        args: serde_json::json!({"path": "src/main.rs"}),
    };
    let payload = serde_json::to_value(WireEnvelope::new(Some("s1"), &event)).unwrap();
    assert_eq!(
        payload["type"].as_str(),
        Some(wire::WIRE_TOOL_EXECUTION_START)
    );
    assert_eq!(payload["sessionId"].as_str(), Some("s1"));
    assert_eq!(payload["toolCallId"].as_str(), Some("c1"));
    assert_eq!(payload["toolName"].as_str(), Some("read"));
    assert_eq!(payload["args"]["path"].as_str(), Some("src/main.rs"));
    assert!(
        payload.get("event").is_none(),
        "flatten 后不应出现嵌套 event key"
    );
}

#[test]
fn wire_envelope_handles_unit_variant_without_panicking() {
    let event = ExtensionEvent::Startup {
        version: "1.0.0".into(),
        session_file: None,
    };
    let payload = serde_json::to_value(ExtensionWireEnvelope::new(Some("s1"), &event)).unwrap();
    assert_eq!(payload["type"].as_str(), Some(wire::WIRE_STARTUP));
    assert_eq!(payload["sessionId"].as_str(), Some("s1"));
    assert_eq!(payload["version"].as_str(), Some("1.0.0"));
    assert!(
        payload.get("event").is_none(),
        "flatten 后不应出现嵌套 event key"
    );
}

#[test]
fn wire_envelope_omits_session_id_when_none() {
    let event = AgentEvent::ToolCallStreaming {
        tool_call_id: "c1".into(),
        tool_name: "write".into(),
        args_preview: serde_json::json!({"path": "~/demo.txt"}),
    };
    let payload = serde_json::to_value(WireEnvelope::new(None, &event)).unwrap();
    assert_eq!(
        payload["type"].as_str(),
        Some(wire::WIRE_TOOL_CALL_STREAMING)
    );
    assert!(
        payload.get("sessionId").is_none(),
        "session_id=None 时不应输出 sessionId"
    );
}

#[test]
fn wire_envelope_preserves_agent_event_shape_plus_session_id() {
    let event = AgentEvent::ContextMetricsUpdate {
        input_tokens_used: 100,
        context_utilization_ratio: 0.5,
        compaction_count: 2,
        compaction_tokens_freed: 128,
        total_tool_result_bytes_persisted: 256,
        preheat_in_progress: false,
        preheat_result_pending: true,
    };
    let mut expected = serde_json::to_value(&event).unwrap();
    expected
        .as_object_mut()
        .expect("agent event payload should be object")
        .insert("sessionId".into(), serde_json::json!("s1"));
    let actual = serde_json::to_value(WireEnvelope::new(Some("s1"), &event)).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn extension_wire_envelope_preserves_tool_call_shape_plus_session_id() {
    let event = ExtensionEvent::ToolCall {
        tool_name: "read".into(),
        tool_call_id: "c1".into(),
        input: serde_json::json!({"path": "src/main.rs"}),
    };
    let mut expected = serde_json::to_value(&event).unwrap();
    expected
        .as_object_mut()
        .expect("extension event payload should be object")
        .insert("sessionId".into(), serde_json::json!("s1"));
    let actual = serde_json::to_value(ExtensionWireEnvelope::new(Some("s1"), &event)).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn agent_event_llm_error_and_notice_use_dedicated_wire_names() {
    let err = AgentEvent::LlmError {
        reason: "error:boom".into(),
        error_code: Some("server_error".into()),
        error_message: "boom".into(),
    };
    let notice = AgentEvent::LlmNotice {
        finish_reason: "max_output_tokens".into(),
        message: "达到 max_output_tokens，回答可能未完成".into(),
    };
    let err_json = serde_json::to_value(&err).unwrap();
    let notice_json = serde_json::to_value(&notice).unwrap();
    assert_eq!(err_json["type"].as_str(), Some(wire::WIRE_LLM_ERROR));
    assert_eq!(err_json["errorCode"].as_str(), Some("server_error"));
    assert_eq!(err_json["errorMessage"].as_str(), Some("boom"));
    assert_eq!(notice_json["type"].as_str(), Some(wire::WIRE_LLM_NOTICE));
    assert_eq!(
        notice_json["finishReason"].as_str(),
        Some("max_output_tokens")
    );
}

#[test]
fn extension_event_tool_hooks_use_tool_call_tool_result_wire_names() {
    let call = ExtensionEvent::ToolCall {
        tool_name: "read".into(),
        tool_call_id: "c1".into(),
        input: serde_json::json!({}),
    };
    let result = ExtensionEvent::ToolResult {
        tool_name: "read".into(),
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
fn wire_plan_build_and_update_constants_are_stable() {
    assert_eq!(wire::WIRE_PLAN_BUILD, "plan.build");
    assert_eq!(wire::WIRE_PLAN_UPDATE, "plan.update");
}

#[test]
fn plan_event_payload_roundtrip() {
    let payload = PlanEventPayload {
        plan_id: "plan_user_login_abc".into(),
        path: "~/.tomcat/plans/plan_user_login_abc.plan.md".into(),
        state: "executing".into(),
    };
    let json = serde_json::to_value(&payload).unwrap();
    assert_eq!(json["plan_id"].as_str(), Some("plan_user_login_abc"));
    assert_eq!(
        json["path"].as_str(),
        Some("~/.tomcat/plans/plan_user_login_abc.plan.md")
    );
    assert_eq!(json["state"].as_str(), Some("executing"));

    let decoded: PlanEventPayload = serde_json::from_value(json).unwrap();
    assert_eq!(decoded, payload);
}

#[test]
fn agent_event_compaction_error_serializes() {
    let e = AgentEvent::CompactionError {
        exhausted_after_retries: true,
        attempts: 3,
        error: "LLM timeout".to_string(),
        source: "preheat".to_string(),
        ratio: Some(0.65),
    };
    let j = serde_json::to_value(&e).unwrap();
    assert_eq!(j["type"].as_str().unwrap(), wire::WIRE_COMPACTION_ERROR);
    assert!(j["exhaustedAfterRetries"].as_bool().unwrap());
    assert_eq!(j["attempts"].as_u64().unwrap(), 3);
    assert_eq!(j["source"].as_str().unwrap(), "preheat");
    assert!(!j.to_string().contains("batchIndex"));
}

#[test]
fn agent_event_tool_result_truncated_serializes() {
    let e = AgentEvent::ToolResultTruncated {
        tool_name: "read".to_string(),
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

#[test]
fn auto_compaction_start_serializes_with_new_payload() {
    let e = AgentEvent::AutoCompactionStart {
        covered_count: 5,
        ratio_before: 0.72,
    };
    let j = serde_json::to_value(&e).unwrap();
    assert_eq!(
        j["type"].as_str().unwrap(),
        wire::WIRE_AUTO_COMPACTION_START
    );
    assert_eq!(j["coveredCount"].as_u64().unwrap(), 5);
    assert!(j["ratioBefore"].as_f64().is_some());
    assert!(!j.to_string().contains("reason"));
}

#[test]
fn auto_compaction_end_serializes_with_new_payload() {
    let e = AgentEvent::AutoCompactionEnd {
        elapsed_ms: 1234,
        summary_chars: 5000,
        covered_count: 5,
        ratio_after: 0.30,
        estimated_covered_tokens_before: 100,
        estimated_summary_tokens: 20,
        estimated_tokens_saved: 80,
    };
    let j = serde_json::to_value(&e).unwrap();
    assert_eq!(j["type"].as_str().unwrap(), wire::WIRE_AUTO_COMPACTION_END);
    assert_eq!(j["elapsedMs"].as_u64().unwrap(), 1234);
    assert_eq!(j["summaryChars"].as_u64().unwrap(), 5000);
    assert!(!j.to_string().contains("aborted"));
}

#[test]
fn context_overflow_trim_start_serializes() {
    let e = AgentEvent::ContextOverflowTrimStart {
        reason: "context_overflow".into(),
        ratio: 1.05,
    };
    let j = serde_json::to_value(&e).unwrap();
    assert_eq!(
        j["type"].as_str().unwrap(),
        wire::WIRE_CONTEXT_OVERFLOW_TRIM_START
    );
    assert_eq!(j["reason"].as_str().unwrap(), "context_overflow");
    assert!(j["ratio"].as_f64().unwrap() > 1.0);
}

#[test]
fn context_overflow_trim_end_serializes() {
    let e = AgentEvent::ContextOverflowTrimEnd {
        ratio_before: 1.05,
        ratio_after: 0.40,
        will_retry: true,
        estimated_tokens_freed: 400,
        turns_removed: 2,
    };
    let j = serde_json::to_value(&e).unwrap();
    assert_eq!(
        j["type"].as_str().unwrap(),
        wire::WIRE_CONTEXT_OVERFLOW_TRIM_END
    );
    assert!(j["willRetry"].as_bool().unwrap());
    assert!(j["ratioBefore"].as_f64().unwrap() > 1.0);
    assert!(j["ratioAfter"].as_f64().unwrap() < 0.50);
}

#[test]
fn boundary_switched_serializes() {
    let e = AgentEvent::BoundarySwitched {
        ratio_before: 0.85,
        ratio_after: 0.30,
        covered_count: 4,
        was_sync_wait: false,
        estimated_tokens_freed: 50,
    };
    let j = serde_json::to_value(&e).unwrap();
    assert_eq!(j["type"].as_str().unwrap(), wire::WIRE_BOUNDARY_SWITCHED);
    assert_eq!(j["coveredCount"].as_u64().unwrap(), 4);
    assert!(!j["wasSyncWait"].as_bool().unwrap());
}

#[test]
fn context_metrics_update_serializes_preheat_result_pending() {
    let e = AgentEvent::ContextMetricsUpdate {
        input_tokens_used: 100,
        context_utilization_ratio: 0.5,
        compaction_count: 0,
        compaction_tokens_freed: 0,
        total_tool_result_bytes_persisted: 0,
        preheat_in_progress: false,
        preheat_result_pending: true,
    };
    let j = serde_json::to_value(&e).unwrap();
    assert_eq!(
        j["type"].as_str().unwrap(),
        wire::WIRE_CONTEXT_METRICS_UPDATE
    );
    assert_eq!(j["preheatInProgress"].as_bool(), Some(false));
    assert_eq!(j["preheatResultPending"].as_bool(), Some(true));
}
