# pi-ai 包

pi-ai（`@mariozechner/pi-ai`）提供统一的多 Provider LLM API，支持流式/非流式调用、Context/Tools、TypeBox 校验，以及跨 Provider 的会话交接与 Context 序列化。仅包含支持 tool calling 的模型，以支撑 Agent 工作流。

---

## 1. 模块职责

- **统一 API**：`stream` / `complete` / `streamSimple` / `completeSimple`，屏蔽各厂商 API 差异。
- **多 Provider**：通过 Api 类型与 api-registry 注册，每个 Api 对应一个 Provider 实现（stream + streamSimple）。
- **Context 与消息**：`Context`（systemPrompt、messages、tools）、`Message`（user / assistant / toolResult）、统一事件类型 `AssistantMessageEvent`。
- **Tools**：TypeBox 定义参数 schema，校验与流式 partial 解析。
- **跨 Provider 交接**：Context 可序列化，便于在会话中切换模型（如先 OpenAI 后 Claude）。

---

## 2. 关键类型（types.ts）

- **Api / KnownApi**：API 标识，如 `openai-completions`、`anthropic-messages`、`google-generative-ai`、`bedrock-converse-stream` 等。
- **Provider / KnownProvider**：厂商名，如 `openai`、`anthropic`、`google`、`amazon-bedrock` 等。
- **Model&lt;TApi&gt;**：`id`、`name`、`api`、`provider`、`baseUrl`、`reasoning`、`input`（text/image）、`cost`、`contextWindow`、`maxTokens`、`compat`（OpenAI 兼容选项）等。
- **Context**：`systemPrompt?`、`messages: Message[]`、`tools?: Tool[]`。
- **Message**：`UserMessage`（role user，content 为 string 或 TextContent/ImageContent 数组）、`AssistantMessage`（content 含 TextContent/ThinkingContent/ToolCall，以及 usage、stopReason 等）、`ToolResultMessage`（toolCallId、toolName、content、isError）。
- **Tool**：`name`、`description`、`parameters`（TSchema/TypeBox）。
- **StreamOptions / SimpleStreamOptions**：`temperature`、`maxTokens`、`signal`、`apiKey`、`sessionId`、`cacheRetention`、`transport`、`reasoning`（Simple）、`thinkingBudgets` 等。
- **AssistantMessageEvent**：`start`、`text_start`/`text_delta`/`text_end`、`thinking_*`、`toolcall_start`/`toolcall_delta`/`toolcall_end`、`done`（含 message）、`error`（含 error message）。

---

## 3. 核心 API（stream.ts）

- **stream(model, context, options?)**：返回 `AssistantMessageEventStream`，使用 Provider 的完整 stream（可带各 Provider 特有 options）。
- **complete(model, context, options?)**：基于 stream 的 `s.result()` 得到最终 `AssistantMessage`。
- **streamSimple(model, context, options?)**：统一选项（含 `reasoning`、`thinkingBudgets`），Agent 层常用。
- **completeSimple(model, context, options?)**：同上，非流式。

内部通过 `getApiProvider(model.api)` 从 api-registry 解析出对应 Provider，再调用其 `stream` 或 `streamSimple`。

---

## 4. Provider 注册（api-registry.ts + providers/register-builtins.ts）

- **ApiProvider&lt;TApi, TOptions&gt;**：`api`、`stream`、`streamSimple`。每个 Api 对应一个实现。
- **registerApiProvider(provider, sourceId?)**：注册到全局 Map；**getApiProvider(api)**：供 stream.ts 解析。
- **register-builtins**：在 stream 被 import 时执行，注册内置 Api：anthropic-messages、openai-completions、openai-responses、azure-openai-responses、openai-codex-responses、google-generative-ai、google-gemini-cli、google-vertex、bedrock-converse-stream。

扩展或自定义 Provider 时，实现 `stream`/`streamSimple` 并在使用前调用 `registerApiProvider`（coding-agent 的扩展可注册自定义 Provider）。

---

## 5. 事件流（utils/event-stream.ts）

- **EventStream&lt;T, R&gt;**：通用异步事件队列，`push(event)`、`end(result?)`、`async *[Symbol.asyncIterator]`、`result(): Promise<R>`。
- **AssistantMessageEventStream**：继承 `EventStream<AssistantMessageEvent, AssistantMessage>`，完成条件为 `event.type === "done" || "error"`，最终结果从 `done.message` 或 `error.error` 提取。

Provider 实现中会构造此类 stream，按协议推送 `text_delta`、`toolcall_*`、`done` 等事件。

---

## 6. Tools 与 TypeBox

- Tool 的 `parameters` 使用 TypeBox（TSchema）定义，便于类型推导与 JSON Schema 校验。
- 库导出 `Type`、`Static`、`TSchema`；`validateToolArguments` / `validateToolCall` 用于在执行前校验 tool call 参数。
- 流式场景下 `toolcall_delta` 的 `event.partial.content[contentIndex]` 可能为 partial JSON，需防御性使用；`toolcall_end` 时参数完整但仍需校验后再执行。

---

## 7. 跨 Provider 交接与 Context 序列化

- **Context** 仅含 `systemPrompt`、`messages`、`tools`，均为可序列化结构；可 JSON 序列化后交给另一模型继续对话。
- 文档与示例中的「跨 Provider handoff」：先与模型 A 对话，将得到的 Context 序列化，再以模型 B 的 `stream(modelB, context)` 继续，实现会话迁移或接力。

---

## 8. 关键文件路径

| 文件 | 说明 |
|------|------|
| `packages/ai/src/stream.ts` | stream/complete/streamSimple/completeSimple 入口，解析 Provider 并调用 |
| `packages/ai/src/types.ts` | Api、Model、Context、Message、Tool、StreamOptions、AssistantMessageEvent 等类型 |
| `packages/ai/src/api-registry.ts` | registerApiProvider、getApiProvider、ApiProvider 接口 |
| `packages/ai/src/providers/register-builtins.ts` | 内置 Api 注册 |
| `packages/ai/src/utils/event-stream.ts` | EventStream、AssistantMessageEventStream |
| `packages/ai/src/env-api-keys.js` | getEnvApiKey 等鉴权辅助（stream 中 re-export） |

各具体 Provider 实现位于 `packages/ai/src/providers/*.ts`（如 anthropic、openai-completions、google、amazon-bedrock 等），每个导出 `stream*` 与 `streamSimple*`，内部将厂商 API 响应转换为统一的 `AssistantMessageEvent` 并 push 到 stream。
