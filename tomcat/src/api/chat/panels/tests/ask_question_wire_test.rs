use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use super::super::{
    ask_question_request_event_name, ask_question_response_event_name, Answer,
    AskQuestionPanel, AskQuestionResult, AskQuestionWireRequest, AskQuestionWireResponse,
    EventBusAskQuestionPanel, Question, QuestionOption,
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
            result: AskQuestionResult {
                answers: vec![Answer {
                    question_id: "color".into(),
                    option_ids: vec!["red".into()],
                    custom_text: None,
                    skipped: false,
                    picked_recommended: true,
                }],
                cancelled: false,
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

    let result = tokio::time::timeout(Duration::from_secs(1), panel.ask(vec![sample_question()], cancel))
        .await
        .expect("panel should observe cancel promptly");
    aborter.await.expect("abort task");

    assert!(result.cancelled);
    assert!(result.answers.is_empty());
}
