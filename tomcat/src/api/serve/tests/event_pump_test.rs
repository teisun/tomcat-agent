use super::*;
use std::time::Duration;

use serial_test::serial;

use crate::core::llm::ChatMessageContent;
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
    assert_eq!(
        lines.len(),
        4,
        "expected four routed plan events, got {lines:?}"
    );
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

#[test]
fn serve_event_pump_allowlist_includes_summary_upgrade_events() {
    assert!(
        super::super::event_pump::EVENT_NAMES.contains(&wire::WIRE_TURN_SUMMARY_UPDATED),
        "turn.summary_updated must stay on the serve allowlist"
    );
    assert!(
        super::super::event_pump::EVENT_NAMES.contains(&wire::WIRE_TOOL_SUMMARY_UPDATED),
        "tool.summary_updated must stay on the serve allowlist"
    );
    assert!(
        super::super::event_pump::EVENT_NAMES.contains(&wire::WIRE_BACKGROUND_TASK_FINISHED),
        "background_task_finished must stay on the serve allowlist"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_background_task_finish_routes_event_and_queues_follow_up_for_same_session() {
    let _api_key = install_test_api_key();
    let (_state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let ticket = slot
        .ctx
        .session_runtime
        .bash_task_registry
        .spawn("printf done".to_string(), None, None)
        .await
        .expect("spawn background task");

    for _ in 0..100 {
        let has_follow_up = !slot.ctx.session_runtime.follow_up_queue.lock().is_empty();
        let has_event = read_ndjson_lines(&buffer).iter().any(|line| {
            line.get("type").and_then(serde_json::Value::as_str)
                == Some(wire::WIRE_BACKGROUND_TASK_FINISHED)
        });
        if has_follow_up && has_event {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let lines = read_ndjson_lines(&buffer);
    assert_eq!(
        lines.len(),
        1,
        "background task completion should not trigger extra serve turns: {lines:?}"
    );
    let event = lines
        .iter()
        .find(|line| {
            line.get("type").and_then(serde_json::Value::as_str)
                == Some(wire::WIRE_BACKGROUND_TASK_FINISHED)
        })
        .expect("background_task_finished should be routed to serve writer");
    assert_eq!(
        event.get("sessionId").and_then(serde_json::Value::as_str),
        Some(slot.session_id.as_str())
    );
    assert_eq!(
        event.get("taskId").and_then(serde_json::Value::as_str),
        Some(ticket.task_id.as_str())
    );
    assert_eq!(
        event.get("logPath").and_then(serde_json::Value::as_str),
        Some(ticket.log_path.as_str())
    );
    assert_eq!(
        event.get("command").and_then(serde_json::Value::as_str),
        Some("printf done")
    );
    assert_eq!(
        event.get("exitCode").and_then(serde_json::Value::as_i64),
        Some(0)
    );

    let queue = slot.ctx.session_runtime.follow_up_queue.lock();
    assert_eq!(queue.len(), 1, "background completion should queue exactly one follow-up");
    let Some(ChatMessageContent::Text(text)) = &queue[0].content else {
        panic!("expected background completion follow-up text, got {:?}", queue[0].content);
    };
    assert!(
        text.contains("<background-task-finished"),
        "expected synthetic completion envelope, got {text:?}"
    );
    assert!(
        text.contains(ticket.task_id.as_str()),
        "expected queued follow-up to mention task id, got {text:?}"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_cleanup_aborts_background_task_subscribers() {
    let _api_key = install_test_api_key();
    let (state, _buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    assert!(
        slot.ctx
            .session_runtime
            .completion_subscriber_handle
            .lock()
            .is_some(),
        "serve register_slot_hooks should install completion subscriber handle"
    );
    assert!(
        slot.background_task_listener.lock().is_some(),
        "serve register_slot_hooks should install background task listener handle"
    );

    super::super::cleanup_session_slot(&state, &slot, false, "test_cleanup")
        .await
        .expect("cleanup session slot");

    assert!(
        slot.ctx
            .session_runtime
            .completion_subscriber_handle
            .lock()
            .is_none(),
        "cleanup should clear completion subscriber handle"
    );
    assert!(
        slot.background_task_listener.lock().is_none(),
        "cleanup should clear background task listener handle"
    );
}
