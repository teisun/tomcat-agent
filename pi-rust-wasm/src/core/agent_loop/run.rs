use std::sync::Arc;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::core::compaction::run_layer0_cleanup;
use crate::core::llm::{ChatMessage, ChatMessageRole, ChatRequest, LlmProvider};
use crate::core::primitives::PrimitiveExecutor;
use crate::core::session::manager::{estimated_tokens_from_chars, ContextState};
use crate::infra::error::AppError;
use crate::infra::event_bus::{EventBus, EventContext};
use crate::infra::events::{AgentEvent, ExtensionEvent, Message};

use super::error_classifier::handle_overflow_retry;
use super::types::{
    unix_ts_ms, AgentLoop, AgentLoopConfig, AgentRunOutcome, AgentRunResult, LoopError,
    ToolCallInfo,
};
use super::{stream_handler, tool_dispatcher};

impl AgentLoop {
    pub fn new(
        llm: Arc<dyn LlmProvider>,
        primitive: Arc<dyn PrimitiveExecutor>,
        event_bus: Arc<dyn EventBus>,
        config: AgentLoopConfig,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            llm,
            primitive,
            event_bus,
            config,
            steering_queue: Arc::new(Mutex::new(Vec::new())),
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
            cancel_token,
            context_state: None,
            block_tool_calls: false,
            start_idx: 0,
            context_tail_start: 0,
        }
    }

    /// 测试用：注入 steering_queue，便于 mock 在工具执行中推入 steering 消息。
    #[cfg(test)]
    pub fn new_with_steering_queue(
        llm: Arc<dyn LlmProvider>,
        primitive: Arc<dyn PrimitiveExecutor>,
        event_bus: Arc<dyn EventBus>,
        config: AgentLoopConfig,
        cancel_token: CancellationToken,
        steering_queue: Arc<Mutex<Vec<ChatMessage>>>,
    ) -> Self {
        Self {
            llm,
            primitive,
            event_bus,
            config,
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
            steering_queue,
            cancel_token,
            context_state: None,
            block_tool_calls: false,
            start_idx: 0,
            context_tail_start: 0,
        }
    }

    pub fn steer(&self, msg: String) {
        self.steering_queue.lock().push(ChatMessage::steering(msg));
    }

    pub fn follow_up(&self, msg: String) {
        self.follow_up_queue.lock().push(ChatMessage::user(msg));
    }

    /// 触发本次 `run` 的取消。幂等且不可逆——调用方需在下一回合前
    /// `new(...)` 时传入新的 `CancellationToken`。
    pub fn abort(&self) {
        self.cancel_token.cancel();
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    pub fn set_context_state(&mut self, state: Option<ContextState>) {
        self.context_state = state;
    }

    pub fn take_context_state(&mut self) -> Option<ContextState> {
        self.context_state.take()
    }

    /// 刷新实时 token 指标并发射 ContextMetricsUpdate 事件（仅当 context_state 存在时）。
    pub(super) fn emit_context_metrics(&mut self) {
        if let Some(ref mut ctx_state) = self.context_state {
            let input_tokens_used = ctx_state.estimated_token_count();
            let context_utilization_ratio = ctx_state.usage_ratio();
            let preheat_in_progress = ctx_state.preheat.is_warmup_task_active();
            let preheat_result_pending = ctx_state.preheat.preheat_result_pending();
            ctx_state.live.input_tokens_used = input_tokens_used;
            ctx_state.live.context_utilization_ratio = context_utilization_ratio;
            ctx_state.live.preheat_in_progress = preheat_in_progress;
            ctx_state.live.preheat_result_pending = preheat_result_pending && !preheat_in_progress;
        }
        if let Some(ref ctx_state) = self.context_state {
            self.emit_event(AgentEvent::ContextMetricsUpdate {
                input_tokens_used: ctx_state.live.input_tokens_used,
                context_utilization_ratio: ctx_state.live.context_utilization_ratio,
                compaction_count: ctx_state.session_obs.compaction_count,
                compaction_tokens_freed: ctx_state.session_obs.compaction_tokens_freed,
                total_tool_result_bytes_persisted: ctx_state
                    .session_obs
                    .tool_result_chars_persisted,
                preheat_in_progress: ctx_state.live.preheat_in_progress,
                preheat_result_pending: ctx_state.live.preheat_result_pending,
            });
        }
    }

    pub(super) fn emit_event(&self, event: AgentEvent) {
        let payload = serde_json::to_value(&event).unwrap_or(serde_json::Value::Null);
        let event_name = payload
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let ctx = EventContext::new(event_name.clone(), payload);
        let _ = self.event_bus.emit_sync(&event_name, ctx);
    }

    pub(super) fn emit_extension_event(&self, event: ExtensionEvent) {
        let payload = serde_json::to_value(&event).unwrap_or(serde_json::Value::Null);
        let event_name = payload
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let ctx = EventContext::new(event_name.clone(), payload);
        let _ = self.event_bus.emit_sync(&event_name, ctx);
    }

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
                }) => {
                    let session_id = self.config.session_id.clone();
                    let tool_results_count = partial_messages
                        .iter()
                        .filter(|m| m.role == ChatMessageRole::Tool)
                        .count();
                    let partial_text_len = partial_text.chars().count();

                    // 先发布独立的 Interrupted 事件（新增）再发布原有的 AgentEnd(interrupted)
                    // 供订阅者做"失败 vs 用户中断"区分；旧订阅者仍然拿到 AgentEnd。
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

                    // 中断时 partial_messages 可能不包括发给 LLM 时 context_tail_start 之前
                    // 的历史消息；外层只需 `new_messages`，故直接透传即可。
                    return AgentRunOutcome::Interrupted(AgentRunResult {
                        final_text: partial_text,
                        new_messages: partial_messages,
                    });
                }
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

            match self.run_reasoning_loop(messages).await {
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

    /// 构造 Aborted 错误：
    ///
    /// - `partial_text` 是本轮 assistant 流**已收到**的 delta 拼接
    ///   （包含将要作为 partial assistant 写入 messages 的文本）；
    /// - `partial_messages` 取 `messages[start_idx..]`——这是本轮新增的全部消息，
    ///   既包含中断前已完成的 tool_result，也包含即将作为 partial 写入的
    ///   assistant 消息（调用方在进入本函数前已 `push` 到 messages）。
    pub(super) fn make_aborted(
        &self,
        messages: &[ChatMessage],
        partial_text: String,
    ) -> LoopError {
        LoopError::Aborted {
            partial_text,
            partial_messages: messages[self.start_idx..].to_vec(),
        }
    }

    /// 第三层：Reasoning loop，LLM 流式 + 工具执行 + Steering/Abort 检查。
    async fn run_reasoning_loop(
        &mut self,
        messages: &mut Vec<ChatMessage>,
    ) -> Result<String, LoopError> {
        let mut final_text = String::new();
        let mut turn_index: usize = 0;

        loop {
            if self.cancel_token.is_cancelled() {
                return Err(self.make_aborted(messages, final_text));
            }

            turn_index += 1;
            self.emit_event(AgentEvent::TurnStart {
                session_id: self.config.session_id.clone(),
                turn_index,
                timestamp: unix_ts_ms(),
            });

            let req = ChatRequest {
                messages: messages.clone(),
                model: self.config.model.clone(),
                temperature: None,
                max_tokens: None,
                stream: Some(true),
                model_override: None,
                tools: Some(self.config.tool_definitions.clone()),
            };

            // context_metrics_update：单次 run_reasoning_loop 内仅在首次 LLM 请求前发一次（中间 tool round 不发）。
            if turn_index == 1 {
                self.emit_context_metrics();
                if let Some(ref ctx_state) = self.context_state {
                    info!(
                        target: "pi_wasm_chat_diag",
                        phase = "emit_context_metrics_turn1",
                        turn_index,
                        input_tokens_used = ctx_state.live.input_tokens_used,
                        context_utilization_ratio = ctx_state.live.context_utilization_ratio,
                        compaction_count = ctx_state.session_obs.compaction_count
                    );
                }
            }

            // Stream 消费（含 LLM connect + MessageStart/Update/End 发射 + cancel 抢占）
            // 整块委托给 stream_handler::run_chat_stream；aborted / Err 路径均已
            // 先发 MessageEnd，调用方仅需补 partial assistant 落盘与 make_aborted。
            let outcome = stream_handler::run_chat_stream(self, req).await?;
            let content_buf = outcome.content_buf;

            // stream 被取消：把 partial content_buf 作为 partial assistant 落到 messages，
            // 让 ctx_state 也把它计入消息预算；再返回 Aborted 携带 partial。
            if outcome.aborted {
                if !content_buf.is_empty() {
                    if let Some(ref mut ctx_state) = self.context_state {
                        ctx_state.on_message_appended(content_buf.len());
                    }
                    messages.push(ChatMessage::assistant(&content_buf));
                    final_text.push_str(&content_buf);
                }
                return Err(self.make_aborted(messages, final_text));
            }

            final_text.push_str(&content_buf);

            let tool_calls: Vec<ToolCallInfo> = outcome
                .tool_calls_buf
                .into_iter()
                .filter(|tc| !tc.name.is_empty())
                .map(|tc| ToolCallInfo {
                    id: tc.id,
                    name: tc.name,
                    arguments: tc.arguments,
                })
                .collect();

            if tool_calls.is_empty() {
                if let Some(ref mut ctx_state) = self.context_state {
                    ctx_state.on_message_appended(content_buf.len());
                }
                messages.push(ChatMessage::assistant(&content_buf));

                // Timing ⑤: L0 → try_restart → check_after_reply → try_start → metrics
                let mut preheat_started: Option<(usize, f64)> = None;
                let mut layer0_release: Option<(usize, usize)> = None;
                if let Some(ref mut ctx_state) = self.context_state {
                    // Step 1: L0 cleanup
                    let l0 = run_layer0_cleanup(
                        ctx_state,
                        &self.config.context_config,
                        std::path::Path::new(&self.config.work_dir),
                        &self.config.session_id,
                    );
                    for pr in &l0.persisted {
                        ctx_state.session_obs.tool_result_chars_persisted += pr.original_chars;
                    }
                    let persist_tok = estimated_tokens_from_chars(l0.persist_chars_freed);
                    let placeholder_tok = estimated_tokens_from_chars(l0.placeholder_chars_freed);
                    if persist_tok > 0 || placeholder_tok > 0 {
                        ctx_state.session_obs.compaction_tokens_freed +=
                            persist_tok + placeholder_tok;
                        layer0_release = Some((persist_tok, placeholder_tok));
                    }

                    // Step 2: restore ExhaustedPending → Running
                    ctx_state.preheat.try_restart_if_pending(
                        ctx_state.usage_ratio(),
                        &ctx_state.messages,
                        &ctx_state.transcript_path,
                        Arc::clone(&self.llm),
                        &self.config.context_config,
                        Arc::clone(&self.event_bus),
                    );

                    // Step 3: L2 non-blocking poll + apply boundary
                    if ctx_state.usage_ratio() >= 0.85 {
                        crate::core::compaction::apply::check_after_reply(
                            ctx_state,
                            &*self.event_bus,
                        );
                    }

                    // Step 4: Idle → Running (start new preheat if conditions met)
                    let ratio = ctx_state.usage_ratio();
                    let turn_count = ctx_state.turn_count();
                    if ctx_state.preheat.try_start(
                        ratio,
                        &ctx_state.messages,
                        &ctx_state.transcript_path,
                        Arc::clone(&self.llm),
                        &self.config.context_config,
                        Arc::clone(&self.event_bus),
                    ) {
                        preheat_started = Some((turn_count, ratio));
                    }
                }
                if let Some((p, ph)) = layer0_release {
                    self.emit_event(AgentEvent::Layer0ContextRelease {
                        persist_tokens_freed: p,
                        placeholder_tokens_freed: ph,
                    });
                }
                if let Some((covered_count, ratio_before)) = preheat_started {
                    self.emit_event(AgentEvent::AutoCompactionStart {
                        covered_count,
                        ratio_before,
                    });
                }

                self.emit_context_metrics();
                self.emit_event(AgentEvent::TurnEnd {
                    session_id: self.config.session_id.clone(),
                    turn_index,
                    message: Message(serde_json::json!({})),
                    tool_results: vec![],
                });
                return Ok(final_text);
            }

            // tool_calls 调度（block / steering / cancel / 事件配对 / 计费 / push）
            // 整块委托给 tool_dispatcher::run_tool_calls；函数内部严格保持原事件顺序：
            // ToolExecutionStart → ExtensionEvent::ToolCall → execute_tool →
            // ExtensionEvent::ToolResult → ToolExecutionEnd；cancel 抢占点均保留
            // "先发 End 让 UI 配对再 make_aborted" 的原语义。
            let dispatch = tool_dispatcher::run_tool_calls(
                self,
                messages,
                &tool_calls,
                &content_buf,
                &final_text,
            )
            .await?;

            // No synchronous cascade here; L0/L1/L2 handled at timing ⑤

            self.emit_event(AgentEvent::TurnEnd {
                session_id: self.config.session_id.clone(),
                turn_index,
                message: Message(serde_json::json!({})),
                tool_results: dispatch.tool_results,
            });

            if dispatch.steered {
                continue;
            }

            if turn_index >= self.config.max_tool_rounds {
                self.emit_context_metrics();
                return Ok(final_text);
            }
        }
    }
}
