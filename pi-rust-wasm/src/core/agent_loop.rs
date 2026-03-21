//! # Agent Loop 核心结构化实现
//!
//! 三层循环（Conversation / Attempt / Reasoning）、Steering、FollowUp、Abort、
//! 事件发布与错误分类重试，与 agent-loop.md 设计对齐。
//!
//! ## 结构示意
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────────────────┐
//! │                              AgentLoop                                        │
//! ├──────────────────────────────────────────────────────────────────────────────┤
//! │ 注入依赖                                                                       │
//! │   llm ──────────────► LlmProvider::chat_stream（流式 LLM 调用）               │
//! │   primitive ─────────► PrimitiveExecutor（read/write/edit/bash）              │
//! │   event_bus ─────────► EventBus::emit_sync（AgentEvent 生命周期发布）          │
//! ├──────────────────────────────────────────────────────────────────────────────┤
//! │ 配置 (AgentLoopConfig)                                                        │
//! │   model            ► LLM 模型名                                               │
//! │   session_id       ► 会话 ID（随事件一起发布）                                  │
//! │   max_attempts     ► Retryable 最大重试次数（默认 3）                           │
//! │   max_tool_rounds  ► 单次 Attempt 最大工具轮次（默认 10）                       │
//! │   retry_base_delay ► 指数退避基准延迟 ms（默认 300）                            │
//! │   tool_definitions ► 传入 LLM 的工具 JSON Schema 列表                         │
//! ├──────────────────────────────────────────────────────────────────────────────┤
//! │ 运行时状态                                                                     │
//! │   steering_queue  ─► Mutex<Vec<AgentMessage::Steering>>（跨线程注入）          │
//! │   follow_up_queue ─► Mutex<Vec<AgentMessage::User>>（同上下文追问）            │
//! │   abort_signal    ─► AtomicBool（Ctrl+C 中断）                                │
//! │   on_stream_delta ─► Option<FnMut(&str)>（流式文字推送到渲染层）               │
//! └──────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## 三层循环调用流
//!
//! ```text
//!   调用方（chat.rs）
//!     │  run(initial_messages)
//!     ▼
//! ┌──────────────────────────────────────────────────────────────────────────────┐
//! │  【第一层】Conversation Loop                  emit: agent_start              │
//! │                                                                               │
//! │   ┌─ 开始时注入 steering_queue 中已有消息                                     │
//! │   │                                                                           │
//! │   │  run_attempt_loop(messages)                                               │
//! │   │   ▼                                                                       │
//! │  【第二层】Attempt Loop（重试）                                                │
//! │   │                                                                           │
//! │   │   for attempt in 1..=max_attempts:                                        │
//! │   │     ┌── Retryable 错误 ──► delay=base×2^(attempt-1) ──► emit:auto_retry │
//! │   │     │                                                                     │
//! │   │     │  run_reasoning_loop(messages)                                       │
//! │   │     │   ▼                                                                 │
//! │   │    【第三层】Reasoning Loop（LLM ↔ 工具）                                  │
//! │   │     │                                                                     │
//! │   │     │   loop:                                                             │
//! │   │     │     abort? ──是──► Err(Aborted)                                    │
//! │   │     │     emit: turn_start                                                │
//! │   │     │     llm.chat_stream(messages)                                       │
//! │   │     │       ├── ContentDelta ──► content_buf / on_stream_delta           │
//! │   │     │       │                    emit: message_update                    │
//! │   │     │       ├── ToolCallDelta ──► tool_calls_buf 累积                    │
//! │   │     │       └── Err(e) ──► classify_error → Retryable / Fatal            │
//! │   │     │     emit: message_end                                               │
//! │   │     │                                                                     │
//! │   │     │     tool_calls 为空? ──是──► emit: turn_end ──► return Ok(text)   │
//! │   │     │                                                                     │
//! │   │     │     for tc in tool_calls:                                           │
//! │   │     │       abort? ──是──► Err(Aborted)                                  │
//! │   │     │       emit: ToolExecutionStart → tool_execution_start              │
//! │   │     │       emit: ExtensionEvent ToolCall → tool_call                    │
//! │   │     │       execute_tool(tc) → (content, is_error)                       │
//! │   │     │       emit: ExtensionEvent ToolResult → tool_result                │
//! │   │     │         ├── read_file / list_dir / write_file                       │
//! │   │     │         ├── edit_file / execute_bash                                │
//! │   │     │         └── unknown ──► is_error=true                              │
//! │   │     │       emit: ToolExecutionEnd → tool_execution_end                 │
//! │   │     │       messages.push(ToolResult)                                     │
//! │   │     │       steering_queue 非空? ──是──► 注入 + break（跳过剩余工具）      │
//! │   │     │     emit: turn_end                                                  │
//! │   │     │     steered? ──是──► continue（下一轮 LLM）                         │
//! │   │     │     turn_index >= max_tool_rounds? ──是──► return Ok(text)         │
//! │   │     └──────────────────────────────────────────────────────────────────  │
//! │   │                                                                           │
//! │   │   Ok(text) ──► emit: agent_end(ok)                                       │
//! │   │   follow_up_queue 非空? ──是──► drain 追加消息，continue 第一层           │
//! │   │              ──否──► return Ok(AgentRunResult)                           │
//! │   │                                                                           │
//! │   │   Err(Aborted) ──► emit: agent_end(interrupted) ──► Err                 │
//! │   └── Err(Fatal)   ──► emit: agent_end(error)       ──► Err                 │
//! └──────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## 错误分类
//!
//! ```text
//!   AppError::Llm(msg)
//!     │
//!     ├── 含 "429" / "500" / "502" / "503" / "超时" / "请求失败"
//!     │       └──► LoopError::Retryable  →  第二层指数退避重试
//!     │
//!     ├── 含 "401" / "400"
//!     │       └──► LoopError::Fatal      →  立即终止，agent_end(error)
//!     │
//!     └── 其他
//!             └──► LoopError::Fatal
//! ```
//!
//! ## AgentMessage 消息类型与 LLM 格式转换
//!
//! ```text
//!   AgentMessage                           ChatMessage (OpenAI)
//!   ─────────────────────────────────────────────────────────────
//!   User     { text }               ──►  role=user,      content=text
//!   Steering { text, timestamp }    ──►  role=user,      content=text
//!   CompactionSummary { summary }   ──►  role=user,      content=summary
//!   System   { text }               ──►  role=system,    content=text
//!   Assistant{ text, tool_calls=[] }──►  role=assistant, content=text
//!   Assistant{ text, tool_calls=[…]}──►  role=assistant, tool_calls=[…]
//!   ToolResult{ id, content, .. }   ──►  role=tool,      tool_call_id=id
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use tokio_stream::StreamExt;

use super::llm::{ChatMessage, ChatMessageRole, ChatRequest, LlmProvider, StreamEvent};
use crate::core::primitives::{EditOperation, EditOperationType, PrimitiveExecutor};
use crate::infra::error::AppError;
use crate::infra::event_bus::{EventBus, EventContext};
use crate::infra::events::{
    AgentEvent, AssistantMessageEvent, ContentBlock, ExtensionEvent, Message, ToolOutput,
};
use tracing::debug;

/// 流式 delta 回调类型，供调用方渲染等。
pub type OnStreamDelta = Box<dyn FnMut(&str) + Send>;

// ─── 5.1 AgentMessage 与转换 ───────────────────────────────────────────────

/// 单次工具调用信息（与 OpenAI 流式 tool_calls 对应）。
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Agent 内部富类型消息；仅在调 LLM 边界转为 ChatMessage。
#[derive(Debug, Clone)]
pub enum AgentMessage {
    User {
        text: String,
    },
    Assistant {
        text: String,
        tool_calls: Vec<ToolCallInfo>,
    },
    ToolResult {
        tool_call_id: String,
        content: String,
        is_error: bool,
    },
    System {
        text: String,
    },
    Steering {
        text: String,
        timestamp: i64,
    },
    CompactionSummary {
        summary: String,
    },
}

/// 将 Agent 消息列表转为 LLM 使用的 ChatMessage 序列。
pub fn convert_to_llm_format(messages: &[AgentMessage]) -> Vec<ChatMessage> {
    messages
        .iter()
        .map(|m| match m {
            AgentMessage::User { text } => ChatMessage::user(text.as_str()),
            AgentMessage::Steering { text, .. } => ChatMessage::user(text.as_str()),
            AgentMessage::CompactionSummary { summary } => ChatMessage::user(summary.as_str()),
            AgentMessage::System { text } => ChatMessage::system(text.as_str()),
            AgentMessage::Assistant { text, tool_calls } => {
                if tool_calls.is_empty() {
                    ChatMessage::assistant(text.as_str())
                } else {
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
                    ChatMessage::assistant_with_tool_calls(
                        if text.is_empty() {
                            None
                        } else {
                            Some(text.as_str())
                        },
                        tc_json,
                    )
                }
            }
            AgentMessage::ToolResult {
                tool_call_id,
                content,
                ..
            } => ChatMessage::tool(tool_call_id, content),
        })
        .collect()
}

/// 从 Session 加载的 ChatMessage 转为 AgentMessage（用于 chat 拼装 initial_messages）。
pub fn agent_messages_from_chat(messages: &[ChatMessage]) -> Vec<AgentMessage> {
    messages
        .iter()
        .map(|m| match &m.role {
            ChatMessageRole::User => AgentMessage::User {
                text: m.text_content().unwrap_or("").to_string(),
            },
            ChatMessageRole::System => AgentMessage::System {
                text: m.text_content().unwrap_or("").to_string(),
            },
            ChatMessageRole::Assistant => {
                let text = m.text_content().unwrap_or("").to_string();
                let tool_calls = m
                    .tool_calls
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(|v| {
                        let obj = v.as_object()?;
                        let id = obj.get("id")?.as_str()?.to_string();
                        let func = obj.get("function")?.as_object()?;
                        let name = func.get("name")?.as_str()?.to_string();
                        let arguments = func
                            .get("arguments")
                            .and_then(|a| a.as_str())
                            .unwrap_or("")
                            .to_string();
                        Some(ToolCallInfo {
                            id,
                            name,
                            arguments,
                        })
                    })
                    .collect();
                AgentMessage::Assistant { text, tool_calls }
            }
            ChatMessageRole::Tool => AgentMessage::ToolResult {
                tool_call_id: m.tool_call_id.as_deref().unwrap_or("").to_string(),
                content: m.text_content().unwrap_or("").to_string(),
                is_error: false,
            },
        })
        .collect()
}

// ─── 5.7 错误分类与重试 ─────────────────────────────────────────────────────

#[derive(Debug)]
pub enum LoopError {
    Retryable(String),
    Fatal(String),
    Aborted,
}

fn classify_error(err: &AppError) -> LoopError {
    let s = err.to_string();
    if s.contains("401") || s.contains("400") {
        return LoopError::Fatal(s);
    }
    if s.contains("429")
        || s.contains("500")
        || s.contains("502")
        || s.contains("503")
        || s.contains("请求失败")
        || s.contains("超时")
        || (s.contains("context") && (s.contains("length") || s.contains("token")))
    {
        return LoopError::Retryable(s);
    }
    LoopError::Fatal(s)
}

/// MVP：保留首条 System（若有）+ 最近 keep_recent 条。
pub fn compact_messages(messages: &mut Vec<AgentMessage>, keep_recent: usize) {
    if messages.len() <= keep_recent + 1 {
        return;
    }
    let system_take = matches!(messages.first(), Some(AgentMessage::System { .. })) as usize;
    let rest = messages.len() - system_take;
    if rest <= keep_recent {
        return;
    }
    let drop = rest - keep_recent;
    let start = system_take + drop;
    messages.drain(system_take..start);
}

// ─── 配置与结果 ─────────────────────────────────────────────────────────────

pub struct AgentLoopConfig {
    pub max_attempts: u32,
    pub max_tool_rounds: usize,
    pub retry_base_delay_ms: u64,
    pub model: String,
    pub session_id: String,
    pub tool_definitions: Vec<serde_json::Value>,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            max_tool_rounds: 10,
            retry_base_delay_ms: 300,
            model: String::new(),
            session_id: String::new(),
            tool_definitions: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct AgentRunResult {
    pub final_text: String,
}

// ─── AgentLoop 结构体 ───────────────────────────────────────────────────────

pub struct AgentLoop {
    llm: Arc<dyn LlmProvider>,
    primitive: Arc<dyn PrimitiveExecutor>,
    event_bus: Arc<dyn EventBus>,
    config: AgentLoopConfig,
    steering_queue: Arc<Mutex<Vec<AgentMessage>>>,
    follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
    abort_signal: Arc<AtomicBool>,
    on_stream_delta: Option<OnStreamDelta>,
}

fn unix_ts_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

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
            on_stream_delta: None,
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
        steering_queue: Arc<parking_lot::Mutex<Vec<AgentMessage>>>,
    ) -> Self {
        Self {
            llm,
            primitive,
            event_bus,
            config,
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
            steering_queue,
            abort_signal,
            on_stream_delta: None,
        }
    }

    pub fn set_on_stream_delta(&mut self, f: OnStreamDelta) {
        self.on_stream_delta = Some(f);
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

        // 第一层开始时可注入已有 steering
        {
            let mut q = self.steering_queue.lock();
            if !q.is_empty() {
                messages.extend(q.drain(..));
            }
        }

        loop {
            match self.run_attempt_loop(&mut messages).await {
                Ok(final_text) => {
                    let result = AgentRunResult {
                        final_text: final_text.clone(),
                    };
                    self.emit_event(AgentEvent::AgentEnd {
                        session_id: self.config.session_id.clone(),
                        messages: vec![],
                        error: None,
                    });

                    // FollowUp：同一上下文继续
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
                    // 在 run_attempt_loop 内已重试至耗尽，最后会返回 Fatal
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
                        if let Some(cb) = self.on_stream_delta.as_mut() {
                            cb(&delta);
                        }
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
                    Ok(StreamEvent::Usage { .. }) => {}
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
                debug!("[tool_debug] 本轮回复无 tool_calls");
                messages.push(AgentMessage::Assistant {
                    text: content_buf,
                    tool_calls: vec![],
                });
                self.emit_event(AgentEvent::TurnEnd {
                    session_id: self.config.session_id.clone(),
                    turn_index,
                    message: Message(serde_json::json!({})),
                    tool_results: vec![],
                });
                return Ok(final_text);
            }
            let tool_names: Vec<&str> = tool_calls.iter().map(|tc| tc.name.as_str()).collect();
            debug!(
                "[tool_debug] 本轮回复 tool_calls: {} 个 names={:?}",
                tool_calls.len(),
                tool_names
            );

            messages.push(AgentMessage::Assistant {
                text: content_buf.clone(),
                tool_calls: tool_calls.clone(),
            });

            let mut tool_results = Vec::new();
            let mut steered = false;

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
                self.primitive
                    .execute_bash(command, cwd, plugin_id)
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

#[derive(Default)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::llm::{ChatRequest, ChatResponse, LlmProvider, StreamEvent};
    use crate::infra::wire;
    use crate::infra::{DefaultEventBus, EventContext};
    use std::sync::Mutex;

    struct MockLlmProvider {
        /// 每次 chat_stream 调用取出一组事件（或错误）返回。
        streams: Mutex<Vec<Vec<Result<StreamEvent, AppError>>>>,
    }

    impl MockLlmProvider {
        fn new(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> Self {
            Self {
                streams: Mutex::new(streams),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for MockLlmProvider {
        fn provider_name(&self) -> &str {
            "mock"
        }
        async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
            Err(AppError::Llm("mock chat not used".to_string()))
        }
        async fn chat_stream(
            &self,
            _req: ChatRequest,
        ) -> Result<
            Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
            AppError,
        > {
            let mut guard = self.streams.lock().unwrap();
            let events = guard.remove(0);
            drop(guard);
            let stream = tokio_stream::iter(events);
            Ok(Box::new(stream))
        }
        fn count_tokens(
            &self,
            _messages: &[crate::core::llm::ChatMessage],
        ) -> Result<u32, AppError> {
            Ok(0)
        }
    }

    /// Mock LLM 的 chat_stream 直接返回 Err（用于 Fatal 401 等测试）。
    struct MockLlmProviderFatal {
        err: String,
    }

    #[async_trait::async_trait]
    impl LlmProvider for MockLlmProviderFatal {
        fn provider_name(&self) -> &str {
            "mock_fatal"
        }
        async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
            Err(AppError::Llm("mock chat not used".to_string()))
        }
        async fn chat_stream(
            &self,
            _req: ChatRequest,
        ) -> Result<
            Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
            AppError,
        > {
            Err(AppError::Llm(self.err.clone()))
        }
        fn count_tokens(
            &self,
            _messages: &[crate::core::llm::ChatMessage],
        ) -> Result<u32, AppError> {
            Ok(0)
        }
    }

    struct MockPrimitiveExecutor;

    /// 工具执行时 sleep，便于在测试中在另一任务里设置 abort。
    struct SleepyMockPrimitive;

    #[async_trait::async_trait]
    impl PrimitiveExecutor for SleepyMockPrimitive {
        async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            Ok(format!("content:{}", path))
        }
        async fn list_dir(
            &self,
            _path: &str,
            _plugin_id: &str,
        ) -> Result<Vec<crate::core::primitives::DirEntry>, AppError> {
            Ok(vec![])
        }
        async fn write_file(
            &self,
            path: &str,
            content: &str,
            overwrite: bool,
            _plugin_id: &str,
        ) -> Result<crate::core::primitives::WriteFileResult, AppError> {
            Ok(crate::core::primitives::WriteFileResult {
                path: path.to_string(),
                written: overwrite || content.is_empty(),
            })
        }
        async fn edit_file(
            &self,
            path: &str,
            _edits: Vec<crate::core::primitives::EditOperation>,
            _plugin_id: &str,
        ) -> Result<crate::core::primitives::EditFileResult, AppError> {
            Ok(crate::core::primitives::EditFileResult {
                path: path.to_string(),
                applied: true,
            })
        }
        async fn execute_bash(
            &self,
            command: &str,
            _cwd: Option<&str>,
            _plugin_id: &str,
        ) -> Result<crate::core::primitives::BashResult, AppError> {
            Ok(crate::core::primitives::BashResult {
                stdout: format!("out:{}", command),
                stderr: String::new(),
                exit_code: 0,
            })
        }
        async fn require_user_confirmation(
            &self,
            _operation: crate::core::primitives::PrimitiveOperation,
            _preview: &str,
            _plugin_id: &str,
        ) -> Result<bool, AppError> {
            Ok(true)
        }
    }

    /// 第一次 read_file 时向 steering_queue 推入 Steering，用于测试“跳过剩余工具”。
    struct SteerableMockPrimitive {
        steering_queue: Arc<parking_lot::Mutex<Vec<AgentMessage>>>,
        read_count: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl PrimitiveExecutor for SteerableMockPrimitive {
        async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
            let n = self
                .read_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                self.steering_queue.lock().push(AgentMessage::Steering {
                    text: "stop after first tool".to_string(),
                    timestamp: 0,
                });
            }
            Ok(format!("content:{}", path))
        }
        async fn list_dir(
            &self,
            _path: &str,
            _plugin_id: &str,
        ) -> Result<Vec<crate::core::primitives::DirEntry>, AppError> {
            Ok(vec![])
        }
        async fn write_file(
            &self,
            path: &str,
            content: &str,
            overwrite: bool,
            _plugin_id: &str,
        ) -> Result<crate::core::primitives::WriteFileResult, AppError> {
            Ok(crate::core::primitives::WriteFileResult {
                path: path.to_string(),
                written: overwrite || content.is_empty(),
            })
        }
        async fn edit_file(
            &self,
            path: &str,
            _edits: Vec<crate::core::primitives::EditOperation>,
            _plugin_id: &str,
        ) -> Result<crate::core::primitives::EditFileResult, AppError> {
            Ok(crate::core::primitives::EditFileResult {
                path: path.to_string(),
                applied: true,
            })
        }
        async fn execute_bash(
            &self,
            command: &str,
            _cwd: Option<&str>,
            _plugin_id: &str,
        ) -> Result<crate::core::primitives::BashResult, AppError> {
            Ok(crate::core::primitives::BashResult {
                stdout: format!("out:{}", command),
                stderr: String::new(),
                exit_code: 0,
            })
        }
        async fn require_user_confirmation(
            &self,
            _operation: crate::core::primitives::PrimitiveOperation,
            _preview: &str,
            _plugin_id: &str,
        ) -> Result<bool, AppError> {
            Ok(true)
        }
    }

    #[async_trait::async_trait]
    impl PrimitiveExecutor for MockPrimitiveExecutor {
        async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
            Ok(format!("content:{}", path))
        }
        async fn list_dir(
            &self,
            _path: &str,
            _plugin_id: &str,
        ) -> Result<Vec<crate::core::primitives::DirEntry>, AppError> {
            Ok(vec![])
        }
        async fn write_file(
            &self,
            path: &str,
            content: &str,
            overwrite: bool,
            _plugin_id: &str,
        ) -> Result<crate::core::primitives::WriteFileResult, AppError> {
            Ok(crate::core::primitives::WriteFileResult {
                path: path.to_string(),
                written: overwrite || content.is_empty(),
            })
        }
        async fn edit_file(
            &self,
            path: &str,
            _edits: Vec<crate::core::primitives::EditOperation>,
            _plugin_id: &str,
        ) -> Result<crate::core::primitives::EditFileResult, AppError> {
            Ok(crate::core::primitives::EditFileResult {
                path: path.to_string(),
                applied: true,
            })
        }
        async fn execute_bash(
            &self,
            command: &str,
            _cwd: Option<&str>,
            _plugin_id: &str,
        ) -> Result<crate::core::primitives::BashResult, AppError> {
            Ok(crate::core::primitives::BashResult {
                stdout: format!("out:{}", command),
                stderr: String::new(),
                exit_code: 0,
            })
        }
        async fn require_user_confirmation(
            &self,
            _operation: crate::core::primitives::PrimitiveOperation,
            _preview: &str,
            _plugin_id: &str,
        ) -> Result<bool, AppError> {
            Ok(true)
        }
    }

    #[tokio::test]
    async fn run_returns_text_when_llm_returns_text_only() {
        let stream1: Vec<Result<StreamEvent, AppError>> = vec![
            Ok(StreamEvent::ContentDelta {
                delta: "Hello".to_string(),
            }),
            Ok(StreamEvent::ContentDelta {
                delta: " world".to_string(),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "stop".to_string(),
            }),
        ];
        let llm = Arc::new(MockLlmProvider::new(vec![stream1]));
        let primitive = Arc::new(MockPrimitiveExecutor);
        let event_bus = Arc::new(DefaultEventBus::new());
        let config = AgentLoopConfig {
            model: "gpt-4".to_string(),
            session_id: "s1".to_string(),
            ..Default::default()
        };
        let abort = Arc::new(AtomicBool::new(false));
        let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
        let messages = vec![AgentMessage::User {
            text: "hi".to_string(),
        }];
        let result = loop_.run(messages).await.unwrap();
        assert_eq!(result.final_text, "Hello world");
    }

    #[tokio::test]
    async fn convert_to_llm_format_steering_as_user() {
        let messages = vec![AgentMessage::Steering {
            text: "stop".to_string(),
            timestamp: 0,
        }];
        let out = convert_to_llm_format(&messages);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0].role,
            crate::core::llm::ChatMessageRole::User
        ));
        assert_eq!(out[0].text_content(), Some("stop"));
    }

    #[tokio::test]
    async fn convert_to_llm_format_all_variants() {
        let messages = vec![
            AgentMessage::User {
                text: "u".to_string(),
            },
            AgentMessage::System {
                text: "s".to_string(),
            },
            AgentMessage::Assistant {
                text: "a".to_string(),
                tool_calls: vec![],
            },
            AgentMessage::ToolResult {
                tool_call_id: "id1".to_string(),
                content: "c".to_string(),
                is_error: false,
            },
        ];
        let out = convert_to_llm_format(&messages);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].text_content(), Some("u"));
        assert_eq!(out[1].text_content(), Some("s"));
        assert_eq!(out[2].text_content(), Some("a"));
        assert_eq!(out[3].text_content(), Some("c"));
    }

    #[test]
    fn compact_messages_keeps_system_and_recent() {
        let mut messages: Vec<AgentMessage> = (0..25)
            .map(|i| AgentMessage::User {
                text: format!("msg{}", i),
            })
            .collect();
        messages.insert(
            0,
            AgentMessage::System {
                text: "sys".to_string(),
            },
        );
        compact_messages(&mut messages, 5);
        assert!(messages.len() <= 6 + 5);
        let first = match &messages[0] {
            AgentMessage::System { text } => text.as_str(),
            _ => "",
        };
        assert_eq!(first, "sys");
    }

    #[test]
    fn classify_error_retryable_429() {
        let e = AppError::Llm("API 错误 429: rate limit".to_string());
        let r = classify_error(&e);
        assert!(matches!(r, LoopError::Retryable(_)));
    }

    #[test]
    fn classify_error_fatal_401() {
        let e = AppError::Llm("API 错误 401: unauthorized".to_string());
        let r = classify_error(&e);
        assert!(matches!(r, LoopError::Fatal(_)));
    }

    /// 重试：Mock LLM 先返回 429 再返回成功 -> 自动重试后得到文本。
    #[tokio::test]
    async fn run_retries_on_429_then_succeeds() {
        let stream_err = vec![Err(AppError::Llm("API 错误 429: rate limit".to_string()))];
        let stream_ok: Vec<Result<StreamEvent, AppError>> = vec![
            Ok(StreamEvent::ContentDelta {
                delta: "OK".to_string(),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "stop".to_string(),
            }),
        ];
        let llm = Arc::new(MockLlmProvider::new(vec![stream_err, stream_ok]));
        let primitive = Arc::new(MockPrimitiveExecutor);
        let event_bus = Arc::new(DefaultEventBus::new());
        let config = AgentLoopConfig {
            max_attempts: 3,
            model: "gpt-4".to_string(),
            session_id: "s1".to_string(),
            ..Default::default()
        };
        let abort = Arc::new(AtomicBool::new(false));
        let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
        let messages = vec![AgentMessage::User {
            text: "hi".to_string(),
        }];
        let result = loop_.run(messages).await.unwrap();
        assert_eq!(result.final_text, "OK");
    }

    /// 工具循环：第 1 次 LLM 返回 read_file tool call，第 2 次返回纯文本；断言 final_text 含第 2 次文本。
    #[tokio::test]
    async fn run_tool_loop_calls_tool_then_returns_text() {
        let stream_tool: Vec<Result<StreamEvent, AppError>> = vec![
            Ok(StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("call_1".to_string()),
                name: Some("read_file".to_string()),
                arguments_delta: Some(r#"{"path":"/tmp/x"}"#.to_string()),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "tool_calls".to_string(),
            }),
        ];
        let stream_text: Vec<Result<StreamEvent, AppError>> = vec![
            Ok(StreamEvent::ContentDelta {
                delta: "done".to_string(),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "stop".to_string(),
            }),
        ];
        let llm = Arc::new(MockLlmProvider::new(vec![stream_tool, stream_text]));
        let primitive = Arc::new(MockPrimitiveExecutor);
        let event_bus = Arc::new(DefaultEventBus::new());
        let config = AgentLoopConfig {
            model: "gpt-4".to_string(),
            session_id: "s1".to_string(),
            ..Default::default()
        };
        let abort = Arc::new(AtomicBool::new(false));
        let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
        let messages = vec![AgentMessage::User {
            text: "read /tmp/x".to_string(),
        }];
        let result = loop_.run(messages).await.unwrap();
        assert!(result.final_text.contains("done"));
    }

    /// 边界：空消息列表不崩溃，run 仍可调用（LLM 可能返回错误或空回复）。
    #[tokio::test]
    async fn run_empty_messages_does_not_crash() {
        let stream1: Vec<Result<StreamEvent, AppError>> = vec![Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        })];
        let llm = Arc::new(MockLlmProvider::new(vec![stream1]));
        let primitive = Arc::new(MockPrimitiveExecutor);
        let event_bus = Arc::new(DefaultEventBus::new());
        let config = AgentLoopConfig {
            model: "gpt-4".to_string(),
            session_id: "s1".to_string(),
            ..Default::default()
        };
        let abort = Arc::new(AtomicBool::new(false));
        let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
        let messages: Vec<AgentMessage> = vec![];
        let result = loop_.run(messages).await;
        assert!(result.is_ok());
        assert!(result.unwrap().final_text.is_empty());
    }

    /// Abort：工具执行前/中设置 abort_signal，run 返回 Err，agent_end 含 interrupted。
    #[tokio::test]
    async fn run_aborts_returns_interrupted() {
        let stream_tools: Vec<Result<StreamEvent, AppError>> = vec![
            Ok(StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("c1".to_string()),
                name: Some("read_file".to_string()),
                arguments_delta: Some(r#"{"path":"/a"}"#.to_string()),
            }),
            Ok(StreamEvent::ToolCallDelta {
                index: 1,
                id: Some("c2".to_string()),
                name: Some("read_file".to_string()),
                arguments_delta: Some(r#"{"path":"/b"}"#.to_string()),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "tool_calls".to_string(),
            }),
        ];
        let llm = Arc::new(MockLlmProvider::new(vec![stream_tools]));
        let primitive = Arc::new(SleepyMockPrimitive);
        let event_bus = Arc::new(DefaultEventBus::new());
        let agent_end_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let err_clone = Arc::clone(&agent_end_error);
        event_bus.on(
            wire::WIRE_AGENT_END,
            Box::new(move |ctx: EventContext| {
                let err = ctx
                    .payload
                    .get("error")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                *err_clone.lock().unwrap() = err;
                Ok(())
            }),
        );
        let config = AgentLoopConfig {
            model: "gpt-4".to_string(),
            session_id: "s1".to_string(),
            ..Default::default()
        };
        let abort_signal = Arc::new(AtomicBool::new(false));
        let mut loop_ =
            AgentLoop::new(llm, primitive, event_bus, config, Arc::clone(&abort_signal));
        let messages = vec![AgentMessage::User {
            text: "read files".to_string(),
        }];
        let abort_for_thread = Arc::clone(&abort_signal);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            abort_for_thread.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let result = loop_.run(messages).await;
        assert!(result.is_err());
        let captured = agent_end_error.lock().unwrap().take();
        assert_eq!(captured.as_deref(), Some("interrupted"));
    }

    /// 事件顺序：纯文本一轮，断言 agent_start -> turn_start -> message_start -> message_update* -> message_end -> turn_end -> agent_end。
    #[tokio::test]
    async fn run_emits_events_in_correct_order() {
        let stream1: Vec<Result<StreamEvent, AppError>> = vec![
            Ok(StreamEvent::ContentDelta {
                delta: "x".to_string(),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "stop".to_string(),
            }),
        ];
        let llm = Arc::new(MockLlmProvider::new(vec![stream1]));
        let primitive = Arc::new(MockPrimitiveExecutor);
        let event_bus = Arc::new(DefaultEventBus::new());
        let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let expected: Vec<String> = vec![
            wire::WIRE_AGENT_START.into(),
            wire::WIRE_TURN_START.into(),
            wire::WIRE_MESSAGE_START.into(),
            wire::WIRE_MESSAGE_UPDATE.into(),
            wire::WIRE_MESSAGE_END.into(),
            wire::WIRE_TURN_END.into(),
            wire::WIRE_AGENT_END.into(),
        ];
        for ev in &expected {
            let list = Arc::clone(&order);
            let name = ev.clone();
            event_bus.on(
                &name,
                Box::new(move |ctx: EventContext| {
                    list.lock().unwrap().push(ctx.event_name.clone());
                    Ok(())
                }),
            );
        }
        let config = AgentLoopConfig {
            model: "gpt-4".to_string(),
            session_id: "s1".to_string(),
            ..Default::default()
        };
        let abort = Arc::new(AtomicBool::new(false));
        let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
        let messages = vec![AgentMessage::User {
            text: "hi".to_string(),
        }];
        let _ = loop_.run(messages).await.unwrap();
        let observed = order.lock().unwrap().clone();
        assert_eq!(observed, expected);
    }

    /// Steering：第 1 个工具执行后注入 steering，第 2 个工具不执行，下一轮 LLM 收到 steering 后返回文本。
    #[tokio::test]
    async fn run_steering_skips_remaining_tools() {
        let stream_tools: Vec<Result<StreamEvent, AppError>> = vec![
            Ok(StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("c1".to_string()),
                name: Some("read_file".to_string()),
                arguments_delta: Some(r#"{"path":"/a"}"#.to_string()),
            }),
            Ok(StreamEvent::ToolCallDelta {
                index: 1,
                id: Some("c2".to_string()),
                name: Some("read_file".to_string()),
                arguments_delta: Some(r#"{"path":"/b"}"#.to_string()),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "tool_calls".to_string(),
            }),
        ];
        let stream_text: Vec<Result<StreamEvent, AppError>> = vec![
            Ok(StreamEvent::ContentDelta {
                delta: "steered".to_string(),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "stop".to_string(),
            }),
        ];
        let llm = Arc::new(MockLlmProvider::new(vec![stream_tools, stream_text]));
        let steering_queue = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let read_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let primitive = Arc::new(SteerableMockPrimitive {
            steering_queue: Arc::clone(&steering_queue),
            read_count: Arc::clone(&read_count),
        });
        let event_bus = Arc::new(DefaultEventBus::new());
        let config = AgentLoopConfig {
            model: "gpt-4".to_string(),
            session_id: "s1".to_string(),
            ..Default::default()
        };
        let abort = Arc::new(AtomicBool::new(false));
        let mut loop_ = AgentLoop::new_with_steering_queue(
            llm,
            primitive,
            event_bus,
            config,
            abort,
            steering_queue,
        );
        let messages = vec![AgentMessage::User {
            text: "read two files".to_string(),
        }];
        let result = loop_.run(messages).await.unwrap();
        assert!(result.final_text.contains("steered"));
        assert_eq!(read_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// FollowUp：run 前先 follow_up("next")，同一上下文继续，final_text 含两轮回复。
    #[tokio::test]
    async fn run_follow_up_continues_in_same_context() {
        let stream_a: Vec<Result<StreamEvent, AppError>> = vec![
            Ok(StreamEvent::ContentDelta {
                delta: "A".to_string(),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "stop".to_string(),
            }),
        ];
        let stream_b: Vec<Result<StreamEvent, AppError>> = vec![
            Ok(StreamEvent::ContentDelta {
                delta: "B".to_string(),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "stop".to_string(),
            }),
        ];
        let llm = Arc::new(MockLlmProvider::new(vec![stream_a, stream_b]));
        let primitive = Arc::new(MockPrimitiveExecutor);
        let event_bus = Arc::new(DefaultEventBus::new());
        let config = AgentLoopConfig {
            model: "gpt-4".to_string(),
            session_id: "s1".to_string(),
            ..Default::default()
        };
        let abort = Arc::new(AtomicBool::new(false));
        let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
        loop_.follow_up("next".to_string());
        let messages = vec![AgentMessage::User {
            text: "first".to_string(),
        }];
        let result = loop_.run(messages).await.unwrap();
        // run() 返回最后一轮结果；第一轮为 "A"，follow_up 后第二轮为 "B"
        assert!(result.final_text.contains("B"));
    }

    /// Fatal 401：chat_stream 直接返回 Err，run 立即终止并返回含 401 的错误。
    #[tokio::test]
    async fn run_fatal_401_terminates_immediately() {
        let llm = Arc::new(MockLlmProviderFatal {
            err: "API 错误 401: unauthorized".to_string(),
        });
        let primitive = Arc::new(MockPrimitiveExecutor);
        let event_bus = Arc::new(DefaultEventBus::new());
        let config = AgentLoopConfig {
            model: "gpt-4".to_string(),
            session_id: "s1".to_string(),
            ..Default::default()
        };
        let abort = Arc::new(AtomicBool::new(false));
        let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
        let messages = vec![AgentMessage::User {
            text: "hi".to_string(),
        }];
        let result = loop_.run(messages).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("401"));
    }

    #[tokio::test]
    async fn convert_to_llm_format_compaction_summary_as_user() {
        let messages = vec![AgentMessage::CompactionSummary {
            summary: "Earlier messages summarized.".to_string(),
        }];
        let out = convert_to_llm_format(&messages);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0].role,
            crate::core::llm::ChatMessageRole::User
        ));
        assert_eq!(out[0].text_content(), Some("Earlier messages summarized."));
    }

    #[tokio::test]
    async fn agent_messages_from_chat_roundtrip() {
        use crate::core::llm::{ChatMessage, ChatMessageRole};
        let chat = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("u"),
            ChatMessage::assistant("a"),
        ];
        let agent = agent_messages_from_chat(&chat);
        let back = convert_to_llm_format(&agent);
        assert_eq!(back.len(), 3);
        assert_eq!(back[0].role, ChatMessageRole::System);
        assert_eq!(back[0].text_content(), Some("sys"));
        assert_eq!(back[1].role, ChatMessageRole::User);
        assert_eq!(back[1].text_content(), Some("u"));
        assert_eq!(back[2].role, ChatMessageRole::Assistant);
        assert_eq!(back[2].text_content(), Some("a"));
    }

    /// chat_stream 直接返回 Err（非 stream 内 Err）时也被正确分类并终止。
    #[tokio::test]
    async fn run_chat_stream_returns_err_is_classified() {
        let llm = Arc::new(MockLlmProviderFatal {
            err: "API 错误 503: service unavailable".to_string(),
        });
        let primitive = Arc::new(MockPrimitiveExecutor);
        let event_bus = Arc::new(DefaultEventBus::new());
        let config = AgentLoopConfig {
            model: "gpt-4".to_string(),
            session_id: "s1".to_string(),
            ..Default::default()
        };
        let abort = Arc::new(AtomicBool::new(false));
        let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
        let messages = vec![AgentMessage::User {
            text: "hi".to_string(),
        }];
        let result = loop_.run(messages).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("503"));
    }
}
