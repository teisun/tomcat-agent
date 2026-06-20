use super::*;
use std::time::Duration;

use serial_test::serial;

use crate::infra::event_bus::EventContext;
use crate::infra::events::wire;

async fn wait_for_lines(
    buffer: &crate::api::serve::test_support::SharedWriterBuffer,
    count: usize,
) -> Vec<serde_json::Value> {
    for _ in 0..50 {
        let lines = read_ndjson_lines(buffer);
        if lines.len() >= count {
            return lines;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    read_ndjson_lines(buffer)
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_event_pump_streams_agent_events() {
    let _api_key = install_test_api_key();
    let (_state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    slot.ctx
        .global_services
        .event_bus
        .emit_sync(
            wire::WIRE_AGENT_START,
            EventContext::new(
                wire::WIRE_AGENT_START,
                serde_json::json!({
                    "type": wire::WIRE_AGENT_START,
                    "sessionId": slot.session_id.clone(),
                    "agentId": "agent-test"
                }),
            )
            .with_session_id(slot.session_id.clone()),
        )
        .unwrap();

    let lines = wait_for_lines(&buffer, 1).await;
    assert_eq!(lines.len(), 1, "expected single routed event, got {lines:?}");
    assert_eq!(
        lines[0].get("type").and_then(serde_json::Value::as_str),
        Some(wire::WIRE_AGENT_START)
    );
    assert_eq!(
        lines[0]
            .get("sessionId")
            .and_then(serde_json::Value::as_str),
        Some(slot.session_id.as_str())
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_lifecycle_events_not_dropped_for_other_sessions() {
    let _api_key = install_test_api_key();
    let (_state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    slot.ctx
        .global_services
        .event_bus
        .emit_sync(
            wire::WIRE_AGENT_END,
            EventContext::new(
                wire::WIRE_AGENT_END,
                serde_json::json!({
                    "type": wire::WIRE_AGENT_END,
                    "sessionId": "other-session",
                    "status": "ok"
                }),
            )
            .with_session_id("other-session"),
        )
        .unwrap();
    slot.ctx
        .global_services
        .event_bus
        .emit_sync(
            wire::WIRE_AGENT_END,
            EventContext::new(
                wire::WIRE_AGENT_END,
                serde_json::json!({
                    "type": wire::WIRE_AGENT_END,
                    "sessionId": slot.session_id.clone(),
                    "status": "ok"
                }),
            )
            .with_session_id(slot.session_id.clone()),
        )
        .unwrap();

    let lines = wait_for_lines(&buffer, 1).await;
    assert_eq!(lines.len(), 1, "expected only same-session event, got {lines:?}");
    assert_eq!(
        lines[0]
            .get("sessionId")
            .and_then(serde_json::Value::as_str),
        Some(slot.session_id.as_str())
    );
    assert_eq!(
        lines[0].get("type").and_then(serde_json::Value::as_str),
        Some(wire::WIRE_AGENT_END)
    );
}
