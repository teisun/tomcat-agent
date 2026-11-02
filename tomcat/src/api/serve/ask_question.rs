//! `ask_question` 的 stdio 控制桥。
//!
//! 负责把 EventBus 上的一问一答协议转译成 `control_request` /
//! `control_response` / `control_cancel` 帧。

use std::sync::Arc;

use dashmap::DashMap;

use crate::api::chat::panels::{
    AskQuestionOutcome, AskQuestionResult, AskQuestionWireRequest, AskQuestionWireResponse,
    EventBusAskQuestionPanel,
};
use crate::infra::event_bus::{EventBus, EventContext};
use crate::{AppError, EventListenerId};

use super::controller_route::{ControlRecipient, ControllerRoute};
use super::types::{ControlFrame, OutFrame};
use super::writer::WriterHandle;

/// Interaction support follows the current controller, not the session's origin.
pub(super) fn bind_panel_for_controller(
    state: &super::ServeState,
    ctx: &crate::api::chat::ChatContext,
    session_id: &str,
) {
    let runtime = &ctx.session_runtime.plan_runtime;
    if state.is_acp() {
        // Context construction installs a terminal panel by default: actively remove it.
        runtime.detach_ask_question_panel();
    } else {
        runtime.attach_ask_question_panel(
            state
                .ask_question
                .panel_for_session(ctx.global_services.event_bus.clone(), session_id),
        );
    }
}

struct PendingQuestion {
    recipient: ControlRecipient,
    response_event: String,
    session_id: String,
    tool_call_id: Option<String>,
    event_bus: Arc<dyn EventBus>,
}

#[derive(Clone)]
pub struct ServeAskQuestionBridge {
    route: ControllerRoute,
    pending: Arc<DashMap<String, PendingQuestion>>,
}

impl ServeAskQuestionBridge {
    pub fn new(writer: WriterHandle) -> Self {
        Self::with_route(ControllerRoute::Direct(writer))
    }

    pub(super) fn with_route(route: ControllerRoute) -> Self {
        Self {
            route,
            pending: Arc::new(DashMap::new()),
        }
    }

    pub(super) fn owns_pending(&self, request_id: &str, connection_id: uuid::Uuid) -> bool {
        self.pending
            .get(request_id)
            .is_some_and(|p| p.recipient.connection_id == connection_id)
    }

    pub fn has_pending_session(&self, session_id: &str) -> bool {
        self.pending
            .iter()
            .any(|pending| pending.session_id == session_id)
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
        let route = self.route.clone();
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
                if request
                    .session_id
                    .as_deref()
                    .is_some_and(|id| id != session_id)
                {
                    tracing::warn!(
                        request_id = request.request_id,
                        payload_session_id = ?request.session_id,
                        listener_session_id = session_id,
                        "dropping ask_question request with mismatched durable session identity"
                    );
                    return Ok(());
                }
                let Some(recipient) = route.recipient(&session_id) else {
                    let response = serde_json::to_value(AskQuestionWireResponse {
                        request_id: request.request_id.clone(),
                        result: AskQuestionResult::terminal(AskQuestionOutcome::HostDisconnected),
                    })?;
                    callback_bus.emit_sync(
                        &request.response_event,
                        EventContext::new(request.response_event.clone(), response)
                            .with_session_id(session_id.clone()),
                    )?;
                    return Ok(());
                };
                pending.insert(
                    request.request_id.clone(),
                    PendingQuestion {
                        recipient: recipient.clone(),
                        response_event: request.response_event.clone(),
                        session_id: session_id.clone(),
                        tool_call_id: request.tool_call_id.clone(),
                        event_bus: callback_bus.clone(),
                    },
                );
                let frame = OutFrame::Control(ControlFrame::request(
                    request.request_id.clone(),
                    "ask_question",
                    Some(session_id.clone()),
                    ctx.payload.clone(),
                ));
                let _ = recipient.writer.send(frame);
                Ok(())
            }),
        )
    }

    pub fn handle_control_response(&self, frame: &ControlFrame) -> Result<bool, AppError> {
        self.handle_control_response_from(frame, uuid::Uuid::nil())
    }

    pub(super) fn handle_control_response_from(
        &self,
        frame: &ControlFrame,
        connection_id: uuid::Uuid,
    ) -> Result<bool, AppError> {
        let ControlFrame::ControlResponse {
            request_id,
            session_id,
            payload,
            ..
        } = frame
        else {
            return Ok(false);
        };
        let Some(entry) = self.pending.get(request_id) else {
            tracing::debug!(
                request_id = request_id,
                "dropping unknown serve control_response"
            );
            return Ok(false);
        };
        if entry.recipient.connection_id != connection_id
            || session_id
                .as_deref()
                .is_some_and(|id| id != entry.session_id)
        {
            tracing::warn!(
                request_id = request_id,
                response_session_id = ?session_id,
                pending_session_id = entry.session_id,
                tool_call_id = ?entry.tool_call_id,
                "dropping ask_question response routed to the wrong session"
            );
            return Ok(false);
        }
        // Decode with the panel's wire contract before consuming its only response route.
        let payload = if payload.get("requestId").is_some() || payload.get("request_id").is_some() {
            payload.clone()
        } else {
            serde_json::json!({ "requestId": request_id, "result": payload })
        };
        let parsed = match serde_json::from_value::<AskQuestionWireResponse>(payload) {
            Ok(parsed) => parsed,
            Err(error) => {
                tracing::warn!(request_id, %error, "dropping malformed ask_question response");
                return Ok(false);
            }
        };
        if parsed.request_id != *request_id {
            tracing::warn!(
                request_id,
                "dropping ask_question response with mismatched inner identity"
            );
            return Ok(false);
        }
        let response = serde_json::to_value(parsed).map_err(|error| {
            AppError::Config(crate::infra::i18n::tr(
                "serve.questionResponseSerialize",
                &[("detail", &error.to_string())],
            ))
        })?;
        drop(entry);
        let Some((_, pending)) = self.pending.remove(request_id) else {
            return Ok(false);
        };
        pending.event_bus.emit_sync(
            &pending.response_event,
            EventContext::new(pending.response_event.clone(), response)
                .with_session_id(pending.session_id),
        )?;
        Ok(true)
    }

    pub fn handle_control_cancel(&self, frame: &ControlFrame) -> Result<bool, AppError> {
        self.handle_control_cancel_from(frame, uuid::Uuid::nil())
    }

    pub(super) fn handle_control_cancel_from(
        &self,
        frame: &ControlFrame,
        connection_id: uuid::Uuid,
    ) -> Result<bool, AppError> {
        let ControlFrame::ControlCancel {
            request_id,
            session_id,
            payload: _,
        } = frame
        else {
            return Ok(false);
        };
        let Some(entry) = self.pending.get(request_id) else {
            tracing::debug!(
                request_id = request_id,
                "dropping unknown serve control_cancel"
            );
            return Ok(false);
        };
        if entry.recipient.connection_id != connection_id
            || session_id
                .as_deref()
                .is_some_and(|id| id != entry.session_id)
        {
            tracing::warn!(
                request_id,
                cancel_session_id = ?session_id,
                pending_session_id = entry.session_id,
                "dropping ask_question cancel routed to the wrong session"
            );
            return Ok(false);
        }
        drop(entry);
        let Some((_, pending)) = self.pending.remove(request_id) else {
            return Ok(false);
        };
        let payload = serde_json::to_value(AskQuestionWireResponse {
            request_id: request_id.clone(),
            result: crate::api::chat::panels::AskQuestionResult::terminal(
                crate::api::chat::panels::AskQuestionOutcome::Interrupted,
            ),
        })
        .map_err(|error| {
            AppError::Config(crate::infra::i18n::tr(
                "serve.questionCancelSerialize",
                &[("detail", &error.to_string())],
            ))
        })?;
        pending.event_bus.emit_sync(
            &pending.response_event,
            EventContext::new(pending.response_event.clone(), payload)
                .with_session_id(pending.session_id),
        )?;
        Ok(true)
    }

    pub fn cancel_live_session(&self, session_id: &str, reason: &str) -> usize {
        let request_ids: Vec<String> = self
            .pending
            .iter()
            .filter(|entry| entry.value().session_id == session_id)
            .map(|entry| entry.key().clone())
            .collect();
        for request_id in &request_ids {
            let Some(pending) = self.pending.get(request_id) else {
                continue;
            };
            let _ = pending
                .recipient
                .writer
                .send(OutFrame::Control(ControlFrame::cancel(
                    request_id.clone(),
                    Some(session_id.to_string()),
                    serde_json::json!({ "outcome": "interrupted", "reason": reason }),
                )));
        }
        self.finalize_session(session_id, AskQuestionOutcome::Interrupted)
    }

    pub fn finalize_session(&self, session_id: &str, outcome: AskQuestionOutcome) -> usize {
        let keys: Vec<String> = self
            .pending
            .iter()
            .filter(|entry| entry.value().session_id == session_id)
            .map(|entry| entry.key().clone())
            .collect();
        let mut finalized = 0;
        for request_id in keys {
            let Some((_, pending)) = self.pending.remove(&request_id) else {
                continue;
            };
            let payload = match serde_json::to_value(AskQuestionWireResponse {
                request_id,
                result: AskQuestionResult::terminal(outcome),
            }) {
                Ok(payload) => payload,
                Err(error) => {
                    tracing::error!(%error, "serialize ask_question terminal response failed");
                    continue;
                }
            };
            if pending
                .event_bus
                .emit_sync(
                    &pending.response_event,
                    EventContext::new(pending.response_event.clone(), payload)
                        .with_session_id(pending.session_id),
                )
                .is_ok()
            {
                finalized += 1;
            }
        }
        finalized
    }

    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn clear_session(&self, session_id: &str) {
        self.finalize_session(session_id, AskQuestionOutcome::HostDisconnected);
    }
}
