//! Translate the existing question panel to controller or same-project observer frames.

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

/// A controller binding resets prior automatic consent, but never removes question support.
pub(super) fn bind_panel_for_controller(
    state: &super::ServeState,
    ctx: &crate::api::chat::ChatContext,
    session_id: &str,
) {
    let runtime = &ctx.session_runtime.plan_runtime;
    runtime.set_interaction_policy(state.acp_options.unwrap_or_default().interaction_policy());
    runtime.attach_ask_question_panel(
        state
            .ask_question
            .panel_for_session(ctx.global_services.event_bus.clone(), session_id),
    );
}

struct PendingQuestion {
    recipient: ControlRecipient,
    session_id: String,
    request: AskQuestionWireRequest,
    event_bus: Arc<dyn EventBus>,
}

#[derive(Clone)]
pub struct ServeAskQuestionBridge {
    route: ControllerRoute,
    pending: Arc<DashMap<String, PendingQuestion>>,
}

fn question_frame(pending: &PendingQuestion) -> Result<OutFrame, AppError> {
    Ok(OutFrame::Control(ControlFrame::request(
        pending.request.request_id.clone(),
        "ask_question",
        Some(pending.session_id.clone()),
        serde_json::to_value(&pending.request)?,
    )))
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
            .is_some_and(|entry| entry.recipient.connection_id == connection_id)
    }

    fn may_answer(&self, pending: &PendingQuestion, connection_id: uuid::Uuid) -> bool {
        pending.recipient.connection_id == connection_id
            || (pending.recipient.is_acp
                && self.route.is_observer(&pending.session_id, connection_id))
    }

    pub(super) fn may_respond(&self, request_id: &str, connection_id: uuid::Uuid) -> bool {
        self.pending
            .get(request_id)
            .is_some_and(|entry| self.may_answer(&entry, connection_id))
    }

    pub(super) fn replay_for(
        &self,
        connection: &super::ServeState,
        session_id: &str,
    ) -> Result<(), AppError> {
        if !self.route.is_observer(session_id, connection.connection_id) {
            return Ok(());
        }
        for entry in self
            .pending
            .iter()
            .filter(|entry| entry.session_id == session_id && entry.recipient.is_acp)
        {
            // Hold the pending borrow through enqueue: resolution must enqueue its cancel after this request.
            connection.writer.send(question_frame(&entry)?)?;
        }
        Ok(())
    }

    fn withdraw(&self, pending: &PendingQuestion, reason: &str, except: Option<uuid::Uuid>) {
        if !pending.recipient.is_acp {
            return;
        }
        for observer in self.route.observers(&pending.session_id) {
            if Some(observer.connection_id) != except {
                let _ = observer.writer.send(OutFrame::Control(ControlFrame::cancel(
                    pending.request.request_id.clone(),
                    Some(pending.session_id.clone()),
                    serde_json::json!({"reason": reason}),
                )));
            }
        }
    }

    pub fn has_pending_session(&self, session_id: &str) -> bool {
        self.pending
            .iter()
            .any(|entry| entry.session_id == session_id)
    }

    pub fn panel_for_session(
        &self,
        event_bus: Arc<dyn EventBus>,
        session_id: &str,
    ) -> Arc<dyn crate::api::chat::panels::AskQuestionPanel> {
        let bridge = self.clone();
        Arc::new(
            EventBusAskQuestionPanel::new(event_bus)
                .with_session_id(session_id.to_string())
                .with_request_id_prefix(format!("askq-{session_id}"))
                .with_request_finalizer(Arc::new(move |request_id| {
                    if let Some((_, pending)) = bridge.pending.remove(request_id) {
                        bridge.withdraw(&pending, "terminated", None);
                    }
                })),
        )
    }

    pub fn register_request_listener(
        &self,
        session_id: String,
        event_bus: Arc<dyn EventBus>,
    ) -> EventListenerId {
        let bridge = self.clone();
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
                let Some(recipient) = bridge.route.recipient(&session_id) else {
                    let response = serde_json::to_value(AskQuestionWireResponse {
                        request_id: request.request_id.clone(),
                        result: AskQuestionResult::terminal(AskQuestionOutcome::HostDisconnected),
                    })?;
                    // EventBus invokes listeners under its lock; reply after this callback returns.
                    let bus = callback_bus.clone();
                    let session_id = session_id.clone();
                    tokio::spawn(async move {
                        if let Err(error) = bus.emit_sync(
                            &request.response_event,
                            EventContext::new(request.response_event.clone(), response)
                                .with_session_id(session_id),
                        ) {
                            tracing::warn!(%error, "deliver disconnected question response");
                        }
                    });
                    return Ok(());
                };
                let request_id = request.request_id.clone();
                bridge.pending.insert(
                    request_id.clone(),
                    PendingQuestion {
                        recipient,
                        session_id: session_id.clone(),
                        request,
                        event_bus: callback_bus.clone(),
                    },
                );
                if let Some(entry) = bridge.pending.get(&request_id) {
                    let frame = question_frame(&entry)?;
                    if entry.recipient.is_acp {
                        for observer in bridge.route.observers(&session_id) {
                            let _ = observer.writer.send(frame.clone());
                        }
                    } else {
                        let _ = entry.recipient.writer.send(frame);
                    }
                }
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
            tracing::debug!(request_id, "dropping unknown serve control_response");
            return Ok(false);
        };
        let observer = entry.recipient.connection_id != connection_id;
        if !self.may_answer(&entry, connection_id)
            || session_id
                .as_deref()
                .is_some_and(|id| id != entry.session_id)
            || (observer && session_id.as_deref() != Some(entry.session_id.as_str()))
            || ["sessionId", "session_id"].iter().any(|key| {
                payload
                    .get(*key)
                    .is_some_and(|id| id.as_str() != Some(entry.session_id.as_str()))
            })
        {
            tracing::warn!(
                request_id,
                response_session_id = ?session_id,
                pending_session_id = entry.session_id,
                tool_call_id = ?entry.request.tool_call_id,
                %connection_id,
                "dropping ask_question response routed to the wrong session"
            );
            return Ok(false);
        }
        let payload = if payload.get("requestId").is_some() || payload.get("request_id").is_some() {
            payload.clone()
        } else {
            serde_json::json!({"requestId": request_id, "result": payload})
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
        let rejection = if observer && parsed.result.outcome != AskQuestionOutcome::Answered {
            Some("observers may only submit answers")
        } else if parsed.result.outcome == AskQuestionOutcome::Answered
            && crate::core::tools::plan_tool::ask_question::validate_answers(
                &entry.request.questions,
                &parsed.result,
            )
            .is_err()
        {
            Some("answers failed question validation")
        } else if parsed.result.outcome != AskQuestionOutcome::Answered
            && !parsed.result.answers.is_empty()
        {
            Some("terminal responses must not contain answers")
        } else {
            None
        };
        if let Some(reason) = rejection {
            tracing::warn!(
                request_id,
                observer,
                reason,
                "dropping rejected ask_question response"
            );
            return Ok(false);
        }
        let response = serde_json::to_value(parsed)?;
        drop(entry);
        let Some((_, pending)) = self.pending.remove(request_id) else {
            return Ok(false);
        };
        self.withdraw(&pending, "resolved", Some(connection_id));
        pending.event_bus.emit_sync(
            &pending.request.response_event,
            EventContext::new(pending.request.response_event.clone(), response)
                .with_session_id(pending.session_id.clone()),
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
            ..
        } = frame
        else {
            return Ok(false);
        };
        let Some(entry) = self.pending.get(request_id) else {
            tracing::debug!(request_id, "dropping unknown serve control_cancel");
            return Ok(false);
        };
        // An observer leaving is not a cancellation of the controller's question.
        if entry.recipient.connection_id != connection_id
            || session_id
                .as_deref()
                .is_some_and(|id| id != entry.session_id)
        {
            tracing::warn!(
                request_id,
                cancel_session_id = ?session_id,
                pending_session_id = entry.session_id,
                tool_call_id = ?entry.request.tool_call_id,
                %connection_id,
                "dropping ask_question cancel routed to the wrong session"
            );
            return Ok(false);
        }
        drop(entry);
        let Some((_, pending)) = self.pending.remove(request_id) else {
            return Ok(false);
        };
        self.withdraw(&pending, "interrupted", None);
        let payload = serde_json::to_value(AskQuestionWireResponse {
            request_id: request_id.clone(),
            result: AskQuestionResult::terminal(AskQuestionOutcome::Interrupted),
        })?;
        pending.event_bus.emit_sync(
            &pending.request.response_event,
            EventContext::new(pending.request.response_event.clone(), payload)
                .with_session_id(pending.session_id.clone()),
        )?;
        Ok(true)
    }

    pub fn cancel_live_session(&self, session_id: &str, reason: &str) -> usize {
        for entry in self
            .pending
            .iter()
            .filter(|entry| entry.session_id == session_id && !entry.recipient.is_acp)
        {
            let _ = entry
                .recipient
                .writer
                .send(OutFrame::Control(ControlFrame::cancel(
                    entry.key().clone(),
                    Some(session_id.to_string()),
                    serde_json::json!({"outcome":"interrupted", "reason":reason}),
                )));
        }
        self.finalize_session(session_id, AskQuestionOutcome::Interrupted)
    }

    pub fn finalize_session(&self, session_id: &str, outcome: AskQuestionOutcome) -> usize {
        let keys = self
            .pending
            .iter()
            .filter(|entry| entry.session_id == session_id)
            .map(|entry| entry.key().clone())
            .collect::<Vec<_>>();
        let mut finalized = 0;
        for request_id in keys {
            let Some((_, pending)) = self.pending.remove(&request_id) else {
                continue;
            };
            self.withdraw(&pending, "terminated", None);
            let payload = serde_json::to_value(AskQuestionWireResponse {
                request_id,
                result: AskQuestionResult::terminal(outcome),
            });
            match payload {
                Ok(payload) => {
                    if pending
                        .event_bus
                        .emit_sync(
                            &pending.request.response_event,
                            EventContext::new(pending.request.response_event.clone(), payload)
                                .with_session_id(pending.session_id.clone()),
                        )
                        .is_ok()
                    {
                        finalized += 1;
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "serialize ask_question terminal response failed")
                }
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
