use super::ServeAskQuestionBridge;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serial_test::serial;

use crate::api::chat::panels::{
    ask_question_response_event_name, AskQuestionWireRequest, AskQuestionWireResponse, Question,
    QuestionOption,
};
use crate::api::serve::test_support::{read_ndjson_lines, spawn_buffered_writer};
use crate::api::serve::types::ControlFrame;
use crate::infra::{DefaultEventBus, EventBus, EventContext};
use crate::ServeConfig;

async fn wait_for_line(
    buffer: &crate::api::serve::test_support::SharedWriterBuffer,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> Vec<serde_json::Value> {
    for _ in 0..50 {
        let lines = read_ndjson_lines(buffer);
        if lines.iter().any(&predicate) {
            return lines;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    read_ndjson_lines(buffer)
}

fn sample_request(request_id: &str) -> AskQuestionWireRequest {
    AskQuestionWireRequest {
        request_id: request_id.to_string(),
        response_event: ask_question_response_event_name(request_id),
        session_id: Some("sid-a".to_string()),
        tool_call_id: Some("tool-call-a".to_string()),
        questions: vec![Question {
            allow_custom: true,
            id: "color".to_string(),
            prompt: "Pick a color".to_string(),
            options: vec![
                QuestionOption {
                    id: "blue".to_string(),
                    label: "Blue".to_string(),
                    recommended: true,
                },
                QuestionOption {
                    id: "green".to_string(),
                    label: "Green".to_string(),
                    recommended: false,
                },
            ],
        }],
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_ask_question_bridge_emits_control_request() {
    let (writer, buffer) = spawn_buffered_writer(&ServeConfig::default());
    let bridge = ServeAskQuestionBridge::new(writer);
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let session_id = "serve-askq";
    bridge.register_request_listener(session_id.to_string(), Arc::clone(&bus));
    let mut request = sample_request("ask-1");
    request.session_id = Some(session_id.to_string());

    bus.emit_sync(
        crate::infra::wire::WIRE_PLAN_ASK_QUESTION,
        EventContext::new(
            crate::infra::wire::WIRE_PLAN_ASK_QUESTION,
            serde_json::to_value(request).unwrap(),
        )
        .with_session_id(session_id),
    )
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("control_request")
    })
    .await;
    let frame = lines
        .iter()
        .find(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("control_request")
        })
        .unwrap();
    assert_eq!(
        frame.get("subtype").and_then(serde_json::Value::as_str),
        Some("ask_question")
    );
    assert_eq!(
        frame.get("sessionId").and_then(serde_json::Value::as_str),
        Some(session_id)
    );
    assert_eq!(
        frame["payload"]
            .get("requestId")
            .and_then(serde_json::Value::as_str),
        Some("ask-1")
    );
    assert_eq!(
        frame["payload"]
            .get("responseEvent")
            .and_then(serde_json::Value::as_str),
        Some(ask_question_response_event_name("ask-1").as_str())
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_ask_question_bridge_round_trips_control_response() {
    let (writer, _buffer) = spawn_buffered_writer(&ServeConfig::default());
    let bridge = ServeAskQuestionBridge::new(writer);
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let session_id = "serve-askq";
    bridge.register_request_listener(session_id.to_string(), Arc::clone(&bus));
    let mut request = sample_request("ask-2");
    request.session_id = Some(session_id.to_string());
    let captured = Arc::new(Mutex::new(None::<AskQuestionWireResponse>));
    let captured_for_listener = Arc::clone(&captured);
    bus.on(
        &request.response_event,
        Box::new(move |ctx| {
            let parsed: AskQuestionWireResponse =
                serde_json::from_value(ctx.payload.clone()).expect("response should parse");
            *captured_for_listener.lock() = Some(parsed);
            Ok(())
        }),
    );

    bus.emit_sync(
        crate::infra::wire::WIRE_PLAN_ASK_QUESTION,
        EventContext::new(
            crate::infra::wire::WIRE_PLAN_ASK_QUESTION,
            serde_json::to_value(&request).unwrap(),
        )
        .with_session_id(session_id),
    )
    .unwrap();

    let handled = bridge
        .handle_control_response(&ControlFrame::response(
            request.request_id.clone(),
            Some(session_id.to_string()),
            serde_json::json!({
                "requestId": request.request_id,
                "result": {
                    "answers": [{
                        "questionId": "color",
                        "optionIds": ["blue"],
                        "pickedRecommended": true
                    }],
                    "cancelled": false
                }
            }),
        ))
        .unwrap();
    assert!(handled);

    let response = captured.lock().clone().expect("response event emitted");
    assert_eq!(response.request_id, "ask-2");
    assert_eq!(response.result.answers.len(), 1);
    assert_eq!(
        response.result.outcome,
        crate::core::plan_runtime::AskQuestionOutcome::Answered
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_ask_question_bridge_routes_by_session() {
    let (writer, buffer) = spawn_buffered_writer(&ServeConfig::default());
    let bridge = ServeAskQuestionBridge::new(writer);
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    bridge.register_request_listener("session-a".to_string(), Arc::clone(&bus));
    bridge.register_request_listener("session-b".to_string(), Arc::clone(&bus));
    let mut request = sample_request("ask-route");
    request.session_id = Some("session-a".to_string());

    bus.emit_sync(
        crate::infra::wire::WIRE_PLAN_ASK_QUESTION,
        EventContext::new(
            crate::infra::wire::WIRE_PLAN_ASK_QUESTION,
            serde_json::to_value(request).unwrap(),
        )
        .with_session_id("session-a"),
    )
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("control_request")
    })
    .await;
    let controls = lines
        .iter()
        .filter(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("control_request")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        controls.len(),
        1,
        "expected exactly one routed control frame"
    );
    assert_eq!(
        controls[0]
            .get("sessionId")
            .and_then(serde_json::Value::as_str),
        Some("session-a")
    );
}

#[tokio::test]
async fn serve_ask_question_bridge_ignores_unknown_request_id() {
    let (writer, _buffer) = spawn_buffered_writer(&ServeConfig::default());
    let bridge = ServeAskQuestionBridge::new(writer);

    let handled = bridge
        .handle_control_response(&ControlFrame::response(
            "missing-request",
            Some("s1".to_string()),
            serde_json::json!({ "cancelled": true, "answers": [] }),
        ))
        .unwrap();

    assert!(!handled);
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_ask_question_terminal_race_finalizes_once_and_drops_late_responses() {
    let (writer, _buffer) = spawn_buffered_writer(&ServeConfig::default());
    let bridge = ServeAskQuestionBridge::new(writer);
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    bridge.register_request_listener("sid-a".to_string(), Arc::clone(&bus));
    let request = sample_request("ask-race");
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let outcomes_for_listener = Arc::clone(&outcomes);
    bus.on(
        &request.response_event,
        Box::new(move |ctx| {
            let parsed: AskQuestionWireResponse = serde_json::from_value(ctx.payload).unwrap();
            outcomes_for_listener.lock().push(parsed.result.outcome);
            Ok(())
        }),
    );
    bus.emit_sync(
        crate::infra::wire::WIRE_PLAN_ASK_QUESTION,
        EventContext::new(
            crate::infra::wire::WIRE_PLAN_ASK_QUESTION,
            serde_json::to_value(&request).unwrap(),
        )
        .with_session_id("sid-a"),
    )
    .unwrap();
    assert_eq!(bridge.pending_count(), 1);

    assert_eq!(
        bridge.finalize_session(
            "sid-a",
            crate::core::plan_runtime::AskQuestionOutcome::Interrupted,
        ),
        1
    );
    assert_eq!(
        bridge.finalize_session(
            "sid-a",
            crate::core::plan_runtime::AskQuestionOutcome::HostDisconnected,
        ),
        0
    );
    assert_eq!(bridge.pending_count(), 0);
    assert_eq!(
        outcomes.lock().as_slice(),
        &[crate::core::plan_runtime::AskQuestionOutcome::Interrupted]
    );
    assert!(!bridge
        .handle_control_response(&ControlFrame::response(
            request.request_id,
            Some("sid-a".to_string()),
            serde_json::json!({ "answers": [], "cancelled": false, "outcome": "answered" }),
        ))
        .unwrap());
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_ask_question_wrong_session_keeps_pending_until_valid_response() {
    let (writer, _buffer) = spawn_buffered_writer(&ServeConfig::default());
    let bridge = ServeAskQuestionBridge::new(writer);
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    bridge.register_request_listener("sid-a".to_string(), Arc::clone(&bus));
    let request = sample_request("ask-wrong-session");
    bus.emit_sync(
        crate::infra::wire::WIRE_PLAN_ASK_QUESTION,
        EventContext::new(
            crate::infra::wire::WIRE_PLAN_ASK_QUESTION,
            serde_json::to_value(&request).unwrap(),
        )
        .with_session_id("sid-a"),
    )
    .unwrap();

    assert!(!bridge
        .handle_control_response(&ControlFrame::response(
            request.request_id.clone(),
            Some("sid-b".to_string()),
            serde_json::json!({ "answers": [], "cancelled": true, "outcome": "skipped" }),
        ))
        .unwrap());
    assert_eq!(bridge.pending_count(), 1);
    assert!(bridge
        .handle_control_response(&ControlFrame::response(
            request.request_id,
            Some("sid-a".to_string()),
            serde_json::json!({ "answers": [], "cancelled": true, "outcome": "skipped" }),
        ))
        .unwrap());
    assert_eq!(bridge.pending_count(), 0);
}
