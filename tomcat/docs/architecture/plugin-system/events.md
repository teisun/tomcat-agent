本文为 [Architecture](../../Architecture.md) 中「事件系统设计」的详细设计，总览见主文档。

## 事件系统设计（替代原钩子设计，完全对齐pi-agent-rust）

### 核心设计原则
基于发布-订阅模式，全局事件总线，支持同步/异步事件监听，是宿主与插件、插件与插件之间通信的唯一方式，完全对齐pi-mono的事件规范。

### 事件分类（对齐 pi_agent_rust）

事件分为两类：**AgentEvent** 供流式/UI 订阅；**ExtensionEvent** 供扩展通过 `agent.on(event_name, ...)` 注册钩子。扩展侧使用**字符串事件名**（snake_case，如 `"tool_call"`、`"session_before_switch"`、`"input"`），与 pi-mono / pi_agent_rust 一致。序列化时 `type` 为 snake_case，payload 字段为 camelCase。自 D15 起，`sessionId` 由 `ScopedEventEmitter` 通过顶层 **wire envelope** 统一注入；Rust enum body 不再重复内嵌 `session_id`。协议/插件消费读顶层 `payload.sessionId`，进程内订阅者读 `EventContext.session_id`。

#### AgentEvent（流式 / UI）

用于 TUI、JSON 模式等，携带完整上下文；与 pi_agent_rust `agent.rs` 对齐。

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    AgentStart,
    AgentEnd { messages: Vec<Message>, error: Option<String> },
    TurnStart { #[serde(rename = "turnIndex")] turn_index: usize, timestamp: i64 },
    TurnEnd { #[serde(rename = "turnIndex")] turn_index: usize, message: Message, #[serde(rename = "toolResults")] tool_results: Vec<Message> },
    MessageStart { message: Message },
    MessageUpdate { message: Message, #[serde(rename = "assistantMessageEvent")] assistant_message_event: AssistantMessageEvent },
    MessageEnd { message: Message },
    ToolExecutionStart { #[serde(rename = "toolCallId")] tool_call_id: String, #[serde(rename = "toolName")] tool_name: String, args: Value },
    ToolExecutionUpdate { #[serde(rename = "toolCallId")] tool_call_id: String, #[serde(rename = "toolName")] tool_name: String, args: Value, #[serde(rename = "partialResult")] partial_result: ToolOutput },
    ToolExecutionEnd { #[serde(rename = "toolCallId")] tool_call_id: String, #[serde(rename = "toolName")] tool_name: String, result: ToolOutput, #[serde(rename = "isError")] is_error: bool },
    AutoCompactionStart { #[serde(rename = "coveredCount")] covered_count: usize, #[serde(rename = "ratioBefore")] ratio_before: f64 },
    AutoCompactionEnd { #[serde(rename = "elapsedMs")] elapsed_ms: u64, #[serde(rename = "summaryChars")] summary_chars: usize, #[serde(rename = "coveredCount")] covered_count: usize, #[serde(rename = "ratioAfter")] ratio_after: f64, #[serde(rename = "estimatedCoveredTokensBefore")] estimated_covered_tokens_before: usize, #[serde(rename = "estimatedSummaryTokens")] estimated_summary_tokens: usize, #[serde(rename = "estimatedTokensSaved")] estimated_tokens_saved: usize },
    CompactionError { #[serde(rename = "exhaustedAfterRetries")] exhausted_after_retries: bool, attempts: u32, error: String, source: String, ratio: Option<f64> },
    ToolResultTruncated { #[serde(rename = "toolName")] tool_name: String, #[serde(rename = "originalChars")] original_chars: usize, #[serde(rename = "truncatedChars")] truncated_chars: usize },
    AutoRetryStart { attempt: u32, #[serde(rename = "maxAttempts")] max_attempts: u32, #[serde(rename = "delayMs")] delay_ms: u64, #[serde(rename = "errorMessage")] error_message: String },
    AutoRetryEnd { success: bool, attempt: u32, #[serde(rename = "finalError")] final_error: Option<String> },
    ExtensionError { #[serde(rename = "extensionId")] extension_id: Option<String>, event: String, error: String },
    ContextMetricsUpdate { #[serde(rename = "inputTokensUsed")] input_tokens_used: usize, #[serde(rename = "contextUtilizationRatio")] context_utilization_ratio: f64, #[serde(rename = "compactionCount")] compaction_count: u32, #[serde(rename = "compactionTokensFreed")] compaction_tokens_freed: usize, #[serde(rename = "totalToolResultBytesPersisted")] total_tool_result_bytes_persisted: usize, #[serde(rename = "preheatInProgress")] preheat_in_progress: bool, #[serde(rename = "preheatResultPending")] preheat_result_pending: bool },
    ToolResultPersisted { #[serde(rename = "toolName")] tool_name: String, #[serde(rename = "originalChars")] original_chars: usize, #[serde(rename = "persistedPath")] persisted_path: String },
    Layer0ContextRelease { #[serde(rename = "persistTokensFreed")] persist_tokens_freed: usize, #[serde(rename = "placeholderTokensFreed")] placeholder_tokens_freed: usize },
    ContextOverflowTrimStart { reason: String, ratio: f64 },
    ContextOverflowTrimEnd { #[serde(rename = "ratioBefore")] ratio_before: f64, #[serde(rename = "ratioAfter")] ratio_after: f64, #[serde(rename = "willRetry")] will_retry: bool, #[serde(rename = "estimatedTokensFreed")] estimated_tokens_freed: usize, #[serde(rename = "turnsRemoved")] turns_removed: usize },
    BoundarySwitched { #[serde(rename = "ratioBefore")] ratio_before: f64, #[serde(rename = "ratioAfter")] ratio_after: f64, #[serde(rename = "coveredCount")] covered_count: usize, #[serde(rename = "wasSyncWait")] was_sync_wait: bool, #[serde(rename = "estimatedTokensFreed")] estimated_tokens_freed: usize },
    // 其余变体（如 LlmError / LlmNotice / ToolCallStreaming / Interrupted / SubAgentStart / SubAgentEnd）见源码 events/mod.rs
}
```

对应 wire JSON 形状示例：

```json
{
  "type": "turn_start",
  "sessionId": "sess_123",
  "turnIndex": 1,
  "timestamp": 1710000000
}
```

**上下文压缩与溢出（线格式名与 payload）**：L0（落盘+占位符）、异步预热线（L1）、边界切换（L2）、溢出裁剪（L3）对应下列 JSON `type`；序列化字段为 camelCase，与实现一致。以下事件**不再**存在于代码库：`preheat_started`、`preheat_completed`、`preheat_error`、`compaction_circuit_breaker_triggered`。

| 层级 | JSON `type` | Payload |
|------|-------------|---------|
| L0（timing ⑤） | `layer0_context_release` | `{ "persistTokensFreed": number, "placeholderTokensFreed": number }`（估算 tok，已计入会话 `compactionTokensFreed`） |
| L1（async preheat） | `auto_compaction_start` | `{ "coveredCount": number, "ratioBefore": number }` |
| L1 | `auto_compaction_end` | `{ "elapsedMs", "summaryChars", "coveredCount", "ratioAfter", "estimatedCoveredTokensBefore", "estimatedSummaryTokens", "estimatedTokensSaved" }`；在 **L1 后台任务**于 `append_entry` 成功（或无 transcript 路径）后发射一次；`ratioAfter` 为预热启动时的利用率快照（与 L2 apply 前主线程读到的 ratio 可能不同）；**不在此事件时**累加会话 `compactionTokensFreed`。**L2** `apply_boundary` **不再**发射 `auto_compaction_end`。 |
| L1（失败耗尽） | `compaction_error` | `{ "exhaustedAfterRetries": boolean, "attempts": number, "error": string, "source": string, "ratio": number \| null }`；典型 `source: "preheat"` |
| L2（apply 失败） | `compaction_error` | 同上；`source: "apply"`，`exhaustedAfterRetries: false` |
| L2 | `boundary_switched` | `{ ratioBefore, ratioAfter, coveredCount, wasSyncWait, estimatedTokensFreed }`（`estimatedTokensFreed` 等于 L1 写入 transcript 的 `estimatedTokensSaved`，apply 成功时计入会话累计） |
| L3（overflow trim） | `context_overflow_trim_start` | `{ "reason": string, "ratio": number }` |
| L3 | `context_overflow_trim_end` | `{ "ratioBefore", "ratioAfter", "willRetry", "estimatedTokensFreed", "turnsRemoved" }` |

**`context_metrics_update`**：累计字段来自 `ContextState::session_obs`（与 `sessions.json` 在 user turn 结束时同步）；瞬时字段来自 `ContextState::live`。`preheatInProgress`：LLM 预热任务仍在跑（`Running` 且 JoinHandle 未完成）。`preheatResultPending`：摘要已就绪、尚未被 `poll_result` 消费（`CachedCompleted`，或 `Running` 且 handle 已完成）；与 `preheatInProgress` 互斥。CLI 可对指标行做中英双语两行展示；`compactionTokensFreed` 在展示文案中可与英文 `saved` 对齐。`totalToolResultBytesPersisted` 字段名历史兼容，**实际为 Unicode 字符累计**（L0 落盘原始长度之和）。

**`wire` 模块常量**（与上表 JSON `type` 一一对应，定义见 [`events/mod.rs`](../../../../src/infra/events/mod.rs) `pub mod wire`）：`WIRE_AUTO_COMPACTION_START`、`WIRE_AUTO_COMPACTION_END`、`WIRE_COMPACTION_ERROR`、`WIRE_BOUNDARY_SWITCHED`、`WIRE_CONTEXT_OVERFLOW_TRIM_START`、`WIRE_CONTEXT_OVERFLOW_TRIM_END`、`WIRE_LAYER0_CONTEXT_RELEASE`。

#### ExtensionEvent（扩展钩子）

与 pi_agent_rust `extension_events.rs` 一致的事件名与 payload；保留会话/插件/系统等扩展事件时同样使用 snake_case + camelCase。

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExtensionEvent {
    #[serde(rename_all = "camelCase")]
    Startup { version: String, session_file: Option<String> },
    #[serde(rename_all = "camelCase")]
    AgentStart,
    #[serde(rename_all = "camelCase")]
    AgentEnd { messages: Vec<Message>, error: Option<String> },
    #[serde(rename_all = "camelCase")]
    TurnStart { turn_index: usize },
    #[serde(rename_all = "camelCase")]
    TurnEnd { turn_index: usize, message: AssistantMessage, tool_results: Vec<ToolResultMessage> },
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

`ExtensionEvent` 的 `sessionId` 同样来自顶层 envelope，而不是 enum body：

```json
{
  "type": "tool_call",
  "sessionId": "sess_123",
  "toolName": "read_file",
  "toolCallId": "call_1",
  "input": { "path": "README.md" }
}
```

**线格式名（JSON `type`）**：`AgentEvent` 使用 `#[serde(tag = "type", rename_all = "snake_case")]`，故 `ToolExecutionStart` / `ToolExecutionEnd` / `ToolExecutionUpdate` 的 `type` 分别为 **`tool_execution_start`**、**`tool_execution_end`**、**`tool_execution_update`**（观察向，与 pi-mono 流式/UI 一致）。扩展钩子 **`tool_call`** / **`tool_result`** 仅用于 **`ExtensionEvent::ToolCall` / `ToolResult`**，与上述观察事件名不同；其余 Agent 线格式名及 `WIRE_*` 常量见源码 [`events/mod.rs`](../../../../src/infra/events/mod.rs) `pub mod wire`。

### 与 pi-mono 工具链事件对照

pi-mono `ExtensionAPI`（`packages/coding-agent` 中 `extensions/types.ts`）中工具相关有五个事件名：**观察向**（`tool_execution_*`）与**钩子向**（`tool_call` / `tool_result`）分离。宿主在 Agent 循环内对单次工具调用按时间顺序发布（同一条 `EventBus`，不同 JSON `type` 与 payload 形状）：

```text
tool_execution_start  →  tool_call  →  [execute_tool]  →  tool_result  →  tool_execution_end
   AgentEvent            ExtensionEvent                    ExtensionEvent    AgentEvent
```

```mermaid
sequenceDiagram
    participant Loop as AgentLoop
    participant Bus as EventBus
    participant Hook as ExtensionHook
    Loop->>Bus: tool_execution_start (AgentEvent)
    Loop->>Bus: tool_call (ExtensionEvent)
    Bus-->>Hook: pi.on("tool_call")
    Loop->>Loop: execute_tool
    Loop->>Bus: tool_result (ExtensionEvent)
    Bus-->>Hook: pi.on("tool_result")
    Loop->>Bus: tool_execution_end (AgentEvent)
```

- **观察向**：UI / 日志订阅 `tool_execution_start`、`tool_execution_end`（及可选 `tool_execution_update`），表示「工具生命周期」。
- **钩子向**：扩展订阅 `tool_call`（执行前）、`tool_result`（执行后）；pi-mono 中可 block / 改写结果，本仓库当前阶段以**发射事件**为主，拦截语义见 [pi-mono-compat-strategy.md §13](pi-mono-compat-strategy.md) 与 `feature-plugin-compat-tier1.md`。
- **VM 路径**：`PluginManager::dispatch_session_event` 透传 `event_type` 至长生命周期 VM，与 `EventBus::emit_sync` **并行**；插件 JS 是否收到与 `emit_sync` 同源的事件取决于桥接实现，详见 compat 文档。

### 事件执行机制

- 宿主在关键节点发布 **AgentEvent**（流式/UI）与 **ExtensionEvent**（扩展钩子），携带完整上下文
- 扩展通过 `agent.on("tool_call", ...)` 等**字符串事件名**监听 ExtensionEvent，与 pi-mono 一致
- 按注册顺序执行回调，支持同步/异步；单次回调错误不影响其他回调与主流程
- 扩展通过 `agent.emit()` 发布自定义事件（如 Custom 前缀），实现插件间通信
- 插件卸载时自动注销该插件所有监听，无泄漏

---

**导航**：返回 [插件系统全貌](../plugin-system-overview.md) | 上一节：[异步 Hostcall 与事件循环](async-hostcall-event-loop.md) | 下一节：[JS API 与 pi-mono 对齐](js-api-alignment.md)
