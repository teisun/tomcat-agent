//! # Agent Loop 顶层骨架（Conversation + Attempt 两层）
//!
//! 本文件只承载两层调度骨架：
//!
//! | 层 | 函数 | 职责 |
//! |---|---|---|
//! | Conversation | [`AgentLoop::run`] | AgentStart/End 生命周期 + steering 注入 + follow_up loop + Aborted/Fatal 终结 |
//! | Attempt | `run_attempt_loop` | Retryable 指数退避 + L3 trim 路由 + AutoRetryStart/End |
//!
//! Reasoning Loop（第三层）抽到 `reasoning_loop.rs` 作为 `pub(super)` 自由函数，
//! 以满足 [RUST_FILE_LINES_SPEC §A](../../../openspec/specs/guides/coding/RUST_FILE_LINES_SPEC.md)
//! 的 300 行硬上限。其余具体动作分布在：访问器 / emit / make_aborted →
//! `accessors.rs`；错误分类与 overflow 回收 → `error_classifier.rs`；流消费 →
//! `stream_handler.rs`；工具执行 → `tool_exec.rs`；工具调度 →
//! `tool_dispatcher.rs`；text-only 收束 → `turn_finalize.rs`。

use crate::core::llm::{ChatMessage, ChatMessageRole};
use crate::infra::error::AppError;
use crate::infra::events::AgentEvent;

use super::error_classifier::handle_overflow_retry;
use super::reasoning_loop::run_reasoning_loop;
use super::types::{AgentLoop, AgentRunOutcome, AgentRunResult, LoopError};

impl AgentLoop {
    /// 第一层：Conversation loop，处理 FollowUp。
    ///
    /// 返回 `AgentRunOutcome` 三态：`Completed` / `Interrupted` / `Failed`。
    /// `Interrupted` 与 `Completed` 共用 `AgentRunResult` 载荷，调用方走同一
    /// 持久化路径即可（T-004 / T-017）。
    pub async fn run(&mut self, initial_messages: Vec<ChatMessage>) -> AgentRunOutcome {
        if self.cancel_token.is_cancelled() {
            // 入口兜底：token 已经被上一轮 cancel 但未重建，立即以空 partial 返回 Interrupted
            // 避免 chat_loop 误把"取消信号"传染给下一回合的正常输入。
            self.emit_event(AgentEvent::AgentStart {
                session_id: self.config.session_id.clone(),
            });
            self.emit_event(AgentEvent::AgentEnd {
                session_id: self.config.session_id.clone(),
                messages: vec![],
                error: Some("interrupted".to_string()),
            });
            return AgentRunOutcome::Interrupted(AgentRunResult {
                final_text: String::new(),
                new_messages: Vec::new(),
            });
        }

        self.emit_event(AgentEvent::AgentStart {
            session_id: self.config.session_id.clone(),
        });

        let mut messages = initial_messages;

        {
            let mut q = self.steering_queue.lock();
            if !q.is_empty() {
                messages.extend(q.drain(..));
            }
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
                        session_id: self.config.session_id.clone(),
                        messages: vec![],
                        error: None,
                    });

                    let mut q = self.follow_up_queue.lock();
                    if q.is_empty() {
                        return AgentRunOutcome::Completed(result);
                    }
                    messages.extend(q.drain(..));
                    continue;
                }
                Err(LoopError::Aborted {
                    partial_text,
                    partial_messages,
                }) => return self.terminate_interrupted(partial_text, partial_messages),
                Err(LoopError::Fatal(e)) => {
                    self.emit_event(AgentEvent::AgentEnd {
                        session_id: self.config.session_id.clone(),
                        messages: vec![],
                        error: Some(e.clone()),
                    });
                    return AgentRunOutcome::Failed(AppError::Llm(e));
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
        &self,
        partial_text: String,
        partial_messages: Vec<ChatMessage>,
    ) -> AgentRunOutcome {
        let session_id = self.config.session_id.clone();
        let tool_results_count = partial_messages
            .iter()
            .filter(|m| m.role == ChatMessageRole::Tool)
            .count();
        let partial_text_len = partial_text.chars().count();
        self.emit_event(AgentEvent::Interrupted {
            session_id: session_id.clone(),
            partial_text_len,
            tool_results_count,
        });
        self.emit_event(AgentEvent::AgentEnd {
            session_id,
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
        let mut last_err: Option<String> = None;
        for attempt in 1..=self.config.max_attempts {
            if attempt > 1 {
                let delay_ms = self.config.retry_base_delay_ms * 2u64.pow(attempt - 2);
                let err_msg = last_err.clone().unwrap_or_else(|| "retry".to_string());
                self.emit_event(AgentEvent::AutoRetryStart {
                    attempt,
                    max_attempts: self.config.max_attempts,
                    delay_ms,
                    error_message: err_msg,
                });
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

            match run_reasoning_loop(self, messages).await {
                Ok(text) => {
                    if attempt > 1 {
                        self.emit_event(AgentEvent::AutoRetryEnd {
                            success: true,
                            attempt,
                            final_error: None,
                        });
                    }
                    return Ok(text);
                }
                Err(err @ LoopError::Aborted { .. }) => return Err(err),
                Err(LoopError::Fatal(e)) => {
                    if attempt > 1 {
                        self.emit_event(AgentEvent::AutoRetryEnd {
                            success: false,
                            attempt,
                            final_error: Some(e.clone()),
                        });
                    }
                    return Err(LoopError::Fatal(e));
                }
                Err(LoopError::Retryable(e)) => {
                    // L3 overflow trim 与诊断日志统一放在 error_classifier 中处理；
                    // retry 控制流（last_err / max_attempts 判定）仍由本函数持有，
                    // 保证"谁拥有 attempt 循环谁决定终止"。
                    let _stats = handle_overflow_retry(self, messages, attempt, &e);
                    last_err = Some(e);
                    if attempt == self.config.max_attempts {
                        let fatal = last_err.unwrap_or_else(|| "重试耗尽".to_string());
                        self.emit_event(AgentEvent::AutoRetryEnd {
                            success: false,
                            attempt,
                            final_error: Some(fatal.clone()),
                        });
                        return Err(LoopError::Fatal(fatal));
                    }
                }
            }
        }
        Err(LoopError::Fatal(
            last_err.unwrap_or_else(|| "重试耗尽".to_string()),
        ))
    }
}
