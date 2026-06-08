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
//! │   max_attempts     ► Retryable 最大重试次数（默认 4）                           │
//! │   max_tool_rounds  ► 单次 Attempt 工具轮次上限（默认 usize::MAX，不硬限）             │
//! │   retry_base_delay ► 指数退避基准延迟 ms（默认 500；含 jitter/cap）               │
//! │   tool_definitions ► 传入 LLM 的工具 JSON Schema 列表                         │
//! ├──────────────────────────────────────────────────────────────────────────────┤
//! │ 运行时状态                                                                     │
//! │   steering_queue  ─► Mutex<Vec<ChatMessage>>（跨线程注入 steering）            │
//! │   follow_up_queue ─► Mutex<Vec<ChatMessage>>（同上下文追问）                  │
//! │   abort_signal    ─► AtomicBool（Ctrl+C 中断）                                │
//! │   （流式 delta 通过 EventBus message_update 事件推送到渲染层）                 │
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
//! │   │     │       ├── ContentDelta ──► content_buf                             │
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
//! │   │     │         ├── read / list_dir / write_file                            │
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
//!   AppError::LlmDetailed(stage/http_status/summary)
//!     │
//!     ├── http_status == 401
//!     │       └──► LoopError::Fatal      →  立即终止，agent_end(error)
//!     │
//!     ├── is_context_overflow(&err)
//!     │       └──► LoopError::Retryable  →  可触发 L3 截断后再试
//!     │
//!     ├── http_status == 400（且非上下文溢出）
//!     │       └──► LoopError::Fatal
//!     │
//!     ├── retryable stage / 429 / 500 / 502 / 503 / 504
//!     │       └──► LoopError::Retryable  →  第二层指数退避重试
//!     │
//!     └── 其他
//!             └──► LoopError::Fatal
//! ```
//!
//! ## 消息类型
//!
//! 消息统一使用 `ChatMessage`（OpenAI wire format），通过 `MessageKind` 字段区分语义：
//!
//! ```text
//!   ChatMessage (role + kind)
//!   ──────────────────────────────────────────────
//!   role=user,      kind=Normal             普通用户消息
//!   role=user,      kind=Steering           Steering 指令
//!   role=user,      kind=CompactionSummary  Compaction 摘要
//!   role=system                             System prompt
//!   role=assistant                          LLM 回复 (含 tool_calls)
//!   role=tool                               工具执行结果
//! ```

mod accessors;
pub mod config_backend;
mod current_tail_guard;
mod error_classifier;
mod reasoning_loop;
mod run;
mod steering_injection;
mod stream_handler;
mod tool_dispatcher;
mod tool_exec;
mod turn_finalize;
mod types;

#[cfg(test)]
mod tests;

pub use config_backend::{ConfigBackend, SharedConfigBackend};
pub use current_tail_guard::{build_collapse_summary_artifacts_for_test, CollapseSummaryArtifacts};
pub use types::{
    AgentLoop, AgentLoopConfig, AgentRunOutcome, AgentRunResult, BackgroundCompletionRoutes,
    CompletionRoute, LoopError, SubagentType, ToolCallInfo,
};
