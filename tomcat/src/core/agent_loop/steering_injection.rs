//! Existing steering safe points share this inbox; admission and closure use one lock.
use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::core::llm::ChatMessage;
use crate::core::session::manager::estimate_msg_chars;
use crate::infra::error::AppError;
use crate::infra::events::AgentEvent;

use super::types::AgentLoop;

/// A rich input keeps the durable original separate from its provider representation.
/// `archival = None` preserves the existing core API for already-built messages.
#[derive(Debug, Clone)]
pub struct SteeringInput {
    pub archival: Option<ChatMessage>,
    pub provider: ChatMessage,
    pub user_message_id: Option<String>,
    pub attachment_shas: Vec<String>,
}

#[derive(Debug)]
pub struct SteeringInbox {
    open: bool,
    items: VecDeque<SteeringInput>,
}

impl Default for SteeringInbox {
    fn default() -> Self {
        Self {
            open: true,
            items: VecDeque::new(),
        }
    }
}

impl From<Vec<ChatMessage>> for SteeringInbox {
    fn from(messages: Vec<ChatMessage>) -> Self {
        let mut inbox = Self::default();
        for message in messages {
            inbox.push(message);
        }
        inbox
    }
}

impl SteeringInbox {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn open(&mut self) {
        // Previous-run leftovers are never eligible for a new run.
        self.items.clear();
        self.open = true;
    }
    pub fn clear(&mut self) {
        self.items.clear();
    }
    pub fn close_and_clear(&mut self) {
        self.open = false;
        self.clear();
    }
    /// Call while holding the same mutex used by push_if_open.
    pub fn close_if_empty(&mut self) -> bool {
        if !self.items.is_empty() {
            return false;
        }
        self.open = false;
        true
    }
    pub fn push_if_open(&mut self, input: SteeringInput) -> bool {
        if !self.open {
            return false;
        }
        self.items.push_back(input);
        true
    }
    /// Legacy text/typed core callers remain compatible; no target-model control.
    pub fn push(&mut self, message: ChatMessage) {
        self.push_if_open(SteeringInput {
            archival: None,
            user_message_id: None,
            provider: message,
            attachment_shas: Vec::new(),
        });
    }
}

impl std::ops::Index<usize> for SteeringInbox {
    type Output = ChatMessage;
    fn index(&self, index: usize) -> &Self::Output {
        &self.items[index].provider
    }
}

pub(super) fn inject_steering_messages(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
) -> Result<bool, AppError> {
    let queue = agent.steering_queue.clone();
    // Freeze the batch size, not a second queue. Later arrivals stay for the next safe point.
    let count = queue.lock().len();
    for _ in 0..count {
        if agent.cancel_token.is_cancelled() {
            break;
        }
        let Some(mut input) = queue.lock().items.pop_front() else {
            break;
        };
        if let Some(mut archival) = input.archival.take() {
            let id = input
                .user_message_id
                .as_deref()
                .expect("rich steering has a stable ID");
            agent.persist_message_with_forced_id_if_needed(&mut archival, id)?;
            input.provider.msg_id = Some(id.to_string());
            if let Some(session) = agent.session_manager.as_ref() {
                let store = session.attachment_store();
                for sha in &input.attachment_shas {
                    if let Err(error) = store.promote(&agent.config.session_id, sha) {
                        tracing::warn!(%sha, %error, "steering attachment promotion failed after archival");
                    }
                }
            }
        }
        agent.push_message(messages, input.provider)?;
        if let Some(ctx_state) = agent.context_state.as_mut() {
            ctx_state
                .on_message_appended(estimate_msg_chars(messages.last().expect("just appended")));
        }
        if let Some(id) = input.user_message_id {
            // Synchronous EventBus emission precedes the outer run's agent_idle frame.
            agent.emit_event(AgentEvent::SteeringConsumed {
                user_message_ids: vec![id],
            });
        }
    }
    Ok(count > 0)
}

pub(super) fn inject_follow_up_messages(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
) -> Result<bool, AppError> {
    // Internal follow-up / Signal semantics are intentionally unchanged.
    let queue: Arc<Mutex<Vec<ChatMessage>>> = agent.follow_up_queue.clone();
    let drained = {
        let mut q = queue.lock();
        if q.is_empty() {
            return Ok(false);
        }
        q.drain(..).collect::<Vec<_>>()
    };
    for msg in drained {
        if let Some(ctx_state) = agent.context_state.as_mut() {
            ctx_state.on_message_appended(estimate_msg_chars(&msg));
        }
        agent.push_message(messages, msg)?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn close_if_empty_and_admission_share_one_boundary() {
        let mut inbox = SteeringInbox::default();
        inbox.push(ChatMessage::steering("first"));
        assert!(!inbox.close_if_empty());
        inbox.clear();
        assert!(inbox.close_if_empty());
        inbox.push(ChatMessage::steering("too late"));
        assert!(inbox.is_empty());
        inbox.open();
        inbox.push(ChatMessage::steering("next run"));
        assert_eq!(inbox.len(), 1);
        inbox.close_and_clear();
        assert!(inbox.is_empty());
        assert!(!inbox.is_open());
    }
}
