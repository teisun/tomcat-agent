use std::sync::Arc;

use crate::api::serve::registry::SessionSlot;
use crate::infra::events::wire;
use crate::EventListenerId;

use super::types::OutFrame;
use super::writer::WriterHandle;

const EVENT_NAMES: &[&str] = &[
    wire::WIRE_AGENT_START,
    wire::WIRE_AGENT_END,
    wire::WIRE_TURN_START,
    wire::WIRE_TURN_END,
    wire::WIRE_MESSAGE_START,
    wire::WIRE_MESSAGE_UPDATE,
    wire::WIRE_MESSAGE_END,
    wire::WIRE_LLM_ERROR,
    wire::WIRE_LLM_NOTICE,
    wire::WIRE_TOOL_EXECUTION_START,
    wire::WIRE_TOOL_CALL_STREAMING,
    wire::WIRE_TOOL_EXECUTION_UPDATE,
    wire::WIRE_TOOL_EXECUTION_END,
    wire::WIRE_AUTO_COMPACTION_START,
    wire::WIRE_AUTO_COMPACTION_END,
    wire::WIRE_COMPACTION_ERROR,
    wire::WIRE_TOOL_RESULT_TRUNCATED,
    wire::WIRE_AUTO_RETRY_START,
    wire::WIRE_AUTO_RETRY_END,
    wire::WIRE_CONTEXT_METRICS_UPDATE,
    wire::WIRE_TOOL_RESULT_PERSISTED,
    wire::WIRE_BOUNDARY_SWITCHED,
    wire::WIRE_CONTEXT_OVERFLOW_TRIM_START,
    wire::WIRE_CONTEXT_OVERFLOW_TRIM_END,
    wire::WIRE_LAYER0_CONTEXT_RELEASE,
    wire::WIRE_EXTENSION_ERROR,
    wire::WIRE_SEARCH_TOOLS_PREFLIGHT,
    wire::WIRE_GIT_PREFLIGHT,
    wire::WIRE_AGENT_INTERRUPTED,
    wire::WIRE_SUB_AGENT_START,
    wire::WIRE_SUB_AGENT_END,
];

pub fn register_session_event_pump(
    slot: &Arc<SessionSlot>,
    writer: WriterHandle,
) -> Vec<EventListenerId> {
    let mut ids = Vec::new();
    for event_name in EVENT_NAMES {
        let session_id = slot.session_id.clone();
        let writer = writer.clone();
        let id = slot.ctx.global_services.event_bus.on(
            event_name,
            Box::new(move |ctx| {
                if ctx.session_id.as_deref() != Some(session_id.as_str()) {
                    return Ok(());
                }
                let _ = writer.send(OutFrame::Event(ctx.payload.clone()));
                Ok(())
            }),
        );
        ids.push(id);
    }
    ids
}

pub fn unregister_session_event_pump(slot: &Arc<SessionSlot>) {
    let ids = std::mem::take(&mut *slot.listener_ids.lock());
    for id in ids {
        slot.ctx.global_services.event_bus.off(id);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serial_test::serial;

    use crate::api::serve::test_support::{
        build_initialized_state_with_streams, install_test_api_key, read_ndjson_lines,
    };
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
        assert_eq!(
            lines.len(),
            1,
            "expected single routed event, got {lines:?}"
        );
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
        assert_eq!(
            lines.len(),
            1,
            "expected only same-session event, got {lines:?}"
        );
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
}
