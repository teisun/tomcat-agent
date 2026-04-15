# pi-ai 包

## 先用大白话

**pi-ai**（npm：`@mariozechner/pi-ai`）是一块「万能转接头」：你的程序只想用**同一套**函数去聊天、流式输出、传图片、注册工具（tool calling），背后到底是 OpenAI、Anthropic、Google 还是 Bedrock，都由它来翻译。

它**只收录支持 tool calling 的模型**，因为下游 Agent 工作流默认模型会「要工具」。

---

## 往里说：模块职责

- **统一入口**：`stream` / `complete` / `streamSimple` / `completeSimple`，屏蔽各厂商 HTTP/SDK 差异。
- **Provider 注册**：每种 `Api` 对应一份实现（`stream` + `streamSimple`），在 **`register-builtins`** 里懒加载注册。
- **Context**：`systemPrompt`、多轮 `messages`、可选 `tools`；整体可序列化，方便换模型继续聊（handoff）。
- **Tools**：用 **TypeBox** 描述参数 schema，便于校验与流式 partial JSON 解析。

---

## ASCII：一次调用经过哪里

```
  你的代码                    pi-ai                     具体 Provider 文件
      |                        |                              |
      +-- streamSimple() ----->+-- getApiProvider(api) ------>+-- 发 HTTP / SDK
      |                        |                              |
      +<-- AssistantMessageEvent 流（text_delta、toolcall_* …）-+
```

---

## 关键类型（`packages/ai/src/types.ts`）

- **Api**：字符串联合，标识走哪条协议实现（如 `openai-responses`、`anthropic-messages`）。
- **Model&lt;TApi&gt;**：`id`、`name`、`api`、`provider`、`contextWindow`、`maxTokens`、`reasoning` 能力等。
- **Context / Message / Tool**：送给模型的上下文与工具定义。
- **AssistantMessageEvent**：`start`、`text_*`、`thinking_*`、`toolcall_*`、`done`、`error` 等（具体名字以类型为准）。

---

## 核心 API（`packages/ai/src/stream.ts`）

- **stream**：最全的 Provider 专属 options。
- **streamSimple / completeSimple**：Agent 常用；选项统一到 `SimpleStreamOptions`（含 `reasoning`、`thinkingBudgets` 等）。
- **complete / completeSimple**：在 stream 上等到最终结果。

内部：`getApiProvider(model.api)` → 调用对应实现的 `stream` 或 `streamSimple`。

---

## Provider 注册（`api-registry.ts` + `providers/register-builtins.ts`）

- **registerApiProvider**：扩展或测试可注入自定义实现。
- **内置 Api**（以仓库 `register-builtins.ts` 为准，含 Mistral）：  
  `anthropic-messages`、`openai-completions`、`openai-responses`、`azure-openai-responses`、`openai-codex-responses`、`google-generative-ai`、`google-gemini-cli`、`google-vertex`、`mistral-conversations`、`bedrock-converse-stream`。

若你维护文档：发版前请 **grep** `registerApiProvider` / `register-builtins` 与本文列举是否仍一致。

---

## 事件流（`packages/ai/src/utils/event-stream.ts`）

- **EventStream&lt;T, R&gt;**：异步迭代、`push`、`end`、`result()`。
- **AssistantMessageEventStream**：消费到 `done` 或 `error` 即结束；最终结果从 `done.message` 或错误里取。

Provider 负责把厂商响应切成统一事件往里 `push`。

---

## Tools 与 TypeBox

- `parameters` 用 TypeBox（`TSchema`）；导出 `validateToolArguments` / `validateToolCall`。
- 流式 `toolcall_delta` 里可能出现 **未拼完的 JSON**，执行前要等到完整并校验。

---

## 跨 Provider 交接

把 `Context` JSON 化或深拷贝后，换另一个 `Model` 再 `stream`，即可让会话「换模型接力」。

---

## 关键文件路径（查代码用）

下面这张表是给**要改 pi-ai 或对照实现**的人用的。

| 文件 | 说明 |
|------|------|
| `packages/ai/src/stream.ts` | 对外 stream/complete/streamSimple |
| `packages/ai/src/types.ts` | Api、Model、Context、Message、事件类型 |
| `packages/ai/src/api-registry.ts` | 注册表 |
| `packages/ai/src/providers/register-builtins.ts` | 内置 Provider 懒加载注册 |
| `packages/ai/src/utils/event-stream.ts` | EventStream |
| `packages/ai/src/env-api-keys.ts` | 环境变量/API Key 探测（Node） |

各 Provider 实现位于 `packages/ai/src/providers/`。
