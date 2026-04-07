use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use tokio_stream::StreamExt;

use crate::core::compaction::{
    is_context_overflow_error, force_drop_oldest_to_target, run_layer0_cleanup,
};
use crate::core::context_metrics::ContextMetrics;
use crate::core::llm::{ChatRequest, LlmProvider, StreamEvent};
use crate::core::primitives::{EditOperation, EditOperationType, PrimitiveExecutor};
use crate::core::session::manager::ContextState;
use crate::infra::error::AppError;
use crate::infra::event_bus::{EventBus, EventContext};
use crate::infra::events::{
    AgentEvent, AssistantMessageEvent, ContentBlock, ExtensionEvent, Message, ToolOutput,
};

use super::convert::{classify_error, convert_to_llm_format};
use super::types::{
    unix_ts_ms, AgentLoop, AgentLoopConfig, AgentMessage, AgentRunResult, LoopError,
    ToolCallAccumulator, ToolCallInfo,
};

impl AgentLoop {
    pub fn new(
        llm: Arc<dyn LlmProvider>,
        primitive: Arc<dyn PrimitiveExecutor>,
        event_bus: Arc<dyn EventBus>,
        config: AgentLoopConfig,
        abort_signal: Arc<AtomicBool>,
    ) -> Self {
        Self {
            llm,
            primitive,
            event_bus,
            config,
            steering_queue: Arc::new(Mutex::new(Vec::new())),
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
            abort_signal,
            context_state: None,
            block_tool_calls: false,
            metrics: ContextMetrics::default(),
            start_idx: 0,
        }
    }

    /// 测试用：注入 steering_queue，便于 mock 在工具执行中推入 steering 消息。
    #[cfg(test)]
    pub fn new_with_steering_queue(
        llm: Arc<dyn LlmProvider>,
        primitive: Arc<dyn PrimitiveExecutor>,
        event_bus: Arc<dyn EventBus>,
        config: AgentLoopConfig,
        abort_signal: Arc<AtomicBool>,
        steering_queue: Arc<Mutex<Vec<AgentMessage>>>,
    ) -> Self {
        Self {
            llm,
            primitive,
            event_bus,
            config,
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
            steering_queue,
            abort_signal,
            context_state: None,
            block_tool_calls: false,
            metrics: ContextMetrics::default(),
            start_idx: 0,
        }
    }

    pub fn steer(&self, msg: String) {
        self.steering_queue.lock().push(AgentMessage::Steering {
            text: msg,
            timestamp: unix_ts_ms(),
        });
    }

    pub fn follow_up(&self, msg: String) {
        self.follow_up_queue
            .lock()
            .push(AgentMessage::User { text: msg });
    }

    pub fn abort(&self) {
        self.abort_signal.store(true, Ordering::SeqCst);
    }

    pub fn abort_signal(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.abort_signal)
    }

    pub fn set_context_state(&mut self, state: Option<ContextState>) {
        self.context_state = state;
    }

    pub fn take_context_state(&mut self) -> Option<ContextState> {
        self.context_state.take()
    }

    /// 刷新实时 token 指标并发射 ContextMetricsUpdate 事件（仅当 context_state 存在时）。
    fn emit_context_metrics(&mut self) {
        if let Some(ref ctx_state) = self.context_state {
            self.metrics.input_tokens_used = ctx_state.estimated_token_count();
            self.metrics.context_utilization_ratio = ctx_state.usage_ratio();
            self.metrics.preheat_in_progress = ctx_state.preheat.is_running();
        }
        if self.context_state.is_some() {
            self.emit_event(AgentEvent::ContextMetricsUpdate {
                input_tokens_used: self.metrics.input_tokens_used,
                context_utilization_ratio: self.metrics.context_utilization_ratio,
                compaction_count: self.metrics.compaction_count,
                compaction_tokens_freed: self.metrics.compaction_tokens_freed,
                total_tool_result_bytes_persisted: self.metrics.total_tool_result_bytes_persisted,
                preheat_in_progress: self.metrics.preheat_in_progress,
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
    pub async fn run(
        &mut self,
        initial_messages: Vec<AgentMessage>,
    ) -> Result<AgentRunResult, AppError> {
        self.abort_signal.store(false, Ordering::SeqCst);

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

        self.start_idx = messages.len();

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
                        return Ok(result);
                    }
                    messages.extend(q.drain(..));
                    continue;
                }
                Err(LoopError::Aborted) => {
                    self.emit_event(AgentEvent::AgentEnd {
                        session_id: self.config.session_id.clone(),
                        messages: vec![],
                        error: Some("interrupted".to_string()),
                    });
                    return Err(AppError::Config("用户中断".to_string()));
                }
                Err(LoopError::Fatal(e)) => {
                    self.emit_event(AgentEvent::AgentEnd {
                        session_id: self.config.session_id.clone(),
                        messages: vec![],
                        error: Some(e.clone()),
                    });
                    return Err(AppError::Llm(e));
                }
                Err(LoopError::Retryable(_)) => {
                    unreachable!()
                }
            }
        }
    }

    /// 第二层：Attempt loop，错误分类与指数退避重试。
    async fn run_attempt_loop(
        &mut self,
        messages: &mut Vec<AgentMessage>,
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
                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
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
                Err(LoopError::Aborted) => return Err(LoopError::Aborted),
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
                    if is_context_overflow_error(&e) && self.context_state.is_some() {
                        let ratio_before = self
                            .context_state
                            .as_ref()
                            .map(|cs| cs.usage_ratio())
                            .unwrap_or(0.0);
                        self.emit_event(AgentEvent::ContextOverflowTrimStart {
                            reason: "context_overflow".into(),
                            ratio: ratio_before,
                        });
                        if let Some(ref mut ctx_state) = self.context_state {
                            force_drop_oldest_to_target(ctx_state);
                            *messages =
                                crate::core::session::manager::build_context_from_state(ctx_state);
                            self.start_idx = messages.len();
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
                        });
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

    /// 第三层：Reasoning loop，LLM 流式 + 工具执行 + Steering/Abort 检查。
    async fn run_reasoning_loop(
        &mut self,
        messages: &mut Vec<AgentMessage>,
    ) -> Result<String, LoopError> {
        let mut final_text = String::new();
        let mut turn_index: usize = 0;

        loop {
            if self.abort_signal.load(Ordering::SeqCst) {
                return Err(LoopError::Aborted);
            }

            turn_index += 1;
            self.emit_event(AgentEvent::TurnStart {
                session_id: self.config.session_id.clone(),
                turn_index,
                timestamp: unix_ts_ms(),
            });

            let llm_messages = convert_to_llm_format(messages);
            let req = ChatRequest {
                messages: llm_messages,
                model: self.config.model.clone(),
                temperature: None,
                max_tokens: None,
                stream: Some(true),
                model_override: None,
                tools: Some(self.config.tool_definitions.clone()),
            };

            let mut stream = match self.llm.chat_stream(req).await {
                Ok(s) => s,
                Err(e) => {
                    return Err(classify_error(&e));
                }
            };

            let mut content_buf = String::new();
            let mut tool_calls_buf: Vec<ToolCallAccumulator> = Vec::new();

            let msg_json = serde_json::json!({});
            self.emit_event(AgentEvent::MessageStart {
                message: Message(msg_json.clone()),
            });

            while let Some(item) = stream.next().await {
                if self.abort_signal.load(Ordering::SeqCst) {
                    break;
                }
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
                messages.push(AgentMessage::Assistant {
                    text: content_buf,
                    tool_calls: vec![],
                });

                // Timing ⑤: L0 → try_restart → check_after_reply → try_start → metrics
                let mut preheat_started: Option<(usize, f64)> = None;
                if let Some(ref mut ctx_state) = self.context_state {
                    // Step 1: L0 cleanup
                    let persisted = run_layer0_cleanup(
                        ctx_state,
                        &self.config.context_config,
                        std::path::Path::new(&self.config.work_dir),
                        &self.config.session_id,
                    );
                    for pr in &persisted {
                        self.metrics.total_tool_result_bytes_persisted += pr.original_chars;
                    }

                    // Step 2: restore ExhaustedPending → Running
                    ctx_state.preheat.try_restart_if_pending(
                        ctx_state.usage_ratio(),
                        &ctx_state.user_turns_list,
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
                    let turn_count = ctx_state.user_turns_list.len();
                    if ctx_state.preheat.try_start(
                        ratio,
                        &ctx_state.user_turns_list,
                        &ctx_state.transcript_path,
                        Arc::clone(&self.llm),
                        &self.config.context_config,
                        Arc::clone(&self.event_bus),
                    ) {
                        preheat_started = Some((turn_count, ratio));
                    }
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

            messages.push(AgentMessage::Assistant {
                text: content_buf.clone(),
                tool_calls: tool_calls.clone(),
            });

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
                    messages.push(AgentMessage::ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: blocked_msg.clone(),
                        is_error: true,
                    });
                    tool_results.push(Message(serde_json::json!({ "content": blocked_msg })));
                }
                self.block_tool_calls = false;

                self.emit_context_metrics();
                self.emit_event(AgentEvent::TurnEnd {
                    session_id: self.config.session_id.clone(),
                    turn_index,
                    message: Message(serde_json::json!({})),
                    tool_results,
                });
                continue;
            }

            for tc in &tool_calls {
                if self.abort_signal.load(Ordering::SeqCst) {
                    return Err(LoopError::Aborted);
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

                let (result_content, is_error) = self.execute_tool(tc).await;

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

                messages.push(AgentMessage::ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: result_content.clone(),
                    is_error,
                });
                tool_results.push(Message(serde_json::json!({ "content": result_content })));

                let mut q = self.steering_queue.lock();
                if !q.is_empty() {
                    messages.extend(q.drain(..));
                    steered = true;
                    break;
                }
            }

            // No synchronous cascade here; L0/L1/L2 handled at timing ⑤

            self.emit_context_metrics();
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
