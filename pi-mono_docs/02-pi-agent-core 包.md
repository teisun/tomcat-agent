# pi-agent-core 包

pi-agent-core（`@mariozechner/pi-agent-core`）提供有状态 Agent 运行时：在 pi-ai 之上封装 tool 执行与事件流，支持 `convertToLlm`/`transformContext`、steering/follow-up 队列，供 coding-agent、web-ui 等上层使用。

---

## 1. 模块职责

- **Agent 类**：持有 `AgentState`（systemPrompt、model、thinkingLevel、tools、messages），对外提供 `prompt()`、`continue()`、`subscribe()`，内部将调用交给 agent-loop。
- **agent-loop**：纯函数式循环，接收 `AgentContext` 与 `AgentLoopConfig`，通过 `streamSimple` 调用 LLM，执行 tool、注入 steering/follow-up 消息，并推送 `AgentEvent`。
- **消息与 LLM 边界**：全程使用 `AgentMessage`（可含自定义类型）；仅在每次 LLM 调用前通过 `convertToLlm` 转为 pi-ai 的 `Message[]`；可选 `transformContext` 做裁剪或注入。

---

## 2. 关键类型（types.ts）

- **AgentMessage**：`Message | CustomAgentMessages[keyof CustomAgentMessages]`，即 user/assistant/toolResult + 通过 declaration merging 扩展的自定义消息。
- **AgentState**：`systemPrompt`、`model`、`thinkingLevel`、`tools`、`messages`、`isStreaming`、`streamMessage`、`pendingToolCalls`、`error?`。
- **AgentContext**：`systemPrompt`、`messages`、`tools?`（AgentTool 数组）。
- **AgentLoopConfig**：继承 `SimpleStreamOptions`，并包含 `model`、`convertToLlm`、`transformContext?`、`getApiKey?`、`getSteeringMessages?`、`getFollowUpMessages?`。
- **AgentTool&lt;TParameters, TDetails&gt;**：继承 pi-ai 的 `Tool`，增加 `label` 与 `execute(toolCallId, params, signal?, onUpdate?)`，返回 `AgentToolResult&lt;TDetails&gt;`（content 块 + details）。
- **AgentEvent**：`agent_start`/`agent_end`、`turn_start`/`turn_end`、`message_start`/`message_update`/`message_end`、`tool_execution_start`/`tool_execution_update`/`tool_execution_end`。
- **StreamFn**：与 `streamSimple` 同签名的函数类型，用于注入自定义 stream（如代理后端）。

---

## 3. Agent 类（Agent.ts）

- **构造**：`AgentOptions` 可设置 `initialState`、`convertToLlm`（默认仅保留 user/assistant/toolResult）、`transformContext`、`steeringMode`/`followUpMode`、`streamFn`、`sessionId`、`getApiKey`、`thinkingBudgets`、`transport`、`maxRetryDelayMs`。
- **状态**：`state` 只读；`setSystemPrompt`、`setModel`、`setThinkingLevel`、`setTools`、`replaceMessages`、`appendMessage`。
- **队列**：`steer(msg)` 插入 steering（在 tool 执行后可中断并注入）；`followUp(msg)` 在 agent 自然结束后再注入；`clearSteeringQueue`/`clearFollowUpQueue`；`steeringMode`/`followUpMode` 为 `"all"` 或 `"one-at-a-time"`。
- **订阅**：`subscribe(fn)` 返回取消函数；所有 `AgentEvent` 均转发给 listeners。
- **prompt(msgs)**：将 messages 追加到 state，调用 `agentLoop(prompts, context, config, signal, streamFn)`，消费返回的 `EventStream` 并 push 到内部 stream、通知 listeners，最后用 `stream.result()` 更新 state.messages。
- **continue()**：不追加新消息，调用 `agentLoopContinue(context, config, ...)`，用于错误重试或从上次断点继续。

---

## 4. agent-loop（agent-loop.ts）

### 4.1 agentLoop(prompts, context, config, signal?, streamFn?)

- 创建 `EventStream<AgentEvent, AgentMessage[]>`，将 `prompts` 并入 `currentContext.messages`，推送 `agent_start`、`turn_start`、对每个 prompt 的 `message_start`/`message_end`，然后进入 **runLoop**。
- 返回的 stream 在 runLoop 结束时以 `agent_end` + `stream.end(newMessages)` 收尾，`stream.result()` 为最终 `AgentMessage[]`。

### 4.2 agentLoopContinue(context, config, signal?, streamFn?)

- 要求 `context.messages` 非空且最后一条不是 `assistant`（即必须是 user 或 toolResult），用于“从当前 context 再跑一轮”而不加新 user 消息。
- 同样进入 runLoop，不预先追加 prompts。

### 4.3 runLoop（核心循环）

- **外层 while(true)**：在“本轮无 tool call 且无 steering”后，检查 `getFollowUpMessages()`；若有则作为 pending 继续内层循环，否则 break 并结束。
- **内层 while(hasMoreToolCalls || pendingMessages.length)**：
  - 可选 `turn_start`（首轮除外）。
  - 若有 **pendingMessages**（steering 或 follow-up）：逐个 push `message_start`/`message_end` 并追加到 `currentContext.messages`。
  - **streamAssistantResponse**：对当前 context 先 `transformContext`（若有），再 `convertToLlm`，构造 pi-ai `Context`，调用 `streamFn || streamSimple`，遍历响应事件，更新 `context.messages` 最后一条并 push `message_update`，结束时 push `message_end`，得到 `AssistantMessage`。
  - 若 `stopReason` 为 error/aborted，直接 `turn_end`、`agent_end`、`stream.end` 并 return。
  - 从 message.content 取出 **toolCalls**，若有则 **executeToolCalls**：对每个 tool call 校验、查找 AgentTool、push `tool_execution_start`、执行 `tool.execute`（可 push `tool_execution_update`）、push `tool_execution_end`，收集 `toolResults`；若执行过程中 `getSteeringMessages` 返回内容则记录为 `steeringAfterTools`，后续跳过剩余 tool、将 steering 作为 pendingMessages。
  - push `turn_end`，然后从 `getSteeringMessages()` 或 `steeringAfterTools` 取下一批 pendingMessages，继续内层循环。
- 内层结束后若无 follow-up 则 break 外层，最后 push `agent_end` 并 `stream.end(newMessages)`。

### 4.4 streamAssistantResponse

- 将 `AgentContext` 经 `transformContext` → `convertToLlm` 得到 `Message[]`，拼成 pi-ai `Context`，调用 `streamFunction(model, llmContext, options)`。
- 遍历返回的 stream：`start` 时写入 partial 到 context.messages 并 push `message_start`；各类 delta/end 更新 context 最后一条并 push `message_update`；`done`/`error` 时取 `response.result()` 写回 context 并 push `message_end`，返回最终 `AssistantMessage`。

### 4.5 executeToolCalls

- 遍历 assistant message 中的 `toolCall`，根据 name 查找 `AgentTool`，校验参数（validateToolArguments），push `tool_execution_start`，调用 `tool.execute(..., onUpdate)`，根据结果 push `tool_execution_update`（若有）、`tool_execution_end`，收集 `ToolResultMessage`；若某次循环中 `getSteeringMessages?.()` 有值则记录并返回，外层会跳过后续 tool 并将这些消息作为 pending。

---

## 5. 事件流小结

- **prompt() 一次调用的典型顺序**：`agent_start` → `turn_start` → 每个新 user 消息的 `message_start`/`message_end` → assistant 的 `message_start` → 多个 `message_update`（text_delta/toolcall_* 等）→ `message_end` → 若有 tool：多个 `tool_execution_start`/`tool_execution_update`/`tool_execution_end` → `turn_end` → 若继续 turn 则重复 turn_start → ... → 最后 `agent_end`。
- **continue()** 无新 user 消息，直接从 `turn_start` 开始，逻辑同上。

---

## 6. 关键文件路径

| 文件 | 说明 |
|------|------|
| `packages/agent/src/Agent.ts` | Agent 类、state、subscribe、prompt/continue、steer/followUp、与 agent-loop 的对接 |
| `packages/agent/src/agent-loop.ts` | agentLoop、agentLoopContinue、runLoop、streamAssistantResponse、executeToolCalls |
| `packages/agent/src/types.ts` | AgentMessage、AgentState、AgentContext、AgentLoopConfig、AgentTool、AgentEvent、StreamFn |

Agent 依赖 pi-ai 的 `streamSimple`、`getModel`、`Message`/`Context`/`Tool`/`AssistantMessage`/`ToolResultMessage`/`validateToolArguments` 等。
