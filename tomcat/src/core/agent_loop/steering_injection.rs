//! # Steering 注入 helper
//!
//! 把 `steering_queue` 中的消息统一走「记账 + append/persist + push」通道，避免
//! `messages.extend(q.drain(..))` 绕过 `ctx_state.on_message_appended(...)` 与
//! `push_message(...)`，从而导致 mid-turn 计数与 `msg_id` 不一致。

use std::sync::Arc;

use parking_lot::Mutex;

use crate::core::llm::ChatMessage;
use crate::core::session::manager::estimate_msg_chars;
use crate::infra::error::AppError;

use super::types::AgentLoop;

fn inject_queue_messages(
    agent: &mut AgentLoop,
    queue: &Arc<Mutex<Vec<ChatMessage>>>,
    messages: &mut Vec<ChatMessage>,
) -> Result<bool, AppError> {
    let drained = {
        let mut q = queue.lock();
        if q.is_empty() {
            return Ok(false);
        }
        q.drain(..).collect::<Vec<_>>()
    };

    for msg in drained {
        if let Some(ref mut ctx_state) = agent.context_state {
            ctx_state.on_message_appended(estimate_msg_chars(&msg));
        }
        agent.push_message(messages, msg)?;
    }

    Ok(true)
}

pub(super) fn inject_steering_messages(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
) -> Result<bool, AppError> {
    let queue = agent.steering_queue.clone();
    inject_queue_messages(agent, &queue, messages)
}

pub(super) fn inject_follow_up_messages(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
) -> Result<bool, AppError> {
    let queue = agent.follow_up_queue.clone();
    inject_queue_messages(agent, &queue, messages)
}
