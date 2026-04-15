# pi-agent-core 包

## 先用大白话

**pi-agent-core**（`@mariozechner/pi-agent-core`）是「**会干活**」的那一层：它已经假设模型有时会喊「帮我执行某个工具」。它负责：**记状态**、**调 pi-ai 问模型**、**解析 tool call**、**跑你注册的 TypeScript 工具**、再把结果塞回对话里；同时用一套事件告诉你界面上该显示什么。

把它想成**乐队指挥**：pi-ai 是小提琴（只负责拉出声音），agent-core 决定**谁先拉、谁后拉、中间插哪段独奏（tool）**。

---

## 往里说：模块职责

- **Agent 类**：持有 `AgentState`，对外 `prompt()`、`continue()`、`subscribe()`。
- **agent-loop**：核心循环（纯函数风格），内部调用 `streamSimple`（或注入的 `streamFn`）。
- **边界**：内部消息用 **AgentMessage**；每次请求模型前用 **`convertToLlm`** 变成 pi-ai 的 `Message[]`；可用 **`transformContext`** 裁剪或注入。

---

## ASCII：一轮里发生什么（极度简化）

```
  prompt() 进来
       |
       v
  +----+----+  agent_start / turn_start …
  | agentLoop |
  +----+----+
       |
       +--> streamSimple --> 模型返回文本或 tool 意图
       |
       +--> 有 tool? -- 是 --> executeTool(s) --> 把 tool 结果塞进消息
       |                    |
       +--------------------+--> 再问模型 … 直到本轮不再要 tool 或无更多 steering
       v
  agent_end
```

---

## 关键类型（`packages/agent/src/types.ts`）

- **AgentMessage**：标准 pi-ai 消息 + 可通过 declaration merging 扩展的自定义消息。
- **AgentState**：`systemPrompt`、`model`、`thinkingLevel`、`tools`、`messages`、流式中间状态等。
- **AgentTool**：在 pi-ai `Tool` 上加 `execute(...)`，返回内容与可选 `details`。
- **AgentEvent**：`agent_start`/`agent_end`、`turn_*`、`message_*`、`tool_execution_*` 等。
- **StreamFn**：可替换默认的 `streamSimple`（例如走代理）。

---

## Agent 类（`packages/agent/src/Agent.ts`）

- **steer / followUp**：在循环中插入「用户半路纠正」或「结束后再追加」类消息；有队列与 `steeringMode` / `followUpMode`（`all` 或 `one-at-a-time`）。
- **prompt(msgs)**：追加消息后进入 `agentLoop`。
- **continue()**：不追加新 user 消息，从当前上下文再跑一轮（用于重试等）。

---

## agent-loop（`packages/agent/src/agent-loop.ts`）

### agentLoop / agentLoopContinue

- **agentLoop**：合并新 prompt，发事件，进入 **runLoop**，结束时 `stream.end(newMessages)`。
- **agentLoopContinue**：要求最后一条不是 `assistant`（常见为停在 `toolResult` 后要续跑）。

### runLoop（核心）

- 外层处理 **follow-up** 队列；内层循环：**streamAssistantResponse**（问模型）→ 若有 **toolCalls** 则 **executeToolCalls**（跑工具、发 `tool_execution_*`）→ 处理 **steering**（可能打断后续 tool）。
- **streamAssistantResponse**：`transformContext` → `convertToLlm` → 调 `streamFn` 或默认 `streamSimple`，把流式事件映射为 `message_update` 等。

### executeToolCalls

- 按 name 找 `AgentTool`，`validateToolArguments`，执行 `execute`，收集 `ToolResultMessage`；若中途出现 steering，则提前返回由外层改 pending 消息。

细节以源码为准；本文只帮你建立**心智模型**。

---

## 事件顺序（典型）

一次 `prompt()` 大致：`agent_start` → `turn_start` → 用户消息 `message_start/end` → 助手 `message_start` → 多次 `message_update` → `message_end` → 若有工具则多组 `tool_execution_*` → `turn_end` → … → `agent_end`。

---

## 关键文件路径

| 文件 | 说明 |
|------|------|
| `packages/agent/src/Agent.ts` | Agent 类 |
| `packages/agent/src/agent-loop.ts` | 循环与 tool 执行 |
| `packages/agent/src/types.ts` | 类型定义 |

依赖 pi-ai 的 `streamSimple`、`Message`、`Context`、`Tool`、`validateToolArguments` 等。
