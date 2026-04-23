//! # 事件枚举 (AgentEvent / ExtensionEvent)
//!
//! 与 Architecture 事件系统设计一致：type 使用 snake_case，payload 字段使用 camelCase。
//! 扩展侧使用字符串事件名，与 pi-mono 对齐。

use serde::Serialize;

/// JSON `type` 字段与 pi-mono / 审计展示用字符串；业务与测试请引用此处常量，避免散落字面量。
pub mod wire {
    // --- AgentEvent（`#[serde(tag = "type", rename_all = "snake_case")]` 下的线格式名）---
    pub const WIRE_AGENT_START: &str = "agent_start";
    pub const WIRE_AGENT_END: &str = "agent_end";
    pub const WIRE_TURN_START: &str = "turn_start";
    pub const WIRE_TURN_END: &str = "turn_end";
    pub const WIRE_MESSAGE_START: &str = "message_start";
    pub const WIRE_MESSAGE_UPDATE: &str = "message_update";
    pub const WIRE_MESSAGE_END: &str = "message_end";
    /// `AgentEvent::ToolExecutionStart` 的 JSON `type`（pi-mono 观察向）。
    pub const WIRE_TOOL_EXECUTION_START: &str = "tool_execution_start";
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
    /// `AgentEvent::Interrupted` 的 JSON `type`：用户中断（Ctrl+C 软中断）。
    /// 与现有 `AgentEnd { error: Some("interrupted") }` **并存**——前者供需要区分
    /// "失败 vs 中断"的订阅者使用，后者保留给原有订阅者做向后兼容。
    pub const WIRE_AGENT_INTERRUPTED: &str = "agent_interrupted";

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

/// 占位：与 pi 系 Message 对齐，MVP 用 JSON 表示。
#[derive(Debug, Clone, Serialize)]
pub struct Message(pub serde_json::Value);

/// 占位：Assistant 消息流式事件。
#[derive(Debug, Clone, Serialize)]
pub struct AssistantMessageEvent(pub serde_json::Value);

/// 占位：工具输出。
#[derive(Debug, Clone, Serialize)]
pub struct ToolOutput(pub serde_json::Value);

/// 占位：AssistantMessage。
#[derive(Debug, Clone, Serialize)]
pub struct AssistantMessage(pub serde_json::Value);

/// 占位：ToolResultMessage。
#[derive(Debug, Clone, Serialize)]
pub struct ToolResultMessage(pub serde_json::Value);

/// 占位：ContentBlock。
#[derive(Debug, Clone, Serialize)]
pub struct ContentBlock(pub serde_json::Value);

/// 占位：ImageContent。
#[derive(Debug, Clone, Serialize)]
pub struct ImageContent(pub serde_json::Value);

/// 宿主侧流式/UI 与生命周期事件，供前端或日志消费。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Agent 会话开始。
    AgentStart {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    /// Agent 会话结束，含消息与可选错误。
    AgentEnd {
        #[serde(rename = "sessionId")]
        session_id: String,
        messages: Vec<Message>,
        error: Option<String>,
    },
    TurnStart {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "turnIndex")]
        turn_index: usize,
        timestamp: i64,
    },
    TurnEnd {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "turnIndex")]
        turn_index: usize,
        message: Message,
        #[serde(rename = "toolResults")]
        tool_results: Vec<Message>,
    },
    MessageStart {
        message: Message,
    },
    MessageUpdate {
        message: Message,
        #[serde(rename = "assistantMessageEvent")]
        assistant_message_event: AssistantMessageEvent,
    },
    MessageEnd {
        message: Message,
    },
    /// 线格式 `tool_execution_start`（观察向）；钩子 `tool_call` 见 `ExtensionEvent::ToolCall`。
    ToolExecutionStart {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        args: serde_json::Value,
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
        #[serde(rename = "sessionId")]
        session_id: String,
        /// partial assistant 累积字符数（非字节数）。
        #[serde(rename = "partialTextLen")]
        partial_text_len: usize,
        /// 本回合已追加到 messages 的 tool_result 数量。
        #[serde(rename = "toolResultsCount")]
        tool_results_count: usize,
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
    AgentStart { session_id: String },
    #[serde(rename_all = "camelCase")]
    AgentEnd {
        session_id: String,
        messages: Vec<Message>,
        error: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    TurnStart {
        session_id: String,
        turn_index: usize,
    },
    #[serde(rename_all = "camelCase")]
    TurnEnd {
        session_id: String,
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
