use std::sync::Arc;

use parking_lot::Mutex;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::core::compaction::{
    force_drop_oldest_to_target, is_context_overflow_error, run_layer0_cleanup,
};
use crate::core::llm::{ChatMessage, ChatMessageRole, ChatRequest, LlmProvider, StreamEvent};
use crate::core::primitives::{EditOperation, EditOperationType, PrimitiveExecutor};
use crate::core::session::manager::{estimated_tokens_from_chars, ContextState};
use crate::infra::error::AppError;
use crate::infra::event_bus::{EventBus, EventContext};
use crate::infra::events::{
    AgentEvent, AssistantMessageEvent, ContentBlock, ExtensionEvent, Message, ToolOutput,
};

use super::convert::classify_error;
use super::types::{
    unix_ts_ms, AgentLoop, AgentLoopConfig, AgentRunOutcome, AgentRunResult, LoopError,
    ToolCallAccumulator, ToolCallInfo,
};

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
        self.steering_queue
            .lock()
            .push(ChatMessage::steering(msg));
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
    fn emit_context_metrics(&mut self) {
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

    fn emit_event(&self, event: AgentEvent) {
        let payload = serde_json::to_value(&event).unwrap_or(serde_json::Value::Null);
        let event_name = payload
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let ctx = EventContext::new(event_name.clone(), payload);
        let _ = self.event_bus.emit_sync(&event_name, ctx);
    }

    fn emit_extension_event(&self, event: ExtensionEvent) {
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
                    let overflow_hit = is_context_overflow_error(&e);
                    let context_state_some = self.context_state.is_some();
                    let err_snippet: String = e.chars().take(200).collect();
                    info!(
                        target: "pi_wasm_chat_diag",
                        phase = "attempt_loop_retryable",
                        attempt,
                        overflow_hit,
                        context_state_some,
                        snippet = %err_snippet
                    );
                    if overflow_hit && context_state_some {
                        let ratio_before = self
                            .context_state
                            .as_ref()
                            .map(|cs| cs.usage_ratio())
                            .unwrap_or(0.0);
                        self.emit_event(AgentEvent::ContextOverflowTrimStart {
                            reason: "context_overflow".into(),
                            ratio: ratio_before,
                        });
                        let mut trim_tokens = 0usize;
                        let mut trim_turns = 0usize;
                        if let Some(ref mut ctx_state) = self.context_state {
                            let (turns_removed, chars_removed) =
                                force_drop_oldest_to_target(ctx_state);
                            trim_turns = turns_removed;
                            trim_tokens = estimated_tokens_from_chars(chars_removed);
                            ctx_state.session_obs.compaction_tokens_freed += trim_tokens;
                            ctx_state.session_obs.compaction_count =
                                ctx_state.session_obs.compaction_count.saturating_add(1);
                            let tail_start = self.context_tail_start.min(messages.len());
                            let tail: Vec<ChatMessage> = messages[tail_start..].to_vec();
                            let mut rebuilt: Vec<ChatMessage> = Vec::new();
                            if messages
                                .first()
                                .is_some_and(|m| m.role == ChatMessageRole::System)
                            {
                                rebuilt.push(messages[0].clone());
                            }
                            rebuilt.extend(
                                crate::core::session::manager::build_context_from_state(ctx_state),
                            );
                            let tail_start_in_rebuilt = rebuilt.len();
                            rebuilt.extend(tail);
                            *messages = rebuilt;
                            self.start_idx = tail_start_in_rebuilt;
                        }
                        let ratio_after = self
                            .context_state
                            .as_ref()
                            .map(|cs| cs.usage_ratio())
                            .unwrap_or(0.0);
                        self.emit_event(AgentEvent::ContextOverflowTrimEnd {
                            ratio_before,
                            ratio_after,
                            will_retry: true,
                            estimated_tokens_freed: trim_tokens,
                            turns_removed: trim_turns,
                        });
                        let compaction_count_after = self
                            .context_state
                            .as_ref()
                            .map(|cs| cs.session_obs.compaction_count)
                            .unwrap_or(0);
                        info!(
                            target: "pi_wasm_chat_diag",
                            phase = "l3_trim_done",
                            attempt,
                            turns_removed = trim_turns,
                            trim_tokens,
                            ratio_before,
                            ratio_after,
                            compaction_count_after
                        );
                    } else if overflow_hit && !context_state_some {
                        info!(
                            target: "pi_wasm_chat_diag",
                            phase = "l3_skipped_no_context_state",
                            attempt
                        );
                    } else if !overflow_hit {
                        info!(
                            target: "pi_wasm_chat_diag",
                            phase = "l3_skipped_not_overflow",
                            attempt
                        );
                    }
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
    fn make_aborted(
        &mut self,
        messages: &Vec<ChatMessage>,
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

            // ── LLM connect：chat_stream 的建连 await 也要可中断 ──
            let cancel = self.cancel_token.clone();
            let mut stream = {
                let connect = self.llm.chat_stream(req);
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        return Err(self.make_aborted(messages, final_text));
                    }
                    conn = connect => match conn {
                        Ok(s) => s,
                        Err(e) => {
                            let snippet: String = e.to_string().chars().take(200).collect();
                            info!(
                                target: "pi_wasm_chat_diag",
                                phase = "reasoning_chat_stream_connect_err",
                                snippet = %snippet
                            );
                            return Err(classify_error(&e));
                        }
                    }
                }
            };

            let mut content_buf = String::new();
            let mut tool_calls_buf: Vec<ToolCallAccumulator> = Vec::new();
            let mut aborted_during_stream = false;

            let msg_json = serde_json::json!({});
            self.emit_event(AgentEvent::MessageStart {
                message: Message(msg_json.clone()),
            });

            loop {
                let cancel = self.cancel_token.clone();
                let next = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        aborted_during_stream = true;
                        break;
                    }
                    item = stream.next() => item,
                };
                let Some(item) = next else {
                    break;
                };
                match item {
                    Ok(StreamEvent::ContentDelta { delta }) => {
                        content_buf.push_str(&delta);
                        self.emit_event(AgentEvent::MessageUpdate {
                            message: Message(serde_json::json!({})),
                            assistant_message_event: AssistantMessageEvent(
                                serde_json::json!({ "delta": delta }),
                            ),
                        });
                    }
                    Ok(StreamEvent::ToolCallDelta {
                        index,
                        id,
                        name,
                        arguments_delta,
                    }) => {
                        while tool_calls_buf.len() <= index as usize {
                            tool_calls_buf.push(ToolCallAccumulator::default());
                        }
                        let acc = &mut tool_calls_buf[index as usize];
                        if let Some(id_val) = id {
                            acc.id = id_val;
                        }
                        if let Some(name_val) = name {
                            acc.name = name_val;
                        }
                        if let Some(args) = arguments_delta {
                            acc.arguments.push_str(&args);
                        }
                    }
                    Ok(StreamEvent::FinishReason { .. }) => break,
                    Ok(StreamEvent::Usage {
                        prompt_tokens,
                        completion_tokens,
                        ..
                    }) => {
                        if let Some(ref mut ctx_state) = self.context_state {
                            ctx_state.update_api_usage(prompt_tokens, completion_tokens);
                        }
                    }
                    Err(e) => {
                        self.emit_event(AgentEvent::MessageEnd {
                            message: Message(serde_json::json!({})),
                        });
                        return Err(classify_error(&e));
                    }
                }
            }

            self.emit_event(AgentEvent::MessageEnd {
                message: Message(serde_json::json!({})),
            });

            // stream 被取消：把 partial content_buf 作为 partial assistant 落到 messages，
            // 让 ctx_state 也把它计入消息预算；再返回 Aborted 携带 partial。
            if aborted_during_stream {
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

            let tool_calls: Vec<ToolCallInfo> = tool_calls_buf
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

            if let Some(ref mut ctx_state) = self.context_state {
                let assistant_chars = content_buf.len()
                    + tool_calls
                        .iter()
                        .map(|tc| tc.name.len() + tc.arguments.len() + tc.id.len() + 40)
                        .sum::<usize>();
                ctx_state.on_message_appended(assistant_chars);
            }

            {
                let tc_json: Vec<serde_json::Value> = tool_calls
                    .iter()
                    .map(|tc| {
                        serde_json::json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": tc.arguments
                            }
                        })
                    })
                    .collect();
                messages.push(ChatMessage::assistant_with_tool_calls(
                    if content_buf.is_empty() {
                        None
                    } else {
                        Some(content_buf.as_str())
                    },
                    tc_json,
                ));
            }

            let mut tool_results = Vec::new();
            let mut steered = false;

            if self.block_tool_calls {
                for tc in &tool_calls {
                    let blocked_msg = format!(
                        "[Tool call blocked: context usage too high. Tool '{}' was not executed.]",
                        tc.name
                    );
                    if let Some(ref mut ctx_state) = self.context_state {
                        ctx_state.on_message_appended(blocked_msg.len());
                    }
                    messages.push(ChatMessage::tool(&tc.id, &blocked_msg));
                    tool_results.push(Message(serde_json::json!({ "content": blocked_msg })));
                }
                self.block_tool_calls = false;

                self.emit_event(AgentEvent::TurnEnd {
                    session_id: self.config.session_id.clone(),
                    turn_index,
                    message: Message(serde_json::json!({})),
                    tool_results,
                });
                continue;
            }

            for tc in &tool_calls {
                if self.cancel_token.is_cancelled() {
                    return Err(self.make_aborted(messages, final_text));
                }

                let args: serde_json::Value =
                    serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null);

                self.emit_event(AgentEvent::ToolExecutionStart {
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.name.clone(),
                    args: args.clone(),
                });

                self.emit_extension_event(ExtensionEvent::ToolCall {
                    tool_name: tc.name.clone(),
                    tool_call_id: tc.id.clone(),
                    input: args.clone(),
                });

                // 工具执行本身是 await 点，用 select! 包住；`kill_on_drop(true)` +
                // `reqwest` 连接被 drop 时自动关闭，保证子进程 / HTTP 连接被及时释放。
                let cancel = self.cancel_token.clone();
                let (result_content, is_error) = {
                    let exec = self.execute_tool(tc);
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            // 即便 cancel 先触发，也发布 ToolExecutionEnd 让 UI 完成配对
                            self.emit_event(AgentEvent::ToolExecutionEnd {
                                tool_call_id: tc.id.clone(),
                                tool_name: tc.name.clone(),
                                result: ToolOutput(serde_json::json!("[interrupted]")),
                                is_error: true,
                            });
                            return Err(self.make_aborted(messages, final_text));
                        }
                        out = exec => out,
                    }
                };

                self.emit_extension_event(ExtensionEvent::ToolResult {
                    tool_name: tc.name.clone(),
                    tool_call_id: tc.id.clone(),
                    input: args,
                    content: vec![ContentBlock(serde_json::json!({ "text": result_content }))],
                    details: None,
                    is_error,
                });

                self.emit_event(AgentEvent::ToolExecutionEnd {
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.name.clone(),
                    result: ToolOutput(serde_json::json!(result_content)),
                    is_error,
                });

                if let Some(ref mut ctx_state) = self.context_state {
                    ctx_state.on_message_appended(result_content.len());
                }

                messages.push(ChatMessage::tool(&tc.id, &result_content));
                tool_results.push(Message(serde_json::json!({ "content": result_content })));

                let mut q = self.steering_queue.lock();
                if !q.is_empty() {
                    messages.extend(q.drain(..));
                    steered = true;
                    break;
                }
            }

            // No synchronous cascade here; L0/L1/L2 handled at timing ⑤

            self.emit_event(AgentEvent::TurnEnd {
                session_id: self.config.session_id.clone(),
                turn_index,
                message: Message(serde_json::json!({})),
                tool_results,
            });

            if steered {
                continue;
            }

            if turn_index >= self.config.max_tool_rounds {
                self.emit_context_metrics();
                return Ok(final_text);
            }
        }
    }

    async fn execute_tool(&self, tc: &ToolCallInfo) -> (String, bool) {
        let args: serde_json::Value = match serde_json::from_str(&tc.arguments) {
            Ok(v) => v,
            Err(e) => return (format!("参数解析失败: {}", e), true),
        };

        let plugin_id = "__agent__";

        let out = match tc.name.as_str() {
            "read_file" => {
                let path = args["path"].as_str().unwrap_or("");
                self.primitive
                    .read_file(path, plugin_id)
                    .await
                    .map_err(|e| e.to_string())
            }
            "write_file" => {
                let path = args["path"].as_str().unwrap_or("");
                let content = args["content"].as_str().unwrap_or("");
                let overwrite = args["overwrite"].as_bool().unwrap_or(false);
                self.primitive
                    .write_file(path, content, overwrite, plugin_id)
                    .await
                    .map(|r| {
                        if r.written {
                            format!("已写入: {}", r.path)
                        } else {
                            format!("写入被拒绝: {}", r.path)
                        }
                    })
                    .map_err(|e| e.to_string())
            }
            "edit_file" => {
                let path = args["path"].as_str().unwrap_or("");
                let old_content = args["old_content"].as_str().unwrap_or("");
                let new_content = args["new_content"].as_str().unwrap_or("");
                let edits = vec![EditOperation {
                    operation_type: EditOperationType::Replace,
                    start_line: None,
                    end_line: None,
                    old_content: Some(old_content.to_string()),
                    new_content: new_content.to_string(),
                }];
                self.primitive
                    .edit_file(path, edits, plugin_id)
                    .await
                    .map(|r| {
                        if r.applied {
                            format!("已编辑: {}", r.path)
                        } else {
                            format!("编辑被拒绝: {}", r.path)
                        }
                    })
                    .map_err(|e| e.to_string())
            }
            "execute_bash" => {
                let command = args["command"].as_str().unwrap_or("");
                let cwd = args["cwd"].as_str();
                let argv_store: Option<Vec<String>> =
                    args.get("args").and_then(|v| v.as_array()).map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    });
                let argv_ref = argv_store.as_deref();
                self.primitive
                    .execute_bash(command, cwd, plugin_id, argv_ref)
                    .await
                    .map(|r| {
                        let mut out = String::new();
                        if !r.stdout.is_empty() {
                            out.push_str(&r.stdout);
                        }
                        if !r.stderr.is_empty() {
                            if !out.is_empty() {
                                out.push('\n');
                            }
                            out.push_str("STDERR: ");
                            out.push_str(&r.stderr);
                        }
                        out.push_str(&format!("\n(exit code: {})", r.exit_code));
                        out
                    })
                    .map_err(|e| e.to_string())
            }
            "list_dir" => {
                let path = args["path"].as_str().unwrap_or("");
                self.primitive
                    .list_dir(path, plugin_id)
                    .await
                    .map(|entries| {
                        let lines: Vec<String> = entries
                            .iter()
                            .map(|e| {
                                if e.is_dir {
                                    format!("  {}/ (dir)", e.name)
                                } else {
                                    format!("  {}", e.name)
                                }
                            })
                            .collect();
                        lines.join("\n")
                    })
                    .map_err(|e| e.to_string())
            }
            other => Err(format!("未知工具: {}", other)),
        };

        match out {
            Ok(s) => (s, false),
            Err(s) => (s, true),
        }
    }
}
