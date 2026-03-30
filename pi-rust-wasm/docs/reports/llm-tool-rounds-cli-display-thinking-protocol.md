# LLM 工具轮次、CLI 展示与 Thinking 协议研究报告

> 日期：2026-03-29
> 范围：pi-rust-wasm 现状分析 + pi-mono / openclaw 设计对照 + 业界方案 + 实施推荐

---

## 目录

- [第一章：工具轮次与上下文管理](#第一章工具轮次与上下文管理)（完整正文：[context-management-deep-dive.md](./context-management-deep-dive.md)）
- [第二章：CLI/TUI 展示设计](#第二章clitui-展示设计)
- [第三章：Thinking/Reasoning 协议接入](#第三章thinkingreasoning-协议接入)
- [跨章节依赖与实施顺序](#跨章节依赖与实施顺序)
- [风险与 Tradeoff 分析](#风险与-tradeoff-分析)
- [引用源码索引](#引用源码索引)

---

## 第一章：工具轮次与上下文管理

本章正文已迁至独立文档：[工具轮次与上下文管理深度分析](./context-management-deep-dive.md)。涵盖 pi-rust-wasm 现状、reasoning loop 与 session 裁剪、pi-mono/openclaw 策略、数学模型与 `max_safe_rounds`、工具循环检测、上下文语义完整性、实现方案（A–E）多维度打分与分阶段推荐路径。

---

## 第二章：CLI/TUI 展示设计

### 2.1 pi-rust-wasm 现状

- `render.rs`：`MarkdownRenderer` 仅处理 `ContentDelta`（代码块语法高亮 + 流式输出），`ToolCallDelta` 和 tool 结果完全不展示
- `chat.rs` L330：`on_stream_delta` 是 `AgentLoop` 唯一的渲染出口，且只传 assistant 正文
- `EventBus` 注入了 `AgentLoop`，loop 内部会 `emit(ToolExecutionStart/End)` 等事件，但 `chat.rs` 未订阅

### 2.2 当前 vs 目标 CLI 渲染效果

```
  ═══════════════════════════════════════════════════════════════
  当前 pi-rust-wasm：只能看到 assistant 正文
  ═══════════════════════════════════════════════════════════════

  LLM 流式返回
  │
  ├─ ContentDelta("让我来看看...")  ──► on_stream_delta ──► MarkdownRenderer ──► 终端
  │                                          ▲
  ├─ ToolCallDelta(read_file, ...)  ──► (丢弃，无人消费)
  │
  ├─ ThinkingDelta(不存在)          ──► (StreamEvent 无此变体)
  │
  └─ FinishReason("stop")

  execute_tool() 执行完毕          ──► emit(ToolExecutionEnd) ──► EventBus ──► (无订阅者)

  用户终端看到的：
  ┌──────────────────────────────────────────────┐
  │ pi.nibbles> 让我来看看这个文件的内容...         │
  │ 好的，我已经完成了重构。主要改动如下...          │
  │                                              │  ← 中间发生了什么？看不到！
  │ u>                                           │
  └──────────────────────────────────────────────┘


  ═══════════════════════════════════════════════════════════════
  目标效果（像 Cursor / 豆包 那样）
  ═══════════════════════════════════════════════════════════════

  用户终端看到的：
  ┌──────────────────────────────────────────────────────────────┐
  │ pi.nibbles>                                                  │
  │                                                              │
  │ [thinking] 用户想要重构 main.rs，我需要先读取文件内容，       │ ← dim/italic
  │            然后分析结构，再进行编辑...                         │
  │                                                              │
  │ [tool] read_file  path="src/main.rs"                        │ ← 灰色
  │ [tool] read_file  ✓ 读取 238 行 (0.3s)                      │ ← 绿色
  │                                                              │
  │ 我来分析一下这段代码的结构...                                  │ ← assistant 正文
  │                                                              │
  │ [tool] edit_file  path="src/main.rs"                        │ ← 灰色
  │ [tool] edit_file  ✓ 替换 15-42 行 (0.1s)                    │ ← 绿色
  │                                                              │
  │ [tool] execute_bash  command="cargo build"                  │ ← 灰色
  │ [tool] execute_bash  ✗ 编译失败 (2.1s)                      │ ← 红色
  │        error[E0308]: mismatched types...                     │ ← 红色，可选展开
  │                                                              │
  │ 编译出错了，让我修复一下类型不匹配的问题...                     │ ← assistant 正文
  │                                                              │
  │ u>                                                           │
  └──────────────────────────────────────────────────────────────┘
```

### 2.3 pi-mono 交互模式

pi-mono 的 TUI 实现在 `packages/coding-agent/src/modes/interactive/`（不是 `src/tui/`）。

**事件链**：

```
  agent-loop.ts                 agent-session.ts             interactive-mode.ts
  ═══════════════               ════════════════             ═══════════════════

  streamAssistantResponse()
  │
  ├─ thinking_delta ──► emit("message_update") ──► _handleAgentEvent ──► handleEvent
  ├─ text_delta     ──►       同上               ──►       同上        ──►    同上
  ├─ toolcall_delta ──►       同上               ──►       同上        ──►    同上
  │
  └─ tool execution
     ├─ start       ──► emit("tool_execution_start")  ──► handleEvent
     ├─ update      ──► emit("tool_execution_update") ──► handleEvent（bash 增量）
     └─ end         ──► emit("tool_execution_end")    ──► handleEvent
```

**渲染规则**：

| 消息类型 | 渲染方式 |
|---------|---------|
| tool_calls（assistant 中的 toolCall 块） | `ToolExecutionComponent`：内置工具有专用格式（read 显示路径、bash 显示命令+计时、edit 显示 diff 高亮）；pending 用黄色背景，完成用绿/红色背景 |
| tool results | **不独立渲染**，合并到对应 `ToolExecutionComponent` 的 output 区域 |
| thinking | `AssistantMessageComponent` 检测 `type === "thinking"` 块；`hideThinkingBlock=true` 时显示一行 "Thinking..."，否则完整 Markdown 渲染（italic + thinkingText 主题色） |
| 全局状态 | Loader spinner 表示 agent 工作中 |

### 2.4 openclaw TUI

openclaw 与 pi-mono **共享 `@mariozechner/pi-tui` 底层库**（`packages/tui`），但**应用层代码独立实现**。

**事件链**：通过 `GatewayChatClient` 订阅 SSE → `handleAgentEvent` 按 `evt.stream` 分派：

```
  GatewayChatClient                     tui-event-handlers.ts
  ═══════════════════                   ═══════════════════════

  evt.event === "chat"
  │ evt.stream === "delta"  ──► TuiStreamAssembler.ingestDelta() ──► chatLog.updateAssistant()
  │ evt.stream === "final"  ──► finalize                         ──► chatLog.finalizeAssistant()

  evt.event === "agent"
  │ evt.stream === "tool"
  │   phase === "start"     ──► chatLog.startTool(toolCallId, toolName, args)
  │   phase === "update"    ──► chatLog.updateToolResult(toolCallId, partialResult)
  │   phase === "result"    ──► chatLog.updateToolResult(toolCallId, result, {isError})
```

**关键差异与 pi-mono 的对比**：

| 维度 | pi-mono | openclaw |
|------|---------|----------|
| 底层 TUI 库 | `@mariozechner/pi-tui` | 同左（共享） |
| 应用层代码 | `modes/interactive/` | `src/tui/`（独立实现） |
| 工具可见度 | 始终显示 | `verboseLevel` 控制：off / normal / full |
| thinking 显示 | `hideThinkingBlock` 设置 | `showThinking` 开关（Ctrl+T 切换） |
| thinking 渲染 | `AssistantMessageComponent` 内 italic 区 | `TuiStreamAssembler` 拼 `[thinking]\n` 前缀 |
| 工具渲染组件 | `ToolExecutionComponent`（长版，含 diff 高亮等） | `ToolExecutionComponent`（短版，emoji + label + 可折叠） |

### 2.5 方案 A 多回调架构（推荐）

```
  AgentLoop (agent_loop.rs)                        chat.rs（CLI 主循环）
  ═══════════════════════════                       ═══════════════════════

  LLM 流式 SSE 解析
  │
  ├─ ContentDelta ─────────► on_stream_delta ────► MarkdownRenderer.push()
  │                          (已有)                  → print!("{}", chunk)
  │
  ├─ ThinkingDelta ────────► on_thinking_delta ──► print dim/italic
  │  (新增 StreamEvent)       (新增回调)              或 "[thinking...]" 一行
  │
  └─ ToolCallDelta ────────► (内部累积到 tool_calls_buf，不直接回调)


  工具执行阶段
  │
  ├─ 开始执行 tc ──────────► on_tool_start ──────► print "[tool] {name} {args}"
  │                          (新增回调)              灰色
  │
  └─ 执行完毕 ─────────────► on_tool_end ────────► print "[tool] {name} ✓/✗ ({time})"
                             (新增回调)              绿色/红色 + 可选结果摘要


  回调签名（Rust）：
  ┌──────────────────────────────────────────────────────────────────────┐
  │ on_stream_delta:   Box<dyn FnMut(&str) + Send>          // 已有      │
  │ on_thinking_delta: Box<dyn FnMut(&str) + Send>          // 新增      │
  │ on_tool_start:     Box<dyn FnMut(&str, &str) + Send>    // 新增      │
  │                                   ↑name  ↑args_json                 │
  │ on_tool_end:       Box<dyn FnMut(&str, &str, bool, f64) + Send>     │
  │                              ↑name ↑result ↑is_err ↑elapsed_sec     │
  └──────────────────────────────────────────────────────────────────────┘
```

### 2.6 实现方案

#### 方案 A：多回调扩展（推荐，最小侵入）

在 `AgentLoop` 上新增三个可选回调（签名见上图），`chat.rs` 注册回调并用 ANSI 转义码格式化输出。

- 涉及文件：`agent_loop.rs`（新增回调字段和 setter，在工具执行前后、thinking delta 处调用）、`chat.rs`（注册回调、格式化输出）
- 工作量：~2 天
- **推荐度：最高**（改动小、无新依赖、与已有 `on_stream_delta` 模式一致）

#### 方案 B：EventBus 订阅模式（中等改动）

不新增回调，在 `chat.rs` 里对已有 `EventBus` 订阅 `ToolExecutionStart` / `ToolExecutionEnd` / `MessageUpdate` 等事件。

- 优点：解耦、可扩展（未来 Web UI / LSP 也走 EventBus）
- 缺点：`DefaultEventBus` 是同步 `emit_sync`，可能需改为 channel-based；payload 是 `serde_json::Value`，需反序列化
- 工作量：~3 天
- **推荐度：中等**（架构更干净但改动更多、近期不需要多消费者）

#### 方案 C：CliRenderer 统一渲染层（远期目标）

借鉴 pi-mono/openclaw 的组件化设计：新建 `CliRenderer` trait，包含 `AssistantTextRenderer`、`ToolExecutionRenderer`、`ThinkingRenderer` 三个子组件。支持 `--verbose` 级别控制（off / normal / full）。

- 涉及文件：新建 `cli_renderer.rs`、改造 `render.rs`、`chat.rs`、CLI 参数新增 `--verbose`
- 工作量：~5-7 天
- **推荐度：远期目标**

#### 方案 D：thinking 折叠/展开控制（补充性方案）

借鉴 pi-mono 的 `hideThinkingBlock` / openclaw 的 `showThinking`：在配置或 CLI 参数中增加 `--show-thinking`（默认 on）。thinking 开启时流式输出（dim/italic），关闭时只显示一行 `[thinking...]`。

- 可与方案 A/B/C 中任一组合
- 工作量：~0.5 天（依赖已有 thinking 回调基础）
- **推荐度：高**（用户体验显著提升，实现极简单）

### 2.7 推荐路径

**近期必做**：A + D（多回调 + thinking 折叠控制），快速实现核心可见性

**中期**：当需要多消费者或 Web UI 时重构为 B

**远期**：C 作为终极形态

---

## 第三章：Thinking/Reasoning 协议接入

### 3.1 关键概念澄清：Thinking 到底是怎么实现的？

**不是靠 prompt 让 LLM 输出思考过程，而是 LLM API 本身有专门的参数和返回格式。** 但各家实现方式不同。

```
  ═══════════════════════════════════════════════════════════════
  误解：通过 prompt 要求 LLM 输出思考（这不是 thinking 协议）
  ═══════════════════════════════════════════════════════════════

  请求：
  {
    "messages": [
      {"role": "user", "content": "请先输出你的思考过程，再给出答案"}  ← 只是普通 prompt
    ]
  }

  返回：
  {
    "choices": [{
      "message": {
        "role": "assistant",
        "content": "我的思考：...\n\n最终答案：..."   ← 全混在 content 里，不可靠
      }
    }]
  }

  问题：思考内容和正文混在一起，无法程序化区分；模型可能不遵守指令


  ═══════════════════════════════════════════════════════════════
  真正的 Thinking 协议：API 参数 + 结构化返回（各家不同）
  ═══════════════════════════════════════════════════════════════

  ┌────────────────────────────────────────────────────────────────────┐
  │ 类型 A：OpenAI / DeepSeek 风格（Chat Completions 扩展字段）         │
  │                                                                    │
  │ 请求体多一个参数：                                                   │
  │ {                                                                  │
  │   "model": "deepseek-reasoner",                                   │
  │   "messages": [...],                                               │
  │   "reasoning_effort": "high"     ← API 级别参数，不是 prompt        │
  │ }                                                                  │
  │                                                                    │
  │ 流式返回的 SSE chunk 里多一个字段：                                   │
  │ {                                                                  │
  │   "choices": [{                                                    │
  │     "delta": {                                                     │
  │       "reasoning_content": "让我分析一下这个问题...",  ← 思考内容    │
  │       "content": null                                 ← 正文为空    │
  │     }                                                              │
  │   }]                                                               │
  │ }                                                                  │
  │     ... 多个 chunk 后 reasoning_content 结束 ...                    │
  │ {                                                                  │
  │   "choices": [{                                                    │
  │     "delta": {                                                     │
  │       "reasoning_content": null,                      ← 思考结束    │
  │       "content": "根据分析，答案是..."                 ← 正文开始    │
  │     }                                                              │
  │   }]                                                               │
  │ }                                                                  │
  │                                                                    │
  │ 适用：OpenAI o 系列、DeepSeek R1、部分 OpenAI 兼容网关              │
  └────────────────────────────────────────────────────────────────────┘

  ┌────────────────────────────────────────────────────────────────────┐
  │ 类型 B：豆包 / Moonshot 风格（请求体 thinking 对象）                 │
  │                                                                    │
  │ 请求体：                                                            │
  │ {                                                                  │
  │   "model": "doubao-seed-2.0-pro",                                 │
  │   "messages": [...],                                               │
  │   "thinking": {                    ← 专门的 thinking 配置对象       │
  │     "type": "enabled"              ← enabled / disabled / auto     │
  │   }                                                                │
  │ }                                                                  │
  │                                                                    │
  │ 流式返回里的 reasoning 内容：                                        │
  │ {                                                                  │
  │   "choices": [{                                                    │
  │     "delta": {                                                     │
  │       "content": "",                                               │
  │       "reasoning_content": "思考过程..."   ← 与类型 A 格式类似       │
  │     }                                                              │
  │   }]                                                               │
  │ }                                                                  │
  │                                                                    │
  │ 适用：豆包 Doubao、Moonshot/Kimi                                    │
  └────────────────────────────────────────────────────────────────────┘

  ┌────────────────────────────────────────────────────────────────────┐
  │ 类型 C：Anthropic 风格（完全不同的事件协议）                         │
  │                                                                    │
  │ 请求体：                                                            │
  │ {                                                                  │
  │   "model": "claude-sonnet-4-...",                                  │
  │   "messages": [...],                                               │
  │   "thinking": {                                                    │
  │     "type": "enabled",                                             │
  │     "budget_tokens": 10000         ← 思考 token 预算               │
  │   }                                                                │
  │ }                                                                  │
  │                                                                    │
  │ 流式返回用完全不同的 SSE 事件类型：                                   │
  │   event: content_block_start                                       │
  │   data: {"type": "thinking", "thinking": ""}                       │
  │                                                                    │
  │   event: content_block_delta                                       │
  │   data: {"type": "thinking_delta", "thinking": "分析这个..."}       │
  │                                                                    │
  │   event: content_block_stop        ← thinking 结束                  │
  │                                                                    │
  │   event: content_block_start                                       │
  │   data: {"type": "text", "text": ""}                               │
  │                                                                    │
  │   event: content_block_delta                                       │
  │   data: {"type": "text_delta", "text": "答案是..."}                │
  │                                                                    │
  │ 特殊：返回 signature 字段，多轮对话需回传（加密的思考摘要）          │
  │ 适用：仅 Anthropic Claude                                          │
  └────────────────────────────────────────────────────────────────────┘
```

**总结**：三种类型的共同点是 **请求体里有专门的 API 参数**（不是 prompt），**响应里有结构化分离的思考内容**（不是混在 content 里）。区别在于参数名、响应字段名、流式协议格式各家不同。

### 3.2 "pi-ai" 是什么

指 pi-mono 里的 `packages/ai`（npm 包名 `@mariozechner/pi-ai`），是统一 LLM provider 抽象层。它**不是** pi-rust-wasm 的组件。

### 3.3 pi-rust-wasm 现状

```
  当前 pi-rust-wasm（openai.rs）
  ═════════════════════════════

  请求体 OpenAiRequestBody：
  ┌──────────────────────────┐       POST /v1/chat/completions
  │ model: "gpt-4"           │──────────────────────────────────►  LLM API
  │ messages: [...]           │
  │ temperature: 0.7          │       没有 reasoning_effort
  │ max_completion_tokens: .. │       没有 thinking 对象
  │ stream: true              │
  │ tools: [...]              │
  └──────────────────────────┘

  流式解析 OpenAiStreamDelta：
  ┌──────────────────────────┐
  │ content: Option<String>  │──► ContentDelta    ──► on_stream_delta ──► 终端
  │ tool_calls: Option<Vec>  │──► ToolCallDelta   ──► (内部累积)
  │                          │
  │ (无 reasoning_content)   │    (无 ThinkingDelta)   (无 on_thinking_delta)
  └──────────────────────────┘
```

- 固定 `/v1/chat/completions`（`openai.rs` L199）
- `OpenAiRequestBody` 无 `thinking` / `reasoning_effort` 字段
- `OpenAiStreamDelta` 无 `reasoning_content` / `reasoning` 字段
- `StreamEvent` 枚举无 thinking 相关变体

### 3.4 业界协议汇总

| 厂商/协议 | API 端点 | 请求参数 | 流式响应字段 | 备注 |
|-----------|---------|---------|------------|------|
| OpenAI Chat Completions | `/v1/chat/completions` | `reasoning_effort` | `delta.reasoning_content` | reasoning 跨 turn 不保留 |
| OpenAI Responses | `/v1/responses` | `reasoning.effort` | `response.reasoning_summary_text.delta` | 新 API，reasoning 跨 turn 保留，SWE-bench +3% |
| Anthropic Messages | `/v1/messages` | `thinking: {type, budget_tokens}` | `thinking_delta` 事件 + `signature` | 独有 signature 机制，多轮需回传 |
| DeepSeek R1 | `/v1/chat/completions`（兼容） | 无特殊参数（模型自带） | `delta.reasoning_content` | 多轮需移除 `reasoning_content` |
| 豆包 Doubao | `/v1/chat/completions`（兼容） | `thinking: {type: enabled/disabled/auto}` | `delta.reasoning_content` | Seed 2.0 系列支持 |
| Moonshot/Kimi | `/v1/chat/completions`（兼容） | `thinking: {type: enabled}` | `delta.reasoning_content` | 与豆包格式类似 |

### 3.5 pi-mono 的做法

pi-ai 的设计核心是**统一抽象 + 按厂商分派**：

**请求侧**（`openai-completions.ts` `buildParams`）：

```
  ThinkingLevel ("minimal" | "low" | "medium" | "high" | "xhigh")
      │
      │  mapReasoningEffort(effort, reasoningEffortMap)
      ▼
  ┌─ thinkingFormat 分派 ─────────────────────────────────────────────┐
  │                                                                    │
  │  "openai"（默认）  → reasoning_effort = "high"                      │
  │  "openrouter"      → reasoning: { effort: "high" }                 │
  │  "zai"             → enable_thinking: true                         │
  │  "qwen"            → enable_thinking: true                         │
  │  "qwen-chat-template" → chat_template_kwargs: {enable_thinking: true} │
  │                                                                    │
  └────────────────────────────────────────────────────────────────────┘
```

**响应侧**（流式解析）：

- `delta.reasoning_content` **或** `delta.reasoning` **或** `delta.reasoning_text` → 三路检测，统一映射为 `thinking_start` / `thinking_delta` / `thinking_end` 事件
- Responses API：独立 provider（`openai-responses.ts`），走 `client.responses.create`，从 `response.output_item.added` 等事件中提取 reasoning

### 3.6 openclaw 的做法

- 复用 `@mariozechner/pi-ai` 的 `streamSimple`，不重新实现 provider
- 特殊厂商（Moonshot/Kimi）用 `createMoonshotThinkingWrapper` 包装 payload 中的 `thinking` 字段
- 能力位控制：`preserveAnthropicThinkingSignatures`（保留 Claude 的 signature）、`dropThinkingBlockModelHints`（Vertex/Bedrock 丢弃 thinking 块）

### 3.7 pi-rust-wasm 改造方案

```
  方案 A 改造后：
  ═════════════════════════════

  请求体 OpenAiRequestBody（新增字段）：
  ┌──────────────────────────────┐   POST /v1/chat/completions
  │ model: "deepseek-reasoner"   │──────────────────────────────────► LLM API
  │ messages: [...]              │
  │ stream: true                 │
  │ tools: [...]                 │
  │ reasoning_effort: "high"     │ ← 新增，可选，OpenAI/DeepSeek 用
  │ thinking: {type: "enabled"}  │ ← 新增，可选，豆包/Moonshot 用
  └──────────────────────────────┘   （两个字段按 provider 配置二选一）

  流式解析 OpenAiStreamDelta（新增字段）：
  ┌──────────────────────────────────┐
  │ content: Option<String>          │──► ContentDelta     ──► on_stream_delta
  │ tool_calls: Option<Vec>          │──► ToolCallDelta    ──► (内部累积)
  │ reasoning_content: Option<String>│──► ThinkingDelta    ──► on_thinking_delta ──► 终端
  │         ↑ 新增                   │        ↑ 新增 StreamEvent     ↑ 新增回调
  └──────────────────────────────────┘

  同一个 /v1/chat/completions 端点，只是请求多了参数、响应多了字段
  不需要换 API 端点！
```

#### 方案 A：最小改动，Chat Completions + reasoning_content 字段扩展（推荐）

具体改动点：

1. `types.rs` — `OpenAiStreamDelta` 增加 `reasoning_content: Option<String>`
2. `types.rs` — `StreamEvent` 增加 `ThinkingDelta { delta: String }` 变体
3. `openai.rs` — `OpenAiRequestBody` 增加可选 `reasoning_effort: Option<String>` 和 `thinking: Option<serde_json::Value>`
4. `openai.rs` — `openai_chunk_to_stream_events` 解析 `reasoning_content`，产出 `ThinkingDelta`
5. `config.rs` — `LlmConfig` 新增 `thinking_format: Option<String>` 配置（决定发哪个参数）
6. `agent_loop.rs` — reasoning loop 中 `ThinkingDelta` 分支调用 `on_thinking_delta` 回调

- 适用：OpenAI、DeepSeek、豆包等 OpenAI 兼容 API（覆盖 80% 场景）
- 工作量：~2 天
- **推荐度：最高**

#### 方案 B：双端点支持（Chat Completions + Responses API）

- 新增 `OpenAiResponsesProvider` 或在现有 provider 里按配置切端点
- 走 `/v1/responses`，`reasoning` 跨 turn 保留，SWE-bench +3%
- 适用：纯 OpenAI 场景
- 工作量：~5 天
- **推荐度：中等**（收益高但工作量大，且非 OpenAI 厂商不支持）

#### 方案 C：Provider 抽象层（类 pi-ai 的 thinkingFormat 分派）

- 为每种 `thinkingFormat` 实现请求参数映射 + 流式解析映射
- 最灵活，但 Rust 实现工作量大
- 工作量：~7-10 天
- **推荐度：远期目标**

### 3.8 Thinking 内容的持久化问题

当前 `AgentMessage` 枚举只有 `User / Assistant / ToolResult / System / Steering / CompactionSummary`，没有 thinking 相关表达。需要决定持久化策略：

| 方案 | 做法 | 优劣 |
|------|------|------|
| 合并到 Assistant.text | thinking + 正文拼接存储 | 简单但丢失结构信息，回放时无法分区展示 |
| Assistant 加 `thinking_text: Option<String>` | 在 `AgentMessage::Assistant` 中新增字段 | 推荐，保留结构，序列化到 transcript 时带 thinking |
| 新增 `AgentMessage::Thinking` 变体 | 独立消息类型 | 与 OpenAI 的消息模型不对齐，`convert_to_llm_format` 处理复杂 |

**推荐**：在 `AgentMessage::Assistant` 中增加 `thinking_text: Option<String>` 字段；`convert_to_llm_format` 时按厂商策略决定是否在发给 LLM 的消息中保留（DeepSeek 要求多轮时移除）。

### 3.9 推荐路径

**近期必做**：方案 A（Chat Completions + reasoning_content 字段扩展）

**中期**：方案 B（Responses API，当 OpenAI 推荐迁移时）

**远期**：方案 C（多厂商 provider 抽象）

---

## 跨章节依赖与实施顺序

三章方案之间存在依赖关系：

```
第三章 方案A（协议层：StreamEvent 新增 ThinkingDelta）
    │
    ├──► 第二章 方案A（展示层：on_thinking_delta 回调依赖 ThinkingDelta 事件）
    │
    └──► 第一章 方案A（上下文层：token 估算需知道 thinking 内容是否计入 context）
         │
         └──► 第一章 方案B（tool result 截断，独立于 thinking，可先行）
```

**推荐实施顺序**：

| 步骤 | 内容 | 依赖 | 预估工时 |
|------|------|------|---------|
| 1 | 第一章 方案B：tool result 截断 | 无 | 1 天 |
| 2 | 第三章 方案A：Chat Completions + reasoning_content | 无 | 2 天 |
| 3 | 第二章 方案A+D：多回调 + thinking 折叠 | 依赖步骤 2 | 2.5 天 |
| 4 | 第一章 方案A：token 估算 + 动态阈值 | 可与 2/3 并行 | 2 天 |
| **合计** | | | **~7.5 天** |

---

## 风险与 Tradeoff 分析

### Token 估算精度

`chars / 4` 启发式在英文场景基本准确，但**中文场景偏差大**：中文 1 字符 ≈ 1-2 token（而非 0.25）。

| 方案 | 精度 | 复杂度 |
|------|------|--------|
| `chars / 4` | 中文偏低估 2-4 倍 | 零依赖 |
| 中文系数：`chinese_chars * 1.5 + ascii_chars / 4` | 中等 | 需判断字符类型 |
| `tiktoken-rs` crate | 精确 | 新增依赖，初始化开销 |

**建议**：先用 `chars / 4` 起步（pi-mono 也是这样），保守设置 `reserve_tokens`（如 20000 而非 16384）来补偿；后续按需引入 tiktoken-rs。

### Thinking 内容的上下文占用

各厂商对 thinking 内容在多轮对话中的处理不同：

| 厂商 | 多轮处理要求 |
|------|------------|
| DeepSeek | **必须移除** `reasoning_content`，否则返回 400 错误 |
| OpenAI Chat Completions | reasoning 跨 turn 不保留（API 自动丢弃） |
| OpenAI Responses | reasoning 跨 turn 自动保留（服务端状态） |
| 豆包 / Moonshot | 文档未明确要求移除 |
| Anthropic | 需回传 `signature`，thinking 文本可省略 |

**对 pi-rust-wasm 的影响**：`convert_to_llm_format` 需要按配置决定是否在 `Assistant` 消息中携带 `thinking_text`。建议增加 `strip_thinking_on_resend: bool` 配置（默认 true）。

### 流式渲染的终端兼容性

| 特性 | iTerm2 / macOS Terminal | Windows Terminal | 基础 xterm |
|------|------------------------|-----------------|-----------|
| ANSI 颜色（`\x1b[32m`） | 支持 | 支持 | 支持 |
| italic（`\x1b[3m`） | 支持 | 支持 | 部分不支持 |
| dim（`\x1b[2m`） | 支持 | 支持 | 支持 |

**建议**：thinking 优先用 dim（`\x1b[2m`）而非 italic，兼容性更好。

### `max_tool_rounds` 保留还是废弃

方案 A 的 token 预警是更好的安全网，但 `max_tool_rounds` 作为**硬上限**仍有防失控价值（token 估算可能不准，网络错误可能导致 usage 缺失）。

**建议**：保留 `max_tool_rounds` 但提高到 **20-30**，作为最后一道防线。

---

## 引用源码索引

### pi-rust-wasm

| 文件 | 关键内容 |
|------|---------|
| `src/core/agent_loop.rs` | 三层循环、`StreamEvent` 消费、工具执行、`AgentLoopConfig` |
| `src/core/llm/types.rs` | `ChatRequest`、`StreamEvent`、`ChatMessage` 定义 |
| `src/core/llm/openai.rs` | `OpenAiRequestBody`、`OpenAiStreamDelta`、SSE 解析 |
| `src/api/chat.rs` | CLI 主循环、`on_stream_delta` 注册、`build_context_messages` 调用 |
| `src/api/render.rs` | `MarkdownRenderer`（代码高亮 + 流式输出） |
| `src/core/session/manager.rs` | `build_context_messages`、`context_cap`、`DEFAULT_CONTEXT_CAP` |
| `src/infra/config.rs` | `LlmConfig`、`AgentConfig`、`PrimitiveConfig` |

### pi-mono

| 文件 | 关键内容 |
|------|---------|
| `packages/agent/src/agent-loop.ts` | agent 主循环、`streamAssistantResponse`、`executeToolCalls` |
| `packages/ai/src/types.ts` | `ThinkingLevel`、`ThinkingBudgets`、`AssistantMessageEvent`、`thinkingFormat` |
| `packages/ai/src/providers/openai-completions.ts` | `buildParams`（thinkingFormat 分派）、`mapReasoningEffort`、流式解析 |
| `packages/ai/src/providers/openai-responses.ts` | Responses API provider |
| `packages/coding-agent/src/core/compaction/compaction.ts` | `shouldCompact`、`findCutPoint`、`generateSummary` |
| `packages/coding-agent/src/core/session-manager.ts` | `buildSessionContext`、compaction 路径 |
| `packages/coding-agent/src/modes/interactive/components/tool-execution.ts` | `ToolExecutionComponent` 渲染 |
| `packages/coding-agent/src/modes/interactive/components/assistant-message.ts` | thinking 块渲染 |

### openclaw

| 文件 | 关键内容 |
|------|---------|
| `src/tui/tui-event-handlers.ts` | tool 事件分发（`stream === "tool"`）、thinking 流式拼装 |
| `src/tui/tui-formatters.ts` | `composeThinkingAndContent`、`extractThinkingFromMessage` |
| `src/tui/components/tool-execution.ts` | `ToolExecutionComponent`（渲染工具执行） |
| `src/tui/components/assistant-message.ts` | `AssistantMessageComponent`（渲染 assistant） |
| `src/agents/pi-embedded-runner/run/attempt.ts` | `installToolResultContextGuard`、`limitHistoryTurns` |
| `src/agents/pi-embedded-runner/run.ts` | overflow recovery、`MAX_OVERFLOW_COMPACTION_ATTEMPTS` |
| `src/agents/tool-loop-detection.ts` | 重复模式检测 |
| `src/agents/pi-embedded-runner/moonshot-stream-wrappers.ts` | Moonshot/Kimi thinking 包装 |
