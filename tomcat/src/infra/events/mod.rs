//! # 事件枚举 (AgentEvent / ExtensionEvent)
//!
//! pi-mono 协议层的事件契约。所有 Agent 生命周期、流式输出、工具调用、自动压缩、
//! 上下文溢出、用户中断等可观测事件都在本文件以强类型 enum 定义；EventBus 用
//! `serde_json` 把它们序列化成 wire 格式的 JSON 给插件 / TUI / 审计消费。
//!
//! ## 三层结构
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────────────────┐
//! │  pub mod wire                ① Wire 字面量层（防散落字面量）             │
//! │  pub const WIRE_*: &str = "agent_start" | "turn_end" | ...              │
//! │  ─ 业务侧只引用本常量；测试断言、审计日志、pi-mono 对齐都走它           │
//! └────────────────────────────────────────────────────────────────────────┘
//!                              │
//!                              │ 与 enum 变体一一对应
//!                              ▼
//! ┌────────────────────────────────────────────────────────────────────────┐
//! │  #[serde(tag="type", rename_all="snake_case")]   ② 强类型层            │
//! │  pub enum AgentEvent {                                                  │
//! │    AgentStart / AgentEnd / Interrupted              （生命周期）        │
//! │    TurnStart / TurnEnd                              （回合）            │
//! │    MessageStart / MessageUpdate / MessageEnd        （流式 delta）      │
//! │    ToolExecutionStart / ToolExecutionUpdate /                           │
//! │      ToolExecutionEnd                               （工具时序）        │
//! │    AutoRetryStart / AutoRetryEnd                    （重试）            │
//! │    AutoCompactionStart / AutoCompactionEnd /                            │
//! │      CompactionError                                （Layer-1 压缩）    │
//! │    ContextOverflowTrimStart / ...End                （Layer-3 截断）    │
//! │    Layer0ContextRelease / BoundarySwitched /                            │
//! │      ContextMetricsUpdate / ToolResultTruncated /                       │
//! │      ToolResultPersisted                             （上下文记账）     │
//! │    ExtensionError                                    （扩展异常）       │
//! │  }                                                                      │
//! │                                                                         │
//! │  pub enum ExtensionEvent {                                              │
//! │    ToolCall / ToolResult                  （插件 hook，对 ToolExecution│
//! │                                            *Start/End 的镜像，便于扩展 │
//! │                                            侧只订阅业务语义事件）       │
//! │  }                                                                      │
//! └────────────────────────────────────────────────────────────────────────┘
//!                              │
//!                              │ ScopedEventEmitter::emit* 序列化信封
//!                              ▼
//! ┌────────────────────────────────────────────────────────────────────────┐
//! │  ③ Wire JSON 层（pi-mono 协议）                                          │
//! │  { "type": "tool_execution_end",                                        │
//! │    "sessionId": "...",         ← payload 字段一律 camelCase            │
//! │    "toolCallId": "call_abc",                                            │
//! │    "isError": false,                                                    │
//! │    ... }                                                                │
//! └────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## 序列化合约（**修改 enum 时必须满足**）
//!
//! - 顶层 `tag = "type"` + `rename_all = "snake_case"`：变体名自动转 wire 名
//!   （e.g. `ToolExecutionEnd` → `"tool_execution_end"`）。
//! - payload 字段必须 `#[serde(rename_all = "camelCase")]` 显式标注，与
//!   pi-mono `AgentEvent.ts` 的字段命名严格对齐。
//! - 新增变体时同步在 [`wire`] 模块加 `WIRE_*` 常量，避免业务直接写字面量。
//! - 测试覆盖：本文件 `#[cfg(test)] mod tests` 对每个变体做"snake_case +
//!   camelCase"双向 snapshot。
//!
//! ## 包装类型 ([`Message`] / [`AssistantMessage`] / [`ToolOutput`] / ...)
//!
//! 用 `pub struct Foo(pub serde_json::Value)` 让 LLM 报文与工具结果在 `AgentEvent`
//! 中以"任意 JSON"携带，不强制 wire schema——避免 LLM provider 升级时全链路
//! 改 enum；强类型断言留在调用方（如 `agent_loop::reasoning_loop`）。

use crate::core::tools::primitive::FileDiffLine;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// JSON `type` 字段与 pi-mono / 审计展示用字符串；业务与测试请引用此处常量，避免散落字面量。
pub mod wire {
    // --- AgentEvent（`#[serde(tag = "type", rename_all = "snake_case")]` 下的线格式名）---
    pub const WIRE_AGENT_START: &str = "agent_start";
    pub const WIRE_AGENT_END: &str = "agent_end";
    /// `AgentEvent::AgentIdle` 的 JSON `type`：整轮任务真正收敛、会话已回到 idle。
    /// 与 `agent_end` 的区别是：后者可能先于最外层任务尾声发出，前者只在 `mark_idle()`
    /// 之后发，用作 UI「可再次发送」的权威信号。
    pub const WIRE_AGENT_IDLE: &str = "agent_idle";
    pub const WIRE_TURN_START: &str = "turn_start";
    pub const WIRE_TURN_END: &str = "turn_end";
    pub const WIRE_MESSAGE_START: &str = "message_start";
    pub const WIRE_MESSAGE_UPDATE: &str = "message_update";
    pub const WIRE_MESSAGE_END: &str = "message_end";
    pub const WIRE_LLM_ERROR: &str = "llm_error";
    pub const WIRE_LLM_NOTICE: &str = "llm_notice";
    /// `AgentEvent::ToolExecutionStart` 的 JSON `type`（pi-mono 观察向）。
    pub const WIRE_TOOL_EXECUTION_START: &str = "tool_execution_start";
    /// `AgentEvent::ToolCallStreaming` 的 JSON `type`：tool call 参数仍在流式到达时的轻量预告。
    pub const WIRE_TOOL_CALL_STREAMING: &str = "tool_call_streaming";
    pub const WIRE_TOOL_EXECUTION_UPDATE: &str = "tool_execution_update";
    /// `AgentEvent::ToolExecutionEnd` 的 JSON `type`（pi-mono 观察向）。
    pub const WIRE_TOOL_EXECUTION_END: &str = "tool_execution_end";
    /// `ExtensionEvent::ToolCall` 的 JSON `type`（pi-mono 扩展钩子）。
    pub const WIRE_TOOL_CALL: &str = "tool_call";
    /// `ExtensionEvent::ToolResult` 的 JSON `type`（pi-mono 扩展钩子）。
    pub const WIRE_TOOL_RESULT: &str = "tool_result";
    pub const WIRE_AUTO_COMPACTION_START: &str = "auto_compaction_start";
    pub const WIRE_AUTO_COMPACTION_END: &str = "auto_compaction_end";
    pub const WIRE_COMPACTION_ERROR: &str = "compaction_error";
    pub const WIRE_TOOL_RESULT_TRUNCATED: &str = "tool_result_truncated";
    pub const WIRE_AUTO_RETRY_START: &str = "auto_retry_start";
    pub const WIRE_AUTO_RETRY_END: &str = "auto_retry_end";
    pub const WIRE_CONTEXT_METRICS_UPDATE: &str = "context_metrics_update";
    pub const WIRE_TOOL_RESULT_PERSISTED: &str = "tool_result_persisted";
    pub const WIRE_BOUNDARY_SWITCHED: &str = "boundary_switched";
    pub const WIRE_CONTEXT_OVERFLOW_TRIM_START: &str = "context_overflow_trim_start";
    pub const WIRE_CONTEXT_OVERFLOW_TRIM_END: &str = "context_overflow_trim_end";
    pub const WIRE_LAYER0_CONTEXT_RELEASE: &str = "layer0_context_release";
    pub const WIRE_EXTENSION_ERROR: &str = "extension_error";
    /// `tomcat chat` 入口后台预检 search_files Tier1 依赖（rg/fd）的状态更新。
    pub const WIRE_SEARCH_TOOLS_PREFLIGHT: &str = "search_tools_preflight";
    /// `tomcat chat` 入口后台预检 git 的状态更新。
    pub const WIRE_GIT_PREFLIGHT: &str = "git_preflight";
    /// `AgentEvent::Interrupted` 的 JSON `type`：用户中断（Ctrl+C 软中断）。
    /// 与现有 `AgentEnd { error: Some("interrupted") }` **并存**——前者供需要区分
    /// "失败 vs 中断"的订阅者使用，后者保留给原有订阅者做向后兼容。
    pub const WIRE_AGENT_INTERRUPTED: &str = "agent_interrupted";
    /// `AgentEvent::SubAgentStart` 的 JSON `type`（multi-agent §14.5）：
    /// 父 Agent 通过 `AgentRegistry::spawn_subagent_internal` 派生子 Agent 时发射。
    pub const WIRE_SUB_AGENT_START: &str = "sub_agent_start";
    /// `AgentEvent::SubAgentEnd` 的 JSON `type`（multi-agent §14.5）：
    /// 子 Agent run() 收敛 / abort / fatal 时发射。
    pub const WIRE_SUB_AGENT_END: &str = "sub_agent_end";

    // --- transcript 自定义事件 type（plan §P0.5 / §7.3 注册口径） ---
    //
    // 与 `TranscriptEntry::Custom` 中的 `extra.event` 字段对齐，并不出现在 `AgentEvent`
    // 枚举（这些是 transcript 落盘语义，不发到 EventBus）。集中常量化以避免字面量散落。
    //
    // 写入路径：`PlanRuntime` / `reviewer` 子流程在落盘后通过 `append_entry` 写 `Custom`
    // 行；读路径：hydrate / 审计在反序列化时直接以 `extra.event == 这些常量之一` 分流。
    /// `~/.tomcat/plans/<slug>_<hash>.plan.md` 落盘成功。
    pub const WIRE_PLAN_CREATE: &str = "plan.create";
    /// `/plan build` 成功把 plan 绑定到当前 runtime 并把盘 state 切到 `executing`。
    pub const WIRE_PLAN_BUILD: &str = "plan.build";
    /// `/plan` 进入 Planning 模式。
    pub const WIRE_PLAN_ENTER: &str = "plan.enter";
    /// `/plan exit` 退回 Chat 模式。
    pub const WIRE_PLAN_EXIT: &str = "plan.exit";
    /// `update_plan` 成功写盘（普通更新 / reopen / finalize 收口都复用此事件）。
    pub const WIRE_PLAN_UPDATE: &str = "plan.update";
    /// 已完成 / 中断后的 plan 被 reopen 到 `pending`。
    pub const WIRE_PLAN_PENDING: &str = "plan.pending";
    /// reviewer 子 Agent 返回（含 `aborted: true` 分支）。
    pub const WIRE_PLAN_REVIEW: &str = "plan.review";
    /// verifier 前 code reviewer 子 Agent 返回。
    pub const WIRE_PLAN_CODE_REVIEW: &str = "plan.code_review";
    /// verifier 子 Agent 返回（含 `verdict=aborted` / `partial` / `fail` 分支）。
    pub const WIRE_PLAN_VERIFY: &str = "plan.verify";
    /// reviewer parse 失败 / 超 `max_review_rounds` 软上限时的告警。
    pub const WIRE_PLAN_REVIEW_WARNING: &str = "plan.review.warning";
    /// code reviewer 跳过 / rounds 用尽等告警。
    pub const WIRE_PLAN_CODE_REVIEW_WARNING: &str = "plan.code_review.warning";
    /// `ask_question` 相关事件基名：一方面用于 transcript 记录工具完成（含 cancelled），
    /// 另一方面供 AskQuestionPanel ↔ host 的 request/response bridge 复用同名基前缀。
    pub const WIRE_PLAN_ASK_QUESTION: &str = "plan.ask_question";
    /// `todos` / `update_plan` 写入完成。
    pub const WIRE_PLAN_TODOS: &str = "plan.todos";
    /// `TodosPanel` 节流刷新快照（用户可见但不进 LLM 上下文）。
    pub const WIRE_PLAN_PANEL: &str = "plan.panel";
    /// PlanRuntime `mode → completed` 派生时落痕。
    pub const WIRE_PLAN_COMPLETE: &str = "plan.complete";
    /// session 标题异步 LLM 覆盖后推送。
    pub const WIRE_SESSION_TITLE_UPDATED: &str = "session.title_updated";
    /// `todos` 工具写入 session scratchpad 后推送。
    pub const WIRE_SESSION_TODOS: &str = "session.todos";
    /// turn 折叠标题异步 utility 覆盖后推送。
    pub const WIRE_TURN_SUMMARY_UPDATED: &str = "turn.summary_updated";
    /// 单条工具（bash）标题异步 utility 覆盖后推送。
    pub const WIRE_TOOL_SUMMARY_UPDATED: &str = "tool.summary_updated";
    /// 后台 bash 任务真正结束后推送，供前端把 bash 卡片从 loading 翻成完成。
    pub const WIRE_BACKGROUND_TASK_FINISHED: &str = "background_task_finished";

    // --- ExtensionEvent ---
    pub const WIRE_STARTUP: &str = "startup";
    pub const WIRE_SESSION_BEFORE_SWITCH: &str = "session_before_switch";
    pub const WIRE_SESSION_BEFORE_FORK: &str = "session_before_fork";
    pub const WIRE_INPUT: &str = "input";

    // --- 审计 kind_label（与 serde `kind` 一致；tool_call 与事件线格式同名）---
    pub const WIRE_AUDIT_PRIMITIVE: &str = "primitive";
    pub const WIRE_AUDIT_HOSTCALL: &str = "hostcall";
    pub const WIRE_AUDIT_PLUGIN_LIFECYCLE: &str = "plugin_lifecycle";

    /// VM / dispatcher 协议中与 AgentEvent 无关的 `event_type`（如 waitForEvent 信封）。
    pub mod vm {
        pub const WIRE_SESSION_START: &str = "session_start";
        /// 宿主向 JS 侧发起命令执行请求（长生命周期 VM async main loop 机制）。
        pub const WIRE_COMMAND_INVOKE: &str = "command_invoke";
    }
}

/// 占位：与 pi-mono / OpenAI 风格 Message 对齐，MVP 用 JSON 表示。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Message(pub serde_json::Value);

/// 占位：Assistant 消息流式事件。
///
/// **payload schema（T2-P0-006 P3 起）**：
///
/// ```json
/// {
///   "kind": "content_delta" | "thinking_delta",   // 必有；老消费者可忽略
///   "delta": "...",                                 // 必有：增量文本
///   "source": "summary" | "raw",                   // thinking_delta 必有
///   "signature": "..."                              // 可选：仅 thinking_delta + Anthropic
/// }
/// ```
///
/// 兼容性：老订阅者只读 `delta` 时仍能拿到正文增量（thinking 不会被推到旧路径，
/// 除非订阅者显式按 `kind` 分流）。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AssistantMessageEvent(pub serde_json::Value);

/// 占位：工具输出。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ToolOutput(pub serde_json::Value);

/// 工具执行成功后给 CLI/TUI 的结构化展示提示。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolDisplay {
    File {
        file: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        added: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        removed: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        diff: Option<Vec<FileDiffLine>>,
    },
    Plan {
        plan: String,
    },
    Text {
        text: String,
    },
}

/// transcript `Custom` 行里 plan.* 事件共用的最小 payload。
///
/// - `event` 字段由调用方单独附加（如 `plan.create` / `plan.build` / `plan.update`）
/// - `state` 仅作 fast cache / 审计展示；recover 仍以盘 `frontmatter.state` 为准
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanEventPayload {
    pub plan_id: String,
    pub path: String,
    pub state: String,
}

/// 占位：AssistantMessage。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AssistantMessage(pub serde_json::Value);

/// 占位：ToolResultMessage。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ToolResultMessage(pub serde_json::Value);

/// 占位：ContentBlock。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ContentBlock(pub serde_json::Value);

/// 占位：ImageContent。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ImageContent(pub serde_json::Value);

/// 领域事件的 wire 信封：把 sessionId 作为 envelope 元数据统一附加在顶层 JSON。
///
/// 仅供 crate 内部 emitter 使用；外部调用方不应直接序列化该类型绕过统一发射路径。
#[derive(Debug, Serialize)]
pub(super) struct WireEnvelope<'a> {
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    session_id: Option<&'a str>,
    #[serde(flatten)]
    event: &'a AgentEvent,
}

impl<'a> WireEnvelope<'a> {
    pub(super) fn new(session_id: Option<&'a str>, event: &'a AgentEvent) -> Self {
        Self { session_id, event }
    }
}

/// serve `--print-schema` 使用的事件 wire 入口：与 [`WireEnvelope`] 同形，但用拥有所有权的字段
/// 参与 `JsonSchema` / fixture 导出，不影响运行时 event bus 发射路径。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct WireEvent {
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(flatten)]
    pub event: AgentEvent,
}

/// 扩展事件的 wire 信封：与 [`WireEnvelope`] 同语义，但载荷为 [`ExtensionEvent`]。
#[derive(Debug, Serialize)]
pub(super) struct ExtensionWireEnvelope<'a> {
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    session_id: Option<&'a str>,
    #[serde(flatten)]
    event: &'a ExtensionEvent,
}

impl<'a> ExtensionWireEnvelope<'a> {
    pub(super) fn new(session_id: Option<&'a str>, event: &'a ExtensionEvent) -> Self {
        Self { session_id, event }
    }
}

/// 宿主侧流式/UI 与生命周期事件，供前端或日志消费。
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Agent 会话开始。
    AgentStart,
    /// Agent 会话结束，含消息与可选错误。
    AgentEnd {
        messages: Vec<Message>,
        error: Option<String>,
    },
    /// 会话已真正回到 idle，可再次接收新的 prompt。
    ///
    /// 与 `AgentEnd` 的区别：
    /// - `AgentEnd` 表示本次 attempt / 会话流已给出终态结果；
    /// - `AgentIdle` 表示最外层会话任务已经完整收尾，`busy=false` 已生效。
    ///
    /// 该事件主要供 IDE / Webview 把「中断」按钮可靠切回「发送」使用。
    AgentIdle,
    TurnStart {
        #[serde(rename = "turnIndex")]
        turn_index: usize,
        timestamp: i64,
    },
    TurnEnd {
        #[serde(rename = "turnIndex")]
        turn_index: usize,
        message: Message,
        #[serde(rename = "toolResults")]
        tool_results: Vec<Message>,
        #[serde(rename = "assistantMessageId", skip_serializing_if = "Option::is_none")]
        assistant_message_id: Option<String>,
        #[serde(rename = "toolCallIds", skip_serializing_if = "Vec::is_empty", default)]
        tool_call_ids: Vec<String>,
        #[serde(rename = "summaryTitle", skip_serializing_if = "Option::is_none")]
        summary_title: Option<String>,
    },
    MessageStart {
        message: Message,
        #[serde(rename = "assistantMessageId")]
        assistant_message_id: String,
    },
    MessageUpdate {
        message: Message,
        #[serde(rename = "assistantMessageId")]
        assistant_message_id: String,
        #[serde(rename = "assistantMessageEvent")]
        assistant_message_event: AssistantMessageEvent,
    },
    MessageEnd {
        message: Message,
        #[serde(rename = "assistantMessageId")]
        assistant_message_id: String,
    },
    /// LLM 终局错误（如 Responses `response.failed` / 顶层 `error` / `content_filter`）。
    LlmError {
        reason: String,
        #[serde(rename = "errorCode", skip_serializing_if = "Option::is_none")]
        error_code: Option<String>,
        #[serde(rename = "errorMessage")]
        error_message: String,
    },
    /// LLM 非错误终局提示（当前用于 `max_output_tokens` 截断轻提示）。
    LlmNotice {
        #[serde(rename = "finishReason")]
        finish_reason: String,
        message: String,
    },
    /// 线格式 `tool_execution_start`（观察向）；钩子 `tool_call` 见 `ExtensionEvent::ToolCall`。
    ToolExecutionStart {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        args: serde_json::Value,
    },
    /// 线格式 `tool_call_streaming`：参数仍在流式到达时的轻量预告；只带摘要，不带大 payload。
    ToolCallStreaming {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        #[serde(rename = "argsPreview")]
        args_preview: serde_json::Value,
    },
    ToolExecutionUpdate {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        args: serde_json::Value,
        #[serde(rename = "partialResult")]
        partial_result: ToolOutput,
    },
    /// 线格式 `tool_execution_end`（观察向）；钩子 `tool_result` 见 `ExtensionEvent::ToolResult`。
    ToolExecutionEnd {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        result: ToolOutput,
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<ToolDisplay>,
        #[serde(rename = "isError")]
        is_error: bool,
    },
    AutoCompactionStart {
        #[serde(rename = "coveredCount")]
        covered_count: usize,
        #[serde(rename = "ratioBefore")]
        ratio_before: f64,
    },
    AutoCompactionEnd {
        #[serde(rename = "elapsedMs")]
        elapsed_ms: u64,
        #[serde(rename = "summaryChars")]
        summary_chars: usize,
        #[serde(rename = "coveredCount")]
        covered_count: usize,
        #[serde(rename = "ratioAfter")]
        ratio_after: f64,
        #[serde(rename = "estimatedCoveredTokensBefore")]
        estimated_covered_tokens_before: usize,
        #[serde(rename = "estimatedSummaryTokens")]
        estimated_summary_tokens: usize,
        #[serde(rename = "estimatedTokensSaved")]
        estimated_tokens_saved: usize,
    },
    CompactionError {
        #[serde(rename = "exhaustedAfterRetries")]
        exhausted_after_retries: bool,
        attempts: u32,
        error: String,
        source: String,
        ratio: Option<f64>,
    },
    ToolResultTruncated {
        #[serde(rename = "toolName")]
        tool_name: String,
        #[serde(rename = "originalChars")]
        original_chars: usize,
        #[serde(rename = "truncatedChars")]
        truncated_chars: usize,
    },
    AutoRetryStart {
        attempt: u32,
        #[serde(rename = "maxAttempts")]
        max_attempts: u32,
        #[serde(rename = "delayMs")]
        delay_ms: u64,
        #[serde(rename = "errorMessage")]
        error_message: String,
    },
    AutoRetryEnd {
        success: bool,
        attempt: u32,
        #[serde(rename = "finalError")]
        final_error: Option<String>,
    },
    /// 扩展/插件触发错误，含扩展 ID、事件名与错误信息。
    ExtensionError {
        #[serde(rename = "extensionId")]
        extension_id: Option<String>,
        event: String,
        error: String,
    },
    ContextMetricsUpdate {
        #[serde(rename = "inputTokensUsed")]
        input_tokens_used: usize,
        #[serde(rename = "contextUtilizationRatio")]
        context_utilization_ratio: f64,
        #[serde(rename = "compactionCount")]
        compaction_count: u32,
        #[serde(rename = "compactionTokensFreed")]
        compaction_tokens_freed: usize,
        #[serde(rename = "totalToolResultBytesPersisted")]
        total_tool_result_bytes_persisted: usize,
        #[serde(rename = "preheatInProgress")]
        preheat_in_progress: bool,
        #[serde(rename = "preheatResultPending")]
        preheat_result_pending: bool,
    },
    ToolResultPersisted {
        #[serde(rename = "toolName")]
        tool_name: String,
        #[serde(rename = "originalChars")]
        original_chars: usize,
        #[serde(rename = "persistedPath")]
        persisted_path: String,
    },
    ContextOverflowTrimStart {
        reason: String,
        ratio: f64,
    },
    ContextOverflowTrimEnd {
        #[serde(rename = "ratioBefore")]
        ratio_before: f64,
        #[serde(rename = "ratioAfter")]
        ratio_after: f64,
        #[serde(rename = "willRetry")]
        will_retry: bool,
        #[serde(rename = "estimatedTokensFreed")]
        estimated_tokens_freed: usize,
        #[serde(rename = "turnsRemoved")]
        turns_removed: usize,
    },
    BoundarySwitched {
        #[serde(rename = "ratioBefore")]
        ratio_before: f64,
        #[serde(rename = "ratioAfter")]
        ratio_after: f64,
        #[serde(rename = "coveredCount")]
        covered_count: usize,
        #[serde(rename = "wasSyncWait")]
        was_sync_wait: bool,
        #[serde(rename = "estimatedTokensFreed")]
        estimated_tokens_freed: usize,
    },
    /// L0 落盘 + 占位符在本轮 timing ⑤ 释放的估算 tokens（不计入 L1/L2）。
    Layer0ContextRelease {
        #[serde(rename = "persistTokensFreed")]
        persist_tokens_freed: usize,
        #[serde(rename = "placeholderTokensFreed")]
        placeholder_tokens_freed: usize,
    },
    /// 用户中断（Soft Interrupt）：携带本回合已累积的 partial 尺寸统计，
    /// 便于订阅者区分"失败 vs 中断"。本事件与 `AgentEnd(interrupted)` **并存**，
    /// 后者保留向后兼容。
    #[serde(rename = "agent_interrupted")]
    Interrupted {
        /// partial assistant 累积字符数（非字节数）。
        #[serde(rename = "partialTextLen")]
        partial_text_len: usize,
        /// 本回合已追加到 messages 的 tool_result 数量。
        #[serde(rename = "toolResultsCount")]
        tool_results_count: usize,
    },
    /// 父 Agent 通过 `AgentRegistry::spawn_subagent_internal` 派生子 Agent 时发射。
    /// 用于审计 / TUI 关联父子关系；与 `Interrupted` 同档「描述生命周期」语义。
    ///
    /// 注：reviewer 子 Agent **不**写父 transcript（隔离），仅靠本事件让父侧观察到子的存在。
    SubAgentStart {
        #[serde(rename = "parentSessionId")]
        parent_session_id: String,
        #[serde(rename = "childSessionId")]
        child_session_id: String,
        /// `SubagentType::as_str()`（如 `"reviewer"`）。
        #[serde(rename = "subagentType")]
        subagent_type: String,
        #[serde(rename = "spawnDepth")]
        spawn_depth: u32,
    },
    /// 子 Agent run() 收敛 / abort / fatal 时发射；`outcome ∈ {"completed","interrupted","failed"}`。
    SubAgentEnd {
        #[serde(rename = "parentSessionId")]
        parent_session_id: String,
        #[serde(rename = "childSessionId")]
        child_session_id: String,
        #[serde(rename = "subagentType")]
        subagent_type: String,
        outcome: String,
        /// 失败 / abort 时的简短理由（成功为 `None`）。
        #[serde(rename = "errorMessage", skip_serializing_if = "Option::is_none")]
        error_message: Option<String>,
    },
}

/// 扩展侧钩子事件，与 pi-mono 事件名一致（如 tool_call、input、session_before_switch）。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExtensionEvent {
    /// 宿主启动时通知扩展。
    #[serde(rename_all = "camelCase")]
    Startup {
        version: String,
        session_file: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    AgentStart,
    #[serde(rename_all = "camelCase")]
    AgentEnd {
        messages: Vec<Message>,
        error: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    TurnStart { turn_index: usize },
    #[serde(rename_all = "camelCase")]
    TurnEnd {
        turn_index: usize,
        message: AssistantMessage,
        tool_results: Vec<ToolResultMessage>,
    },
    /// 工具调用，扩展可在此拦截或记录。
    #[serde(rename_all = "camelCase")]
    ToolCall {
        tool_name: String,
        tool_call_id: String,
        input: serde_json::Value,
    },
    #[serde(rename_all = "camelCase")]
    ToolResult {
        tool_name: String,
        tool_call_id: String,
        input: serde_json::Value,
        content: Vec<ContentBlock>,
        details: Option<serde_json::Value>,
        is_error: bool,
    },
    #[serde(rename_all = "camelCase")]
    SessionBeforeSwitch {
        current_session: Option<String>,
        target_session: String,
    },
    #[serde(rename_all = "camelCase")]
    SessionBeforeFork {
        current_session: Option<String>,
        fork_entry_id: String,
    },
    /// 用户输入（文本与附件），扩展可在此做预处理。
    #[serde(rename_all = "camelCase")]
    Input {
        #[serde(rename = "text")]
        content: String,
        #[serde(rename = "images")]
        attachments: Vec<ImageContent>,
    },
}

#[cfg(test)]
mod tests;
