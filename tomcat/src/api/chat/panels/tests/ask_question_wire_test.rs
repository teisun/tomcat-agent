use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use super::super::{
    ask_question_request_event_name, ask_question_response_event_name, Answer, AskQuestionOutcome,
    AskQuestionPanel, AskQuestionResult, AskQuestionTermination, AskQuestionWireRequest,
    AskQuestionWireResponse, EventBusAskQuestionPanel, Question, QuestionOption,
};
use crate::infra::{DefaultEventBus, EventBus, EventContext};

fn sample_question() -> Question {
    Question {
        allow_custom: true,
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

#[test]
fn runtime_custom_answer_flag_roundtrips_without_becoming_a_model_argument() {
    let mut question = sample_question();
    question.allow_custom = false;
    let request = AskQuestionWireRequest {
        request_id: "fixed".into(),
        response_event: "response".into(),
        session_id: None,
        tool_call_id: None,
        questions: vec![question],
    };
    let value = serde_json::to_value(&request).unwrap();
    assert_eq!(value["questions"][0]["allowCustom"], false);
    let decoded: AskQuestionWireRequest = serde_json::from_value(value).unwrap();
    assert!(!decoded.questions[0].allow_custom);
    let tool_question: Question = serde_json::from_value(serde_json::json!({"id":"q", "prompt":"test", "options":[], "allow_custom":false, "allowCustom":false})).unwrap();
    assert!(
        tool_question.allow_custom,
        "a model cannot disable ordinary custom answers"
    );
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
        outcome: crate::core::plan_runtime::AskQuestionOutcome::Answered,
    }
}

#[test]
fn confirmed_host_disconnect_marks_shared_termination_before_returning_result() {
    let termination = AskQuestionTermination::default();
    let result = super::super::ask_question_wire::confirmed_host_disconnect(&termination);
    assert_eq!(result.outcome, AskQuestionOutcome::HostDisconnected);
    assert_eq!(
        termination
            .result()
            .expect("termination should record host disconnect")
            .outcome,
        AskQuestionOutcome::HostDisconnected
    );
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
        .ask(vec![sample_question()], AskQuestionTermination::default())
        .await;
    host.await.expect("host task");

    assert_eq!(result.outcome, AskQuestionOutcome::Answered);
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
async fn event_bus_panel_returns_interrupted_when_wait_is_aborted() {
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let panel = EventBusAskQuestionPanel::new(bus);
    let termination = AskQuestionTermination::default();
    let termination_for_task = termination.clone();
    let aborter = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        termination_for_task.interrupt();
    });

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        panel.ask(vec![sample_question()], termination),
    )
    .await
    .expect("panel should observe cancel promptly");
    aborter.await.expect("abort task");

    assert_eq!(result.outcome, AskQuestionOutcome::Interrupted);
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
        bus_for_host
            .emit_sync(
                &req.response_event,
                EventContext::new(&req.response_event, serde_json::json!({"invalid": true})),
            )
            .expect("malformed response must not consume listener");
        let response = AskQuestionWireResponse {
            request_id: req.request_id.clone(),
            result: AskQuestionResult {
                answers: vec![],
                outcome: crate::core::plan_runtime::AskQuestionOutcome::CancelledUnknown,
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
        .ask(vec![sample_question()], AskQuestionTermination::default())
        .await;
    host.await.expect("host task");

    assert_eq!(result.outcome, AskQuestionOutcome::CancelledUnknown);
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
        session_id: Some("sid-ask-question".into()),
        tool_call_id: Some("tool-call-serde".into()),
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
    assert_eq!(request_value["sessionId"], "sid-ask-question");
    assert_eq!(request_value["toolCallId"], "tool-call-serde");

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
    assert_eq!(response.result.outcome, AskQuestionOutcome::Answered);
    assert!(!response.result.answers[0].picked_recommended);
}

#[tokio::test(start_paused = true)]
async fn event_bus_panel_has_no_wall_clock_deadline_even_after_many_hours() {
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let panel = EventBusAskQuestionPanel::new(Arc::clone(&bus));
    let termination = AskQuestionTermination::default();
    let termination_for_task = termination.clone();
    let task = tokio::spawn(async move {
        panel
            .ask(vec![sample_question()], termination_for_task)
            .await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(12 * 60 * 60)).await;
    tokio::task::yield_now().await;
    assert!(
        !task.is_finished(),
        "ask_question must remain pending without user input"
    );

    termination.interrupt();
    let result = task.await.expect("panel task");
    assert_eq!(result.outcome, AskQuestionOutcome::Interrupted);
}
