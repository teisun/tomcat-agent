//! # Agent Loop 顶层骨架（Conversation + Attempt 两层）
//!
//! 本文件只承载两层调度骨架；Reasoning Loop（第三层）抽到 `reasoning_loop.rs`，
//! 让 `run.rs` 满足 [RUST_FILE_LINES_SPEC §A](../../../docs/openspec/specs/guides/coding/RUST_FILE_LINES_SPEC.md)
//! 的 300 行硬上限。`mod.rs` 已有三层全景大图，本图只画 **本文件持有的两层** + 与
//! 同族子模块的协作箭头，避免与上层 doc 重复。
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────────────────┐
//! │  AgentLoop::run(initial_messages)              ← 调用方：api::chat     │
//! └────────────────────────────────────────────────────────────────────────┘
//!    │  ① 入口三检：cancel_token.is_cancelled? │ steering_queue 注入 │
//!    │              start_idx / context_tail_start 标记
//!    ▼
//! ┌── 第一层 Conversation Loop  (本文件 AgentLoop::run) ────────────────────┐
//! │  emit AgentStart                                                         │
//! │  loop {                                                                  │
//! │    ┌────────────────────────────────────────────────────────────────┐   │
//! │    │  第二层 Attempt Loop  (本文件 AgentLoop::run_attempt_loop)     │   │
//! │    │  for attempt in 1..=max_attempts:                              │   │
//! │    │    if attempt > 1:                                             │   │
//! │    │      emit AutoRetryStart(attempt, delay=base*2^(n-1))          │   │
//! │    │      tokio::select! { cancel ► Aborted │ sleep ► continue }    │   │
//! │    │                                                                │   │
//! │    │    ┌── reasoning_loop::run_reasoning_loop(self, &mut msgs) ──┐ │   │
//! │    │    │      (← 第三层骨架在 reasoning_loop.rs)                 │ │   │
//! │    │    │   委托：stream_handler / tool_dispatcher / tool_exec    │ │   │
//! │    │    │         / turn_finalize（timing ⑤）                    │ │   │
//! │    │    └─────────────────────────────────────────────────────────┘ │   │
//! │    │       │                                                        │   │
//! │    │       ├ Ok(text)        ── attempt>1 ► emit AutoRetryEnd(ok)   │   │
//! │    │       ├ Err(Aborted)    ── 透传给第一层                        │   │
//! │    │       ├ Err(Fatal)      ── attempt>1 ► emit AutoRetryEnd(err)  │   │
//! │    │       └ Err(Retryable)  ── error_classifier::handle_overflow   │   │
//! │    │                            _retry（L3 trim + 重建 messages）   │   │
//! │    └────────────────────────────────────────────────────────────────┘   │
//! │                                                                          │
//! │    Ok(text) ► emit AgentEnd(ok)                                          │
//! │              ► follow_up_queue 非空 ► drain ► continue                   │
//! │              ► 否则 ► return Completed(AgentRunResult)                   │
//! │    Err(Aborted)  ► terminate_interrupted ► emit Interrupted + AgentEnd   │
//! │                                            ► return Interrupted          │
//! │    Err(Fatal)    ► emit AgentEnd(error)   ► return Failed                │
//! │  }                                                                       │
//! └──────────────────────────────────────────────────────────────────────────┘
//!    │
//!    ▼ 三态出口
//!   AgentRunOutcome::{ Completed | Interrupted | Failed }
//! ```
//!
//! ## 与同族子模块的边界
//!
//! - **本文件**：只做 Conversation/Attempt 的"何时调用 / 错误归并 / 三态收口"。
//! - `accessors.rs`：`new` / `steer` / `follow_up` / `abort` / `emit_*` / `make_aborted`。
//! - `reasoning_loop.rs`：单 turn 的 LLM↔工具调度（第三层骨架）。
//! - `error_classifier.rs`：`classify_error` + `handle_overflow_retry`（L3 截断）。
//! - `stream_handler.rs` / `tool_dispatcher.rs` / `tool_exec.rs` / `turn_finalize.rs`：
//!   分别负责流消费 / 工具调度 / 工具执行 / text-only 收束。

use crate::core::llm::{ChatMessage, ChatMessageRole};
use crate::core::session::{
    find_dangling_tail_tool_call_ids, manager::INTERRUPTED_TOOL_RESULT_TEXT,
};
use crate::infra::config::DEFAULT_AGENT_MAX_ATTEMPTS;
use crate::infra::error::{
    is_retryable_llm_error, is_unsupported_multimodal_text, llm_http_status, llm_retry_after_ms,
    llm_stage, llm_summary, AppError,
};
use crate::infra::events::{wire, AgentEvent};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

use super::error_classifier::{handle_overflow_retry, handle_unsupported_multimodal_retry};
use super::reasoning_loop::run_reasoning_loop;
use super::steering_injection::inject_steering_messages;
use super::types::{AgentLoop, AgentRunOutcome, AgentRunResult, LoopError};

/// 自动无人执行可遇到短暂限流/网络抖动；上限 30 秒既尊重服务端恢复窗口，
/// 又始终可被取消，不会把 Agent 卡成不可交互。
const INTERACTIVE_RETRY_DELAY_CAP_MS: u64 = 8_000;
const UNATTENDED_RETRY_DELAY_CAP_MS: u64 = 30_000;
/// PLAN/EXEC 无人值守时，临时传输故障与限流获得的总请求预算（含第一次请求）。
const UNATTENDED_TRANSIENT_MAX_ATTEMPTS: u32 = 10;

fn retry_attempt_budget(unattended_execution: bool, interactive_max_attempts: u32) -> u32 {
    if unattended_execution {
        UNATTENDED_TRANSIENT_MAX_ATTEMPTS
    } else {
        interactive_max_attempts
    }
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod retry_budget_tests {
    use super::*;

    #[test]
    fn unattended_mode_allows_ten_attempts() {
        assert_eq!(
            retry_attempt_budget(true, DEFAULT_AGENT_MAX_ATTEMPTS),
            UNATTENDED_TRANSIENT_MAX_ATTEMPTS
        );
        assert_eq!(
            retry_attempt_budget(false, DEFAULT_AGENT_MAX_ATTEMPTS),
            DEFAULT_AGENT_MAX_ATTEMPTS
        );
        assert_eq!(
            compute_retry_delay_with_cap_ms(500, 20, 40, UNATTENDED_RETRY_DELAY_CAP_MS),
            UNATTENDED_RETRY_DELAY_CAP_MS
        );
    }
}

#[cfg(test)]
pub(super) fn compute_retry_delay_ms(base_delay_ms: u64, attempt: u32, jitter_seed: u64) -> u64 {
    compute_retry_delay_with_cap_ms(
        base_delay_ms,
        attempt,
        jitter_seed,
        INTERACTIVE_RETRY_DELAY_CAP_MS,
    )
}

fn compute_retry_delay_with_cap_ms(
    base_delay_ms: u64,
    attempt: u32,
    jitter_seed: u64,
    cap_ms: u64,
) -> u64 {
    if attempt <= 1 || base_delay_ms == 0 {
        return 0;
    }
    let exp = base_delay_ms.saturating_mul(2u64.saturating_pow(attempt.saturating_sub(2)));
    let jitter_pct = 80 + (jitter_seed % 41);
    (exp.saturating_mul(jitter_pct) / 100).min(cap_ms)
}

fn next_retry_delay_ms(base_delay_ms: u64, attempt: u32, cap_ms: u64) -> u64 {
    let jitter_seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()))
        .unwrap_or(20);
    compute_retry_delay_with_cap_ms(base_delay_ms, attempt, jitter_seed, cap_ms)
}

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
    pub async fn run(&mut self, initial_messages: Vec<ChatMessage>) -> AgentRunOutcome {
        self.completion_guard_injections = 0;
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

        let mut messages = initial_messages;

        if let Err(err) = inject_steering_messages(self, &mut messages) {
            self.emit_event(AgentEvent::AgentEnd {
                messages: vec![],
                error: Some(err.to_string()),
            });
            return AgentRunOutcome::Failed(err);
        }

        self.context_tail_start = match messages.last() {
            Some(m) if m.role == ChatMessageRole::User => messages.len().saturating_sub(1),
            _ => messages.len(),
        };

        self.start_idx = self.context_tail_start;

        loop {
            match self.run_attempt_loop(&mut messages).await {
                Ok(final_text) => {
                    let new_messages = messages[self.start_idx..].to_vec();
                    let result = AgentRunResult {
                        final_text: final_text.clone(),
                        new_messages,
                    };
                    self.emit_event(AgentEvent::AgentEnd {
                        messages: vec![],
                        error: None,
                    });

                    if self.reasoning_turn_budget_exhausted {
                        self.sync_persisted_messages_into_context(&result.new_messages);
                        return AgentRunOutcome::Completed(result);
                    }

                    let mut q = self.follow_up_queue.lock();
                    if q.is_empty() {
                        drop(q);
                        self.sync_persisted_messages_into_context(&result.new_messages);
                        return AgentRunOutcome::Completed(result);
                    }
                    let drained: Vec<_> = q.drain(..).collect();
                    drop(q);
                    for msg in drained {
                        if let Err(err) = self.push_message(&mut messages, msg) {
                            return AgentRunOutcome::Failed(err);
                        }
                    }
                    continue;
                }
                Err(LoopError::Aborted {
                    partial_text,
                    partial_messages,
                }) => return self.terminate_interrupted(partial_text, partial_messages),
                Err(LoopError::Fatal(e)) => {
                    let new_messages = messages[self.start_idx..].to_vec();
                    self.sync_persisted_messages_into_context(&new_messages);
                    self.emit_event(AgentEvent::AgentEnd {
                        messages: vec![],
                        error: Some(e.to_string()),
                    });
                    return AgentRunOutcome::Failed(e);
                }
                Err(LoopError::Retryable(_)) => {
                    unreachable!("Retryable 应在 run_attempt_loop 内部处理")
                }
            }
        }
    }

    /// 终结 Conversation Loop 的 `Interrupted` 分支：先发独立 `Interrupted` 事件
    /// （T2-P0-007 引入的细分订阅点），再发兼容老订阅者的 `AgentEnd(error="interrupted")`，
    /// 最后封装 `AgentRunOutcome::Interrupted` 返回。
    fn terminate_interrupted(
        &mut self,
        partial_text: String,
        mut partial_messages: Vec<ChatMessage>,
    ) -> AgentRunOutcome {
        let added_tool_results = append_missing_interrupted_tool_results(&mut partial_messages);
        if added_tool_results > 0 {
            if let Some(ref mut ctx_state) = self.context_state {
                for _ in 0..added_tool_results {
                    ctx_state.on_message_appended(INTERRUPTED_TOOL_RESULT_TEXT.len());
                }
            }
        }
        self.sync_persisted_messages_into_context(&partial_messages);
        let tool_results_count = partial_messages
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
            new_messages: partial_messages,
        })
    }

    /// 第二层：Attempt loop，错误分类与指数退避重试。
    async fn run_attempt_loop(
        &mut self,
        messages: &mut Vec<ChatMessage>,
    ) -> Result<String, LoopError> {
        let mut last_err: Option<AppError> = None;
        let mut unsupported_multimodal_hits = 0u32;
        let unattended_execution = self.config.unattended_retry
            || self
                .config
                .plan_runtime
                .as_ref()
                .and_then(|runtime| runtime.executing_plan_id())
                .is_some();
        let max_attempts = retry_attempt_budget(unattended_execution, self.config.max_attempts);
        let retry_delay_cap_ms = if unattended_execution {
            UNATTENDED_RETRY_DELAY_CAP_MS
        } else {
            INTERACTIVE_RETRY_DELAY_CAP_MS
        };
        for attempt in 1..=max_attempts {
            if attempt > 1 {
                let delay_ms = last_err
                    .as_ref()
                    .and_then(llm_retry_after_ms)
                    .unwrap_or_else(|| {
                        next_retry_delay_ms(
                            self.config.retry_base_delay_ms,
                            attempt,
                            retry_delay_cap_ms,
                        )
                    });
                let err_msg = last_err
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "retry".to_string());
                self.emit_event(AgentEvent::AutoRetryStart {
                    attempt,
                    max_attempts,
                    delay_ms,
                    error_message: err_msg,
                });
                if let Err(error) = self.persist_custom_entry_if_needed(serde_json::json!({
                    "event": wire::WIRE_AUTO_RETRY_START,
                    "attempt": attempt,
                    "max_attempts": max_attempts,
                    "delay_ms": delay_ms,
                    "error_message": last_err.as_ref().map(ToString::to_string),
                })) {
                    warn!(error = %error, "auto retry start transcript write failed");
                }
                // Sleep 期间也要响应取消，不然 Ctrl+C 会被"3 秒退避"吃掉
                let cancel = self.cancel_token.clone();
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        return Err(LoopError::Aborted {
                            partial_text: String::new(),
                            partial_messages: messages[self.start_idx..].to_vec(),
                        });
                    }
                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)) => {}
                }
            }

            // 正常的 stream/tool await 会在内部优雅地处理取消，以便保留 partial
            // assistant 与已完成的 tool result。此处是结构性兜底：后续新增的
            // 非流式 await（例如 collapse 摘要）即使遗漏了 cancel race，也不能
            // 让整个 turn 等到该操作自然结束。
            let reasoning_result = {
                let cancel = self.cancel_token.clone();
                tokio::select! {
                    biased;
                    result = run_reasoning_loop(self, messages, attempt, max_attempts) => Some(result),
                    _ = cancel.cancelled() => None,
                }
            };
            let reasoning_result = reasoning_result.unwrap_or_else(|| {
                Err(LoopError::Aborted {
                    partial_text: String::new(),
                    partial_messages: messages[self.start_idx..].to_vec(),
                })
            });

            match reasoning_result {
                Ok(text) => {
                    if attempt > 1 {
                        self.emit_event(AgentEvent::AutoRetryEnd {
                            success: true,
                            attempt,
                            final_error: None,
                        });
                        if let Err(error) = self.persist_custom_entry_if_needed(serde_json::json!({
                            "event": wire::WIRE_AUTO_RETRY_END,
                            "success": true,
                            "attempt": attempt,
                            "final_error": serde_json::Value::Null,
                        })) {
                            warn!(error = %error, "auto retry end transcript write failed");
                        }
                    }
                    return Ok(text);
                }
                Err(err @ LoopError::Aborted { .. }) => return Err(err),
                Err(LoopError::Fatal(e)) => {
                    let err_text = e.to_string();
                    if attempt > 1 {
                        self.emit_event(AgentEvent::AutoRetryEnd {
                            success: false,
                            attempt,
                            final_error: Some(err_text),
                        });
                        if let Err(error) = self.persist_custom_entry_if_needed(serde_json::json!({
                            "event": wire::WIRE_AUTO_RETRY_END,
                            "success": false,
                            "attempt": attempt,
                            "final_error": e.to_string(),
                        })) {
                            warn!(error = %error, "auto retry end transcript write failed");
                        }
                    }
                    return Err(LoopError::Fatal(e));
                }
                Err(LoopError::Retryable(e)) => {
                    let elevated_retry_kind = is_retryable_llm_error(&e);
                    // PLAN/EXEC 的 10 次预算只留给真实临时传输故障与限流；
                    // 其他可恢复类别仍保留交互档 4 次，避免长时间重试一个不会自愈的问题。
                    if unattended_execution
                        && !elevated_retry_kind
                        && attempt >= DEFAULT_AGENT_MAX_ATTEMPTS
                    {
                        return Err(LoopError::Fatal(e));
                    }
                    // L3 overflow trim 与诊断日志统一放在 error_classifier 中处理；
                    // retry 控制流（last_err / max_attempts 判定）仍由本函数持有，
                    // 保证"谁拥有 attempt 循环谁决定终止"。
                    let overflow_hit = crate::infra::error::is_context_overflow(&e);
                    let overflow_stats = handle_overflow_retry(self, messages, attempt, &e).await;
                    // ContextOverflow 只能在 payload 确实缩小后才允许下一次请求。
                    // 否则每次都会发送同一份超限上下文，消耗完 attempt 后才给用户一个
                    // 更晚、更难理解的错误。
                    if overflow_hit && !overflow_stats.applied {
                        tracing::warn!(
                            target: "tomcat_chat_diag",
                            phase = "context_overflow_no_progress",
                            attempt,
                            "context overflow recovery did not shrink the payload; stop retrying"
                        );
                        return Err(LoopError::Fatal(e));
                    }
                    let unsupported_multimodal_hit = matches!(&e, AppError::LlmDetailed(_))
                        && llm_stage(&e).is_none()
                        && llm_http_status(&e).is_none()
                        && llm_summary(&e)
                            .as_deref()
                            .is_some_and(is_unsupported_multimodal_text);
                    if unsupported_multimodal_hit {
                        unsupported_multimodal_hits = unsupported_multimodal_hits.saturating_add(1);
                        let attempts_left_after_this = max_attempts.saturating_sub(attempt);
                        if unsupported_multimodal_hits >= 2 && attempts_left_after_this >= 1 {
                            let _stats =
                                handle_unsupported_multimodal_retry(self, messages, attempt, &e);
                        }
                    } else {
                        unsupported_multimodal_hits = 0;
                    }
                    last_err = Some(e);
                    if attempt == max_attempts {
                        let fatal = last_err
                            .take()
                            .unwrap_or_else(|| AppError::Llm("重试耗尽".to_string()));
                        let final_error = fatal.to_string();
                        self.emit_event(AgentEvent::AutoRetryEnd {
                            success: false,
                            attempt,
                            final_error: Some(final_error.clone()),
                        });
                        if let Err(error) = self.persist_custom_entry_if_needed(serde_json::json!({
                            "event": wire::WIRE_AUTO_RETRY_END,
                            "success": false,
                            "attempt": attempt,
                            "final_error": final_error,
                        })) {
                            warn!(error = %error, "auto retry end transcript write failed");
                        }
                        return Err(LoopError::Fatal(fatal));
                    }
                }
            }
        }
        Err(LoopError::Fatal(
            last_err.unwrap_or_else(|| AppError::Llm("重试耗尽".to_string())),
        ))
    }
}
