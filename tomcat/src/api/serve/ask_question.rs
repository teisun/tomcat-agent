//! `ask_question` 的 stdio 控制桥。
//!
//! 负责把 EventBus 上的一问一答协议转译成 `control_request` /
//! `control_response` / `control_cancel` 帧。

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
                if ctx.session_id.as_deref() != Some(session_id.as_str()) {
                    return Ok(());
                }
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
