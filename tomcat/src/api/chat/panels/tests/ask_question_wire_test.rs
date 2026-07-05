use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use super::super::{
    ask_question_request_event_name, ask_question_response_event_name, Answer, AskQuestionPanel,
    AskQuestionResult, AskQuestionWireRequest, AskQuestionWireResponse, EventBusAskQuestionPanel,
    Question, QuestionOption,
};
use crate::infra::{DefaultEventBus, EventBus, EventContext};

fn sample_question() -> Question {
    Question {
        id: "color".into(),
        prompt: "pick a color".into(),
        options: vec![
            QuestionOption {
                id: "red".into(),
                label: "Red".into(),
                recommended: true,
            },
            QuestionOption {
                id: "blue".into(),
                label: "Blue".into(),
                recommended: false,
            },
        ],
    }
}

fn sample_result() -> AskQuestionResult {
    AskQuestionResult {
        answers: vec![Answer {
            question_id: "color".into(),
            option_ids: vec!["red".into()],
            custom_text: None,
            skipped: false,
            picked_recommended: true,
        }],
        cancelled: false,
    }
}

#[tokio::test]
async fn event_bus_panel_round_trips_via_mock_host() {
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let panel = EventBusAskQuestionPanel::new(bus.clone()).with_request_id_prefix("wiretest");
    let captured = Arc::new(Mutex::new(None::<AskQuestionWireRequest>));
    let request_ready = Arc::new(tokio::sync::Notify::new());

    let captured_for_listener = captured.clone();
    let request_ready_for_listener = request_ready.clone();
    bus.on(
        ask_question_request_event_name(),
        Box::new(move |ctx| {
            let req: AskQuestionWireRequest =
                serde_json::from_value(ctx.payload).expect("wire request should parse");
            *captured_for_listener.lock() = Some(req);
            request_ready_for_listener.notify_one();
            Ok(())
        }),
    );

    let bus_for_host = bus.clone();
    let captured_for_host = captured.clone();
    let host = tokio::spawn(async move {
        request_ready.notified().await;
        let req = captured_for_host
            .lock()
            .clone()
            .expect("host should receive request");
        let response = AskQuestionWireResponse {
            request_id: req.request_id.clone(),
            result: sample_result(),
        };
        bus_for_host
            .emit_sync(
                &req.response_event,
                EventContext::new(
                    &req.response_event,
                    serde_json::to_value(response).expect("wire response serialize"),
                ),
            )
            .expect("host response emit");
    });

    let result = panel
        .ask(vec![sample_question()], Arc::new(AtomicBool::new(false)))
        .await;
    host.await.expect("host task");

    assert!(!result.cancelled);
    assert_eq!(result.answers.len(), 1);
    assert_eq!(result.answers[0].question_id, "color");
    assert_eq!(result.answers[0].option_ids, vec!["red"]);
    assert!(result.answers[0].picked_recommended);

    let req = captured.lock().clone().expect("request should be captured");
    assert_eq!(req.questions.len(), 1);
    assert_eq!(req.questions[0].id, "color");
    assert_eq!(req.questions[0].options.len(), 2);
    assert_eq!(
        req.response_event,
        ask_question_response_event_name(&req.request_id)
    );
}

#[tokio::test]
async fn event_bus_panel_returns_cancelled_when_wait_is_aborted() {
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let panel = EventBusAskQuestionPanel::new(bus);
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_task = cancel.clone();
    let aborter = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        cancel_for_task.store(true, Ordering::Relaxed);
    });

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        panel.ask(vec![sample_question()], cancel),
    )
    .await
    .expect("panel should observe cancel promptly");
    aborter.await.expect("abort task");

    assert!(result.cancelled);
    assert!(result.answers.is_empty());
}

#[tokio::test]
async fn event_bus_panel_request_event_carries_session_id() {
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let panel = EventBusAskQuestionPanel::new(bus.clone())
        .with_session_id("sid-ask-question")
        .with_request_id_prefix("wiretest");
    let captured_ctx = Arc::new(Mutex::new(None::<EventContext>));
    let request_ready = Arc::new(tokio::sync::Notify::new());

    let captured_ctx_for_listener = captured_ctx.clone();
    let request_ready_for_listener = request_ready.clone();
    bus.on(
        ask_question_request_event_name(),
        Box::new(move |ctx| {
            *captured_ctx_for_listener.lock() = Some(ctx);
            request_ready_for_listener.notify_one();
            Ok(())
        }),
    );

    let bus_for_host = bus.clone();
    let captured_ctx_for_host = captured_ctx.clone();
    let host = tokio::spawn(async move {
        request_ready.notified().await;
        let ctx = captured_ctx_for_host
            .lock()
            .clone()
            .expect("host should receive request ctx");
        let req: AskQuestionWireRequest =
            serde_json::from_value(ctx.payload.clone()).expect("wire request should parse");
        let response = AskQuestionWireResponse {
            request_id: req.request_id.clone(),
            result: AskQuestionResult {
                answers: vec![],
                cancelled: true,
            },
        };
        bus_for_host
            .emit_sync(
                &req.response_event,
                EventContext::new(
                    &req.response_event,
                    serde_json::to_value(response).expect("wire response serialize"),
                ),
            )
            .expect("host response emit");
    });

    let result = panel
        .ask(vec![sample_question()], Arc::new(AtomicBool::new(false)))
        .await;
    host.await.expect("host task");

    assert!(result.cancelled);
    let ctx = captured_ctx
        .lock()
        .clone()
        .expect("request ctx should be captured");
    assert_eq!(ctx.session_id.as_deref(), Some("sid-ask-question"));
    assert_eq!(
        ctx.payload.get("sessionId").and_then(|v| v.as_str()),
        Some("sid-ask-question")
    );
}

#[test]
fn ask_question_wire_payload_serializes_as_camel_case() {
    let request = AskQuestionWireRequest {
        request_id: "ask-serde".into(),
        response_event: ask_question_response_event_name("ask-serde"),
        questions: vec![sample_question()],
    };
    let request_value = serde_json::to_value(&request).expect("serialize wire request");
    assert_eq!(
        request_value.get("requestId").and_then(|v| v.as_str()),
        Some("ask-serde")
    );
    assert!(
        request_value.get("request_id").is_none(),
        "wire request should not leak snake_case keys"
    );
    assert_eq!(
        request_value.get("responseEvent").and_then(|v| v.as_str()),
        Some(ask_question_response_event_name("ask-serde").as_str())
    );

    let response = AskQuestionWireResponse {
        request_id: "ask-serde".into(),
        result: sample_result(),
    };
    let response_value = serde_json::to_value(&response).expect("serialize wire response");
    assert_eq!(
        response_value["result"]["answers"][0]
            .get("questionId")
            .and_then(|v| v.as_str()),
        Some("color")
    );
    assert_eq!(
        response_value["result"]["answers"][0]
            .get("optionIds")
            .and_then(|v| v.as_array())
            .and_then(|items| items.first())
            .and_then(|v| v.as_str()),
        Some("red")
    );
    assert!(
        response_value["result"]["answers"][0]
            .get("question_id")
            .is_none(),
        "wire response should not leak snake_case keys"
    );
}

#[test]
fn ask_question_wire_payload_deserializes_camel_case_host_response() {
    let response: AskQuestionWireResponse = serde_json::from_value(serde_json::json!({
        "requestId": "ask-deser",
        "result": {
            "answers": [{
                "questionId": "color",
                "optionIds": ["blue"],
                "customText": "navy",
                "pickedRecommended": false
            }],
            "cancelled": false
        }
    }))
    .expect("deserialize camelCase host response");

    assert_eq!(response.request_id, "ask-deser");
    assert_eq!(response.result.answers[0].question_id, "color");
    assert_eq!(response.result.answers[0].option_ids, vec!["blue"]);
    assert_eq!(
        response.result.answers[0].custom_text.as_deref(),
        Some("navy")
    );
    assert!(!response.result.answers[0].picked_recommended);
}
