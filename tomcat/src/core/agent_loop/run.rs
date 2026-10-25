//! # Conversation lifecycle
//!
//! Conversation → Reasoning (model/tools) → per-request retry.
//! This layer owns follow-up input and the Completed / Interrupted / Failed exit;
//! request retries never restart tool execution or reset the reasoning loop's text.

use crate::core::llm::{ChatMessage, ChatMessageRole};
use crate::core::session::{
    find_dangling_tail_tool_call_ids,
    manager::{estimate_msg_chars, INTERRUPTED_TOOL_RESULT_TEXT},
};
use crate::infra::events::AgentEvent;
use tracing::warn;

use super::reasoning_loop::run_reasoning_loop;
use super::steering_injection::{inject_follow_up_messages, inject_steering_messages};
use super::types::{AgentLoop, AgentRunOutcome, AgentRunResult, LoopError};

fn append_missing_interrupted_tool_results(partial_messages: &mut Vec<ChatMessage>) -> usize {
    let Ok(recent) = partial_messages
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
    else {
        return 0;
    };
    let Some(tool_call_ids) = find_dangling_tail_tool_call_ids(&recent) else {
        return 0;
    };
    let added = tool_call_ids.len();
    for tool_call_id in tool_call_ids {
        partial_messages.push(ChatMessage::tool(
            &tool_call_id,
            INTERRUPTED_TOOL_RESULT_TEXT,
        ));
    }
    added
}

impl AgentLoop {
    /// 第一层：Conversation loop，处理 FollowUp。
    ///
    /// 返回 `AgentRunOutcome` 三态：`Completed` / `Interrupted` / `Failed`。
    /// `Interrupted` 与 `Completed` 共用 `AgentRunResult` 载荷，调用方走同一
    /// 持久化路径即可（T-004 / T-017）。
    /// Compatibility wrapper for callers whose list ends in exactly one user
    /// message that starts the turn. Call [`Self::run_turn`] when the caller
    /// already knows the precise boundary.
    pub async fn run(&mut self, initial_messages: Vec<ChatMessage>) -> AgentRunOutcome {
        let turn_start = initial_messages.len().saturating_sub(1);
        self.run_turn(initial_messages, turn_start).await
    }

    /// Runs one turn with the exact index at which its protected tail starts.
    ///
    /// The caller computes the boundary while appending the input. Establish it
    /// before steering is drained, so injected steering belongs to the same
    /// protected tail rather than changing how the boundary is inferred.
    pub async fn run_turn(
        &mut self,
        initial_messages: Vec<ChatMessage>,
        turn_start: usize,
    ) -> AgentRunOutcome {
        debug_assert!(
            initial_messages
                .iter()
                .all(|message| message.role != ChatMessageRole::System),
            "system prompt must be assembled only when constructing the LLM request"
        );
        debug_assert!(
            self.context_state
                .as_ref()
                .is_none_or(|state| state.messages.is_empty()),
            "ContextState.messages must be parked empty while AgentLoop owns the working list"
        );
        if !self.steering_queue.lock().is_open() {
            self.steering_queue.lock().open();
        }
        let mut messages = initial_messages;
        self.start_idx = turn_start.min(messages.len());
        let outcome = self.run_inner(&mut messages).await;
        self.steering_queue.lock().close_and_clear();
        self.park_messages(messages);
        outcome
    }

    async fn run_inner(&mut self, messages: &mut Vec<ChatMessage>) -> AgentRunOutcome {
        self.completion_guard_injections = 0;
        self.silent_reply_allowed = false;
        if self.cancel_token.is_cancelled() {
            // 入口兜底：token 已经被上一轮 cancel 但未重建，立即以空 partial 返回 Interrupted
            // 避免 chat_loop 误把"取消信号"传染给下一回合的正常输入。
            self.emit_event(AgentEvent::AgentStart);
            self.emit_event(AgentEvent::AgentEnd {
                messages: vec![],
                error: Some("interrupted".to_string()),
            });
            return AgentRunOutcome::Interrupted(AgentRunResult {
                final_text: String::new(),
                new_messages: Vec::new(),
            });
        }

        self.emit_event(AgentEvent::AgentStart);

        if let Err(err) = inject_steering_messages(self, messages) {
            self.emit_event(AgentEvent::AgentEnd {
                messages: vec![],
                error: Some(err.to_string()),
            });
            return AgentRunOutcome::Failed(err);
        }

        loop {
            match run_reasoning_loop(self, messages).await {
                Ok(final_text) => {
                    let new_messages = messages[self.start_idx..].to_vec();
                    let result = AgentRunResult {
                        final_text: final_text.clone(),
                        new_messages,
                    };

                    if self.reasoning_turn_budget_exhausted {
                        self.hand_back_unfinished_plan("tool_round_budget").await;
                        self.emit_event(AgentEvent::AgentEnd {
                            messages: vec![],
                            error: None,
                        });
                        return AgentRunOutcome::Completed(result);
                    }

                    if self.follow_up_queue.lock().is_empty()
                        && self.steering_queue.lock().close_if_empty()
                    {
                        self.hand_back_unfinished_plan("run_ended_while_executing")
                            .await;
                        self.emit_event(AgentEvent::AgentEnd {
                            messages: vec![],
                            error: None,
                        });
                        return AgentRunOutcome::Completed(result);
                    }
                    self.emit_event(AgentEvent::AgentEnd {
                        messages: vec![],
                        error: None,
                    });
                    if let Err(err) = inject_steering_messages(self, messages)
                        .and_then(|_| inject_follow_up_messages(self, messages))
                    {
                        return AgentRunOutcome::Failed(err);
                    }
                    continue;
                }
                Err(LoopError::Aborted {
                    partial_text,
                    partial_messages,
                }) => return self.terminate_interrupted(messages, partial_text, partial_messages),
                Err(LoopError::Fatal(e)) => {
                    self.emit_event(AgentEvent::AgentEnd {
                        messages: vec![],
                        error: Some(e.to_string()),
                    });
                    return AgentRunOutcome::Failed(e);
                }
                Err(LoopError::Retryable(_)) => {
                    unreachable!("Retryable must be handled inside the request retry loop")
                }
            }
        }
    }

    fn park_messages(&mut self, messages: Vec<ChatMessage>) {
        if let Some(context_state) = self.context_state.as_mut() {
            context_state.messages = messages;
        }
    }

    async fn hand_back_unfinished_plan(&self, reason: &str) {
        hand_back_unfinished_plan_if_needed(
            self.config.subagent_type.is_root(),
            self.config.plan_runtime.as_ref(),
            reason,
        )
        .await;
    }
}

async fn hand_back_unfinished_plan_if_needed(
    is_root_agent: bool,
    runtime: Option<&std::sync::Arc<crate::core::plan_runtime::PlanRuntime>>,
    reason: &str,
) {
    if !is_root_agent {
        return;
    }
    let Some(runtime) = runtime else {
        return;
    };
    let Some(path) = runtime.active_plan_path() else {
        return;
    };
    let Ok(plan) = crate::core::plan_runtime::file_store::read_plan(&path) else {
        return;
    };
    if matches!(
        runtime.next_action(&plan.frontmatter).await,
        crate::core::plan_runtime::NextAction::Done
            | crate::core::plan_runtime::NextAction::HandOff { .. }
    ) {
        return;
    }
    if let Err(error) = runtime.hand_back_stalled_plan(reason, None) {
        warn!(%error, reason, "failed to hand back unfinished plan at run exit");
    }
}

impl AgentLoop {
    /// 终结 Conversation Loop 的 `Interrupted` 分支：先发独立 `Interrupted` 事件
    /// （T2-P0-007 引入的细分订阅点），再发兼容老订阅者的 `AgentEnd(error="interrupted")`，
    /// 最后封装 `AgentRunOutcome::Interrupted` 返回。
    fn terminate_interrupted(
        &mut self,
        messages: &mut Vec<ChatMessage>,
        partial_text: String,
        partial_messages: Vec<ChatMessage>,
    ) -> AgentRunOutcome {
        let mut repaired_tail = partial_messages;
        let added_tool_results = append_missing_interrupted_tool_results(&mut repaired_tail);
        for message in repaired_tail
            .iter()
            .skip(repaired_tail.len().saturating_sub(added_tool_results))
        {
            if let Err(error) = self.push_message(messages, message.clone()) {
                tracing::warn!(%error, "failed to persist interrupted tool-result repair");
                // The tool round is now incomplete in memory and on disk; the next turn
                // must reload (and heal) from the transcript before appending input.
                if let Some(ctx_state) = self.context_state.as_mut() {
                    ctx_state.hydrated = false;
                }
            } else if let Some(ctx_state) = self.context_state.as_mut() {
                ctx_state.on_message_appended(estimate_msg_chars(message));
            }
        }
        let new_messages = messages[self.start_idx.min(messages.len())..].to_vec();
        let tool_results_count = new_messages
            .iter()
            .filter(|m| m.role == ChatMessageRole::Tool)
            .count();
        let partial_text_len = partial_text.chars().count();
        if let Err(error) = self.persist_custom_entry_if_needed(serde_json::json!({
            "event": "agent.interrupted",
            "partial_text_len": partial_text_len,
            "tool_results_count": tool_results_count,
        })) {
            tracing::warn!(%error, "failed to persist agent interruption marker");
        }
        self.emit_event(AgentEvent::Interrupted {
            partial_text_len,
            tool_results_count,
        });
        self.emit_event(AgentEvent::AgentEnd {
            messages: vec![],
            error: Some("interrupted".to_string()),
        });
        AgentRunOutcome::Interrupted(AgentRunResult {
            final_text: partial_text,
            new_messages,
        })
    }
}

#[cfg(test)]
#[path = "tests/run_test.rs"]
mod tests;
