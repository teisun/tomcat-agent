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
    pub const WIRE_AUTO_RETRY_START: &str = "auto_retry_start";
    pub const WIRE_AUTO_RETRY_END: &str = "auto_retry_end";
    pub const WIRE_EXTENSION_ERROR: &str = "extension_error";

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
        reason: String,
    },
    AutoCompactionEnd {
        result: Option<serde_json::Value>,
        aborted: bool,
        #[serde(rename = "willRetry")]
        will_retry: bool,
        #[serde(rename = "errorMessage")]
        error_message: Option<String>,
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
mod tests {
    use super::*;

    #[test]
    fn agent_event_serialize_type_snake_case() {
        let e = AgentEvent::ExtensionError {
            extension_id: Some("ext-1".to_string()),
            event: wire::WIRE_TOOL_CALL.to_string(),
            error: "test".to_string(),
        };
        let j = serde_json::to_string(&e).unwrap();
        assert!(j.contains(wire::WIRE_EXTENSION_ERROR));
        assert!(j.contains("extensionId"));
    }

    #[test]
    fn agent_event_tool_execution_uses_pi_mono_wire_names() {
        let start = AgentEvent::ToolExecutionStart {
            tool_call_id: "c1".into(),
            tool_name: "read_file".into(),
            args: serde_json::json!({}),
        };
        let end = AgentEvent::ToolExecutionEnd {
            tool_call_id: "c1".into(),
            tool_name: "read_file".into(),
            result: ToolOutput(serde_json::json!({})),
            is_error: false,
        };
        assert_eq!(
            serde_json::to_value(&start).unwrap()["type"]
                .as_str()
                .unwrap(),
            wire::WIRE_TOOL_EXECUTION_START
        );
        assert_eq!(
            serde_json::to_value(&end).unwrap()["type"]
                .as_str()
                .unwrap(),
            wire::WIRE_TOOL_EXECUTION_END
        );
    }

    #[test]
    fn extension_event_tool_hooks_use_tool_call_tool_result_wire_names() {
        let call = ExtensionEvent::ToolCall {
            tool_name: "read_file".into(),
            tool_call_id: "c1".into(),
            input: serde_json::json!({}),
        };
        let result = ExtensionEvent::ToolResult {
            tool_name: "read_file".into(),
            tool_call_id: "c1".into(),
            input: serde_json::json!({}),
            content: vec![ContentBlock(serde_json::json!({"text": "ok"}))],
            details: None,
            is_error: false,
        };
        assert_eq!(
            serde_json::to_value(&call).unwrap()["type"]
                .as_str()
                .unwrap(),
            wire::WIRE_TOOL_CALL
        );
        assert_eq!(
            serde_json::to_value(&result).unwrap()["type"]
                .as_str()
                .unwrap(),
            wire::WIRE_TOOL_RESULT
        );
    }

    #[test]
    fn extension_event_serialize_camel_case() {
        let e = ExtensionEvent::Startup {
            version: "1.0".to_string(),
            session_file: None,
        };
        let j = serde_json::to_string(&e).unwrap();
        assert!(j.contains(wire::WIRE_STARTUP));
        assert!(j.contains("sessionFile"));
    }
}
