use std::sync::Arc;

use dashmap::DashMap;

use crate::api::chat::panels::{
    AskQuestionWireRequest, AskQuestionWireResponse, EventBusAskQuestionPanel,
};
use crate::infra::event_bus::{EventBus, EventContext};
use crate::{AppError, EventListenerId};

use super::types::{ControlFrame, OutFrame};
use super::writer::WriterHandle;

struct PendingQuestion {
    response_event: String,
    session_id: String,
    event_bus: Arc<dyn EventBus>,
}

#[derive(Clone)]
pub struct ServeAskQuestionBridge {
    writer: WriterHandle,
    pending: Arc<DashMap<String, PendingQuestion>>,
}

impl ServeAskQuestionBridge {
    pub fn new(writer: WriterHandle) -> Self {
        Self {
            writer,
            pending: Arc::new(DashMap::new()),
        }
    }

    pub fn panel_for_session(
        &self,
        event_bus: Arc<dyn EventBus>,
        session_id: &str,
    ) -> Arc<dyn crate::api::chat::panels::AskQuestionPanel> {
        Arc::new(
            EventBusAskQuestionPanel::new(event_bus)
                .with_session_id(session_id.to_string())
                .with_request_id_prefix(format!("askq-{session_id}")),
        )
    }

    pub fn register_request_listener(
        &self,
        session_id: String,
        event_bus: Arc<dyn EventBus>,
    ) -> EventListenerId {
        let writer = self.writer.clone();
        let pending = Arc::clone(&self.pending);
        let callback_bus = event_bus.clone();
        event_bus.on(
            crate::infra::wire::WIRE_PLAN_ASK_QUESTION,
            Box::new(move |ctx| {
                let Ok(request) =
                    serde_json::from_value::<AskQuestionWireRequest>(ctx.payload.clone())
                else {
                    return Ok(());
                };
                pending.insert(
                    request.request_id.clone(),
                    PendingQuestion {
                        response_event: request.response_event.clone(),
                        session_id: session_id.clone(),
                        event_bus: callback_bus.clone(),
                    },
                );
                let frame = OutFrame::Control(ControlFrame::request(
                    request.request_id.clone(),
                    "ask_question",
                    Some(session_id.clone()),
                    ctx.payload.clone(),
                ));
                let _ = writer.send(frame);
                Ok(())
            }),
        )
    }

    pub fn handle_control_response(&self, frame: &ControlFrame) -> Result<bool, AppError> {
        let ControlFrame::ControlResponse {
            request_id,
            payload,
            ..
        } = frame
        else {
            return Ok(false);
        };
        let Some((_, pending)) = self.pending.remove(request_id) else {
            tracing::debug!(
                request_id = request_id,
                "dropping unknown serve control_response"
            );
            return Ok(false);
        };
        let response = if let Ok(parsed) =
            serde_json::from_value::<AskQuestionWireResponse>(payload.clone())
        {
            serde_json::to_value(parsed).map_err(|error| {
                AppError::Config(format!("serialize ask question response failed: {error}"))
            })?
        } else if payload
            .get("requestId")
            .and_then(serde_json::Value::as_str)
            .is_some()
        {
            payload.clone()
        } else {
            serde_json::to_value(AskQuestionWireResponse {
                request_id: request_id.clone(),
                result: serde_json::from_value(payload.clone()).unwrap_or(
                    crate::api::chat::panels::AskQuestionResult {
                        answers: Vec::new(),
                        cancelled: true,
                    },
                ),
            })
            .map_err(|error| {
                AppError::Config(format!("serialize ask question response failed: {error}"))
            })?
        };
        pending.event_bus.emit_sync(
            &pending.response_event,
            EventContext::new(pending.response_event.clone(), response)
                .with_session_id(pending.session_id),
        )?;
        Ok(true)
    }

    pub fn handle_control_cancel(&self, frame: &ControlFrame) -> Result<bool, AppError> {
        let ControlFrame::ControlCancel {
            request_id,
            session_id: _,
            payload: _,
        } = frame
        else {
            return Ok(false);
        };
        let Some((_, pending)) = self.pending.remove(request_id) else {
            tracing::debug!(
                request_id = request_id,
                "dropping unknown serve control_cancel"
            );
            return Ok(false);
        };
        let payload = serde_json::to_value(AskQuestionWireResponse {
            request_id: request_id.clone(),
            result: crate::api::chat::panels::AskQuestionResult {
                answers: Vec::new(),
                cancelled: true,
            },
        })
        .map_err(|error| {
            AppError::Config(format!("serialize ask question cancel failed: {error}"))
        })?;
        pending.event_bus.emit_sync(
            &pending.response_event,
            EventContext::new(pending.response_event.clone(), payload)
                .with_session_id(pending.session_id),
        )?;
        Ok(true)
    }

    pub fn clear_session(&self, session_id: &str) {
        let keys: Vec<String> = self
            .pending
            .iter()
            .filter(|entry| entry.value().session_id == session_id)
            .map(|entry| entry.key().clone())
            .collect();
        for key in keys {
            self.pending.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ServeAskQuestionBridge;
    use std::sync::Arc;
    use std::time::Duration;

    use parking_lot::Mutex;
    use serial_test::serial;

    use crate::api::chat::panels::{
        ask_question_response_event_name, Answer, AskQuestionResult, AskQuestionWireRequest,
        AskQuestionWireResponse, Question, QuestionOption,
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
            questions: vec![Question {
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
        let request = sample_request("ask-1");

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
    }

    #[tokio::test]
    #[serial(env_lock)]
    async fn serve_ask_question_bridge_round_trips_control_response() {
        let (writer, _buffer) = spawn_buffered_writer(&ServeConfig::default());
        let bridge = ServeAskQuestionBridge::new(writer);
        let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
        let session_id = "serve-askq";
        bridge.register_request_listener(session_id.to_string(), Arc::clone(&bus));
        let request = sample_request("ask-2");
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
                serde_json::to_value(AskQuestionWireResponse {
                    request_id: request.request_id.clone(),
                    result: AskQuestionResult {
                        answers: vec![Answer {
                            question_id: "color".to_string(),
                            option_ids: vec!["blue".to_string()],
                            custom_text: None,
                            skipped: false,
                            picked_recommended: true,
                        }],
                        cancelled: false,
                    },
                })
                .unwrap(),
            ))
            .unwrap();
        assert!(handled);

        let response = captured.lock().clone().expect("response event emitted");
        assert_eq!(response.request_id, "ask-2");
        assert_eq!(response.result.answers.len(), 1);
        assert!(!response.result.cancelled);
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
}
