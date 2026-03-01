//! 事件枚举 AgentEvent / ExtensionEvent，与 Architecture.md 事件系统设计一致。
//! type snake_case，payload camelCase。

use serde::Serialize;

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

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    AgentStart {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
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
    MessageStart { message: Message },
    MessageUpdate {
        message: Message,
        #[serde(rename = "assistantMessageEvent")]
        assistant_message_event: AssistantMessageEvent,
    },
    MessageEnd { message: Message },
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
    ToolExecutionEnd {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        result: ToolOutput,
        #[serde(rename = "isError")]
        is_error: bool,
    },
    AutoCompactionStart { reason: String },
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
    ExtensionError {
        #[serde(rename = "extensionId")]
        extension_id: Option<String>,
        event: String,
        error: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExtensionEvent {
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
            event: "tool_call".to_string(),
            error: "test".to_string(),
        };
        let j = serde_json::to_string(&e).unwrap();
        assert!(j.contains("extension_error"));
        assert!(j.contains("extensionId"));
    }

    #[test]
    fn extension_event_serialize_camel_case() {
        let e = ExtensionEvent::Startup {
            version: "1.0".to_string(),
            session_file: None,
        };
        let j = serde_json::to_string(&e).unwrap();
        assert!(j.contains("startup"));
        assert!(j.contains("sessionFile"));
    }
}
