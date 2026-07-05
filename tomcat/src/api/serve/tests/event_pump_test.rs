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

#[tokio::test]
#[serial(env_lock)]
async fn serve_event_pump_streams_plan_transition_events() {
    let _api_key = install_test_api_key();
    let (_state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    for event_name in [
        wire::WIRE_PLAN_ENTER,
        wire::WIRE_PLAN_EXIT,
        wire::WIRE_PLAN_PENDING,
        wire::WIRE_PLAN_COMPLETE,
    ] {
        slot.ctx
            .global_services
            .event_bus
            .emit_sync(
                event_name,
                EventContext::new(
                    event_name,
                    serde_json::json!({
                        "type": event_name,
                        "sessionId": slot.session_id.clone(),
                        "planId": "plan-1",
                        "path": "/workspace/plan-1.plan.md",
                        "state": match event_name {
                            wire::WIRE_PLAN_ENTER => "planning",
                            wire::WIRE_PLAN_EXIT => "chat",
                            wire::WIRE_PLAN_PENDING => "pending",
                            _ => "completed",
                        }
                    }),
                )
                .with_session_id(slot.session_id.clone()),
            )
            .unwrap();
    }

    let lines = wait_for_lines(&buffer, 4).await;
    assert_eq!(lines.len(), 4, "expected four routed plan events, got {lines:?}");
    assert_eq!(
        lines
            .iter()
            .map(|line| line.get("type").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec![
            Some(wire::WIRE_PLAN_ENTER),
            Some(wire::WIRE_PLAN_EXIT),
            Some(wire::WIRE_PLAN_PENDING),
            Some(wire::WIRE_PLAN_COMPLETE),
        ]
    );
}
