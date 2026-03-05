本文为 [Architecture](../Architecture.md) 中「事件系统设计」的详细设计，总览见主文档。

## 事件系统设计（替代原钩子设计，完全对齐pi-agent-rust）

### 核心设计原则
基于发布-订阅模式，全局事件总线，支持同步/异步事件监听，是宿主与插件、插件与插件之间通信的唯一方式，完全对齐pi-mono的事件规范。

### 事件分类（对齐 pi_agent_rust）

事件分为两类：**AgentEvent** 供流式/UI 订阅；**ExtensionEvent** 供扩展通过 `agent.on(event_name, ...)` 注册钩子。扩展侧使用**字符串事件名**（snake_case，如 `"tool_call"`、`"session_before_switch"`、`"input"`），与 pi-mono / pi_agent_rust 一致。序列化时 `type` 为 snake_case，payload 字段为 camelCase。

#### AgentEvent（流式 / UI）

用于 TUI、JSON 模式等，携带完整上下文；与 pi_agent_rust `agent.rs` 对齐。

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    AgentStart { #[serde(rename = "sessionId")] session_id: Arc<str> },
    AgentEnd { #[serde(rename = "sessionId")] session_id: Arc<str>, messages: Vec<Message>, error: Option<String> },
    TurnStart { #[serde(rename = "sessionId")] session_id: Arc<str>, #[serde(rename = "turnIndex")] turn_index: usize, timestamp: i64 },
    TurnEnd { #[serde(rename = "sessionId")] session_id: Arc<str>, #[serde(rename = "turnIndex")] turn_index: usize, message: Message, #[serde(rename = "toolResults")] tool_results: Vec<Message> },
    MessageStart { message: Message },
    MessageUpdate { message: Message, #[serde(rename = "assistantMessageEvent")] assistant_message_event: AssistantMessageEvent },
    MessageEnd { message: Message },
    ToolExecutionStart { #[serde(rename = "toolCallId")] tool_call_id: String, #[serde(rename = "toolName")] tool_name: String, args: Value },
    ToolExecutionUpdate { #[serde(rename = "toolCallId")] tool_call_id: String, #[serde(rename = "toolName")] tool_name: String, args: Value, #[serde(rename = "partialResult")] partial_result: ToolOutput },
    ToolExecutionEnd { #[serde(rename = "toolCallId")] tool_call_id: String, #[serde(rename = "toolName")] tool_name: String, result: ToolOutput, #[serde(rename = "isError")] is_error: bool },
    AutoCompactionStart { reason: String },
    AutoCompactionEnd { result: Option<Value>, aborted: bool, #[serde(rename = "willRetry")] will_retry: bool, #[serde(rename = "errorMessage")] error_message: Option<String> },
    AutoRetryStart { attempt: u32, #[serde(rename = "maxAttempts")] max_attempts: u32, #[serde(rename = "delayMs")] delay_ms: u64, #[serde(rename = "errorMessage")] error_message: String },
    AutoRetryEnd { success: bool, attempt: u32, #[serde(rename = "finalError")] final_error: Option<String> },
    ExtensionError { #[serde(rename = "extensionId")] extension_id: Option<String>, event: String, error: String },
}
```

#### ExtensionEvent（扩展钩子）

与 pi_agent_rust `extension_events.rs` 一致的事件名与 payload；保留会话/插件/系统等扩展事件时同样使用 snake_case + camelCase。

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExtensionEvent {
    #[serde(rename_all = "camelCase")]
    Startup { version: String, session_file: Option<String> },
    #[serde(rename_all = "camelCase")]
    AgentStart { session_id: String },
    #[serde(rename_all = "camelCase")]
    AgentEnd { session_id: String, messages: Vec<Message>, error: Option<String> },
    #[serde(rename_all = "camelCase")]
    TurnStart { session_id: String, turn_index: usize },
    #[serde(rename_all = "camelCase")]
    TurnEnd { session_id: String, turn_index: usize, message: AssistantMessage, tool_results: Vec<ToolResultMessage> },
    #[serde(rename_all = "camelCase")]
    ToolCall { tool_name: String, tool_call_id: String, input: Value },
    #[serde(rename_all = "camelCase")]
    ToolResult { tool_name: String, tool_call_id: String, input: Value, content: Vec<ContentBlock>, details: Option<Value>, is_error: bool },
    #[serde(rename_all = "camelCase")]
    SessionBeforeSwitch { current_session: Option<String>, target_session: String },
    #[serde(rename_all = "camelCase")]
    SessionBeforeFork { current_session: Option<String>, fork_entry_id: String },
    #[serde(rename_all = "camelCase")]
    Input { #[serde(rename = "text")] content: String, #[serde(rename = "images")] attachments: Vec<ImageContent> },
    // 保留：会话/插件/系统/4原语等扩展事件，命名同上
    // SessionCreate, SessionDestroy, SessionSwitch, PluginLoad/Unload/Enable/Disable, ToolRegister/Unregister, ToolCallError, SystemReady, SystemShutdown, ConfigChange, Custom(String) 等
}
```

### 事件执行机制

- 宿主在关键节点发布 **AgentEvent**（流式/UI）与 **ExtensionEvent**（扩展钩子），携带完整上下文
- 扩展通过 `agent.on("tool_call", ...)` 等**字符串事件名**监听 ExtensionEvent，与 pi-mono 一致
- 按注册顺序执行回调，支持同步/异步；单次回调错误不影响其他回调与主流程
- 扩展通过 `agent.emit()` 发布自定义事件（如 Custom 前缀），实现插件间通信
- 插件卸载时自动注销该插件所有监听，无泄漏
