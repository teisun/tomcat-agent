# 多 Agent 项目对接 OpenAI 及多 LLM 抽象方式对比

**文档性质**：基于 Tomcat 工作区内仓库的代码检索与模块阅读整理的技术报告，供 `tomcat` 选型与对标参考。  
**检索日期**：以仓库当前快照为准。**说明**：Cursor 工作区索引有时不完整；**是否含某目录请以 Tomcat 根下 `ls` 为准**（例如 **`openclaw/`**、**`pi-mono/`** 与本文报告路径一致时即可稽核源码）。

---

## 1. 范围与材料来源

| 仓库 | 本工作区可用性 | 说明 |
|------|----------------|------|
| **pi_agent_rust** | 完整 | `src/provider.rs`、`src/providers/*`、`src/models.rs` 等可稽核 |
| **hermes-agent** | 完整 | `agent/transports/*`、`agent/auxiliary_client.py` 等可稽核 |
| **tomcat** | 完整 | `src/core/llm/*` 当前唯一 OpenAI 实现为 Chat Completions |
| **pi-mono** | **完整（磁盘）** | 目录 **`Tomcat/pi-mono/`**（独立仓库快照）：含 **`packages/ai`**（如 **`src/providers/openai-completions.ts`**）、**`packages/coding-agent`** 等——详见 §6。另：**`pi_agent_rust/legacy_pi_mono_code/`** 内为 **crates 发布用 stub**，与磁盘上完整 **`pi-mono`** 不是同一套用途 |
| **openclaw** | **完整（磁盘）** | 目录 **`Tomcat/openclaw/`**（TypeScript）；核心见 **`src/agents/openai-transport-stream.ts`**、网关 **`src/gateway/openai-http.ts`**（详见 §5） |

---

## 2. OpenAI 官方接口在本话题下的划分

为对齐下文「接了哪条 HTTP 路径」，先区分三条常见主线：

| 接口 | 典型路径 | 用途（简述） |
|------|-----------|--------------|
| **Chat Completions** | `POST /v1/chat/completions` | 传统对话：`messages` + 可选 `tools`；多模态常用 **`content` 数组**（`text` + `image_url`） |
| **Responses** | `POST /v1/responses` | OpenAI 新一代统一接口：`input` 条目、`input_text` / `input_image` 等；文档侧常与 **PDF `input_file`** 等能力一并描述 |
| **其它** | Embeddings、Realtime、Files 等 | 本报告聚焦「主对话循环」，不展开 |

### 2.1 ASCII：两条主对话 HTTP 路径（心智模型）

```text
                         同一 OpenAI 账号 / Key（示意）
                                    │
        ┌───────────────────────────┼───────────────────────────┐
        │                           │                           │
        ▼                           ▼                           │
 POST /v1/chat/completions          POST /v1/responses          │  其它 /v1/…
        │                           │                           │
  body: model                       body: model                 │
       messages[] ←───────────────► input[] (+ instructions)     │
       tools                        tools（映射策略不同）          │
        │                           │                           │
        └─────────────┬─────────────┘                           │
                      ▼                                         │
               多模态差异（摘要）
               • Completions：content 数组里塞 text / image_url
               • Responses：input_text / input_image（文档还含 input_file=PDF）
                      │
                      └─────────────────────────────────────────┘
```

---

## 3. pi_agent_rust：接口覆盖面最广，trait + 工厂路由

### 3.1 核心抽象

- **`Provider` trait**（`src/provider.rs`）：所有后端实现同一套 **`stream(context, options) -> Stream<StreamEvent>`**，输入为统一的 **`Context`**（system prompt、对话 **`Message`** 列表、**`ToolDef`** 列表）。
- **统一内部模型**：`crate::model::Message` / `UserContent` / `ContentBlock` 等，由各 Provider **编码为对应厂商 JSON**。

### 3.2 与 OpenAI 相关的两条硬路由

实现类型与 **HTTP 入口**（默认值，可被 `base_url` 覆盖）：

| `api` 标识（ModelEntry / provider.api()） | 实现模块 | 默认 URL |
|------------------------------------------|----------|----------|
| **`openai-completions`** | `providers/openai.rs` → `OpenAIProvider` | `https://api.openai.com/v1/chat/completions` |
| **`openai-responses`** | `providers/openai_responses.rs` → `OpenAIResponsesProvider` | `https://api.openai.com/v1/responses` |

另有 **`openai-codex-responses`**（Codex / ChatGPT OAuth 端点变体）、**Azure**（`azure-openai` / `azure-openai-responses`）、以及与 OpenAI **无关**的 Anthropic、Gemini、Vertex、Cohere、Bedrock、GitLab 等，均由同一 **`create_provider`** 工厂按 **`ModelEntry` + `resolve_provider_route`** 分发。

### 3.3 工厂入口

- **`create_provider(entry, extensions)`**（`src/providers/mod.rs`）：  
  - 若扩展注册了 **`streamSimple`** 类 Provider，优先走扩展运行时；  
  - 否则根据 **`ProviderRouteKind`** 构造 **`Arc<dyn Provider>`**（Anthropic / OpenAI Completions / OpenAI Responses / Azure / …）。

### 3.3.1 ASCII：`create_provider` 分发（示意）

```text
                    ModelEntry
            (provider · api · base_url · model.id · compat…)
                              │
                              ▼
               ┌──────────────────────────────┐
               │  extension streamSimple ?    │
               └──────────────┬───────────────┘
                      yes │          │ no
                          ▼          ▼
               ExtensionStream…    resolve_provider_route(entry)
               SimpleProvider              │
                          │                ▼
                          │     ┌─────────────────────┐
                          │     │ ProviderRouteKind   │
                          │     └──────────┬──────────┘
                          │                │
          ┌───────────────┼────────────────┼───────────────┐
          ▼               ▼                ▼               ▼
   AnthropicProvider   OpenAIProvider   OpenAIResponses…   Azure…
   (Messages API)      (…/chat/         (…/responses)    …
                        completions)
          │               │                │
          └───────────────┴────────────────┴──── Arc<dyn Provider>
                              │
                              ▼
                    .stream(Context, StreamOptions)
                              │
                              ▼
                      HTTP POST → SSE / JSON
                              │
                              ▼
                      Stream<StreamEvent>
```

### 3.4 多模态与协议映射（OpenAI 侧）

- **Chat Completions**：请求体中的 **`OpenAIContent`** 支持 **`text` + `image_url`**（见 `openai.rs` 内 `OpenAIContentPart`），与官方 vision 用法对齐。
- **Responses**：将用户消息映射为 **`input_text` / `input_image`**（`openai_responses.rs` 内 `OpenAIResponsesUserContentPart`）。  
  **说明**：在当前检索范围内，**未见完整的 `input_file`（PDF）分支** 与内部 `UserContent` 的系统性映射；若需 PDF，需在统一模型层与 Provider 层扩展。

### 3.5 小结

**pi_agent_rust** 是典型的 **「统一 trait + 按模型元数据路由到具体 HTTP API」**：同一 Agent 循环代码，换 **`ModelEntry.api` / base_url** 即可切换 **Completions vs Responses** 及非 OpenAI 厂商。

---

## 4. hermes-agent：Transport 抽象 + `api_mode`，外表仍是 OpenAI SDK 形状

### 4.1 核心抽象

- **`ProviderTransport` ABC**（`agent/transports/base.py`）：每个 **`api_mode`** 一套实现，负责  
  **`convert_messages` → `convert_tools` → `build_kwargs` → `normalize_response`**。  
- **内部统一**：尽量以 **OpenAI 格式的 `messages` / `tools`** 作为「中间表示」，再转成各厂商原生请求。

### 4.2 本仓库内已存在的 transport 模块

目录 **`agent/transports/`** 包含：

| 模块 | `api_mode`（概念） | 职责摘要 |
|------|-------------------|----------|
| **`chat_completions.py`** | `chat_completions` | **默认**：兼容 OpenAI Chat Completions 的 ~16 类网关（OpenRouter、Ollama、DeepSeek、xAI 等）；消息几乎 **identity**，差异在 **`build_kwargs`**（temperature、reasoning、`extra_body` 等） |
| **`anthropic.py`** | `anthropic_messages` | 委托 **`anthropic_adapter`**：OpenAI messages → Anthropic `(system, messages)` |
| **`codex.py`** | `codex_responses` | **Responses API（Codex/Grok/GitHub 等变体）**：委托 **`codex_responses_adapter`**，把 chat messages 转为 Responses **`input`** |

### 4.2.1 ASCII：Transport 管道（逻辑分层）

```text
   ┌─────────────────────────────────────────────────────────────┐
   │  Agent / AIAgent：手里尽量拿「OpenAI 形状」的 messages[] tools[] │
   └─────────────────────────────┬───────────────────────────────┘
                                 │
                                 ▼
                    api_mode 决定选哪条 Transport
                                 │
         ┌───────────────────────┼───────────────────────┐
         ▼                       ▼                       ▼
  ChatCompletionsTransport   AnthropicTransport    ResponsesApiTransport
  (chat_completions)         (anthropic_messages)  (codex_responses)
         │                       │                       │
         │  convert_*            │  → anthropic_adapter  │  → codex_responses_adapter
         │  近乎直通              │  → Messages API      │  → Responses input
         │                       │                       │
         └───────────────────────┴───────────────────────┘
                                 │
                                 ▼
                         build_kwargs(…)  ← 各厂商温度、reasoning、extra_body
                                 │
                                 ▼
                    SDK: chat.completions.create / messages.create / responses.create
                    （具体客户端由上层装配，Transport 不管连接池）
                                 │
                                 ▼
                    normalize_response(raw) → NormalizedResponse
                    （工具调用、usage、文本统一抽一层）
```

### 4.3 与 OpenAI SDK 的关系

- **`auxiliary_client.py`** 等处强调：辅助任务统一暴露 **`client.chat.completions.create(**kwargs)`** 形态；在 Codex 等场景下，底层可把 **Chat Completions 形状适配到 Responses**（注释中明确存在 **chat.completions → Responses** 的翻译层）。
- 主 Agent（`AIAgent`）侧根据 **provider + `api_mode`** 选用对应 **Transport**，再交给具体 SDK / HTTP 客户端。

### 4.4 小结

**hermes-agent** 的多 LLM 抽象是 **「Transport 插件 + 归一化响应 `NormalizedResponse`」**：新增厂商往往新增 **api_mode 分支或扩展 `build_kwargs`**，而不是改业务 Agent 核心逻辑。

---

## 5. openclaw：TypeScript + 官方 `openai` SDK，Completions 与 Responses 双轨

源码根路径：**`openclaw/`**（与 Tomcat 仓库并列）。

### 5.1 与 OpenAI 相关的两条调用链（HTTP 语义）

实现集中在 **`src/agents/openai-transport-stream.ts`**（依赖 **`openai`** npm 包及 **`@mariozechner/pi-ai`** 的模型上下文类型）：

| 入口工厂（导出函数） | SDK 调用 | 典型用途 |
|---------------------|----------|----------|
| **`createOpenAICompletionsTransportStreamFn`** | **`client.chat.completions.create(...)`** | 经典 Chat Completions + 流式 chunk 处理 |
| **`createOpenAIResponsesTransportStreamFn`** | **`client.responses.create(...)`** | OpenAI **Responses API**（与 pi_agent_rust 的 `openai-responses`、hermes 的 `codex_responses` 同属一类「Responses 系」） |
| （另有 **`createAzureOpenAIResponsesTransportStreamFn`** 等） | 同上，Azure `baseURL`/部署 | Azure OpenAI 变体 |

消息侧：文件头部可见 **`convertMessages`** 来自 **`@mariozechner/pi-ai/openai-completions`**，与 **`openai/resources/responses`** 类型一并用于拼装请求。

### 5.2 网关侧：OpenAI 兼容 HTTP + 图片

**`src/gateway/openai-http.ts`** 实现 **OpenAI Chat Completions 兼容的 HTTP 入口**：解析 **`messages`**，**`resolveImagesForRequest`** 从请求中提取 **`image_url`** 等内容，再 **`buildAgentCommandInput`**（含 **`images`**）交给内部 Agent 管线——即 **在网关层就把「类 OpenAI 多模态聊天请求」转成自家 `command` 形状**，而非仅转发裸 JSON。

### 5.3 小结

**openclaw** 与 **pi_agent_rust** 类似，对 OpenAI **同时具备 Completions 与 Responses** 两条流式封装；语言栈为 **TypeScript**，并与 **pi-ai / pi-agent-core** 的 **`StreamFn`、`Model`、`Context`** 集成。PDF 是否走 Responses `input_file` 需结合具体 `buildOpenAIResponsesParams` 与模型策略单独追踪，本报告不展开。

### 5.4 ASCII：openclaw 双轨（与 §2.1 对照）

```text
  model / gateway 配置
           │
           ├────────────────────────────┬────────────────────────────┐
           ▼                            ▼                            │
 createOpenAICompletions…          createOpenAIResponses…            │  Azure 等变体
           │                            │
           ▼                            ▼
   client.chat.completions.create   client.responses.create
           │                            │
           └────────────┬───────────────┘
                        ▼
              统一助理事件流 · usage · tool 解析（同文件内 process*Stream）
```

---

## 6. pi-mono（Tomcat/pi-mono）：TypeScript monorepo，`packages/ai` 管多 Provider

在磁盘 **`Tomcat/pi-mono/`** 下列出了完整 **`packages/`**（如 **`agent`**、**`ai`**、**`coding-agent`**、**`tui`** 等），**并非仅占位目录**。

### 6.1 与 OpenAI / 多 LLM 相关的入口（稽核线索）

- **`packages/ai`**：Provider 实现所在层；例如存在 **`src/providers/openai-completions.ts`**（名称即对应 **Chat Completions** 语义）；测试目录中含 **`openai-responses-*`**、**`anthropic-*`** 等用例，说明 **`packages/ai`** 承担 **多厂商协议适配**（本报告不定论全表，仅提供检索锚点）。
- **`packages/coding-agent`**：上层 coding agent 编排，依赖 **`packages/ai`** 的类型与客户端。

### 6.2 与 `pi_agent_rust` 内 legacy stub 的区别

**`pi_agent_rust/legacy_pi_mono_code/`** 下的 **`models.generated.ts`** 可为 **空 stub**（供 Rust 侧 crates / 解析管线占位），**不代表**磁盘上 **`Tomcat/pi-mono`** 无实现——对比应以 **`Tomcat/pi-mono/packages/`** 为准。

---

## 7. tomcat（本项目）：薄 trait + 单一 OpenAI Chat Completions

### 7.1 抽象

- **`LlmProvider` trait**（`src/core/llm/provider.rs`）：**`chat` / `chat_stream` / `count_tokens`**。
- **`ChatMessage` / `ChatRequest`**（`src/core/llm/types.rs`）：与 OpenAI **messages** 对齐；`ChatMessageContent` 支持 **Text 或 Parts**，但 **`ChatMessageContentPart` 当前仅有 `type` + `text`**，**尚无 `image_url` 等字段**。

### 7.2 OpenAI 实现

- **`src/core/llm/openai.rs`**：固定 **`POST {base}/v1/chat/completions`**，JSON body 为 **`OpenAiRequestBody`**（model、messages、temperature、max_tokens、tools、stream）。
- **结论**：当前主线是 **单一 Chat Completions**；若要 **原生 PDF（Responses `input_file`）** 或 **完整 vision parts**，需 **扩展请求类型与 HTTP 路径（或新增 Responses 客户端）**。

### 7.3 ASCII：tomcat 当前调用栈（单路径）

```text
  chat UI / API 入口
         │
         ▼
  build_context · AgentLoop · tool_exec …
         │
         ▼
  ChatRequest { model, messages: ChatMessage[], tools?, stream }
         │
         ▼
  ┌──────────────────────┐
  │ trait LlmProvider    │
  │  · chat()            │
  │  · chat_stream()     │
  │  · count_tokens()    │
  └──────────┬───────────┘
             │ 当前主要实现
             ▼
  ┌──────────────────────┐
  │ OpenAiProvider       │
  │ (core/llm/openai.rs) │
  └──────────┬───────────┘
             │
             ▼
    POST {base_url}/v1/chat/completions
             │
             ├─ stream: true  ──► SSE 解析 ──► StreamEvent::ContentDelta …
             └─ stream: false ──► JSON choices[0].message …

  ChatMessageContent 虽有 Parts，但 Part 仅 text → vision/PDF 尚未接到 HTTP
```

### 7.4 ASCII：tomcat 全链路（入口 → Agent → LLM HTTP）

与 §5.4（openclaw 双轨）对照：**本仓库 LLM 出口当前只有一条**——**Chat Completions**；下图覆盖「一轮对话里谁在组装 `messages`、谁在打 HTTP」。

```text
  ┌─────────────────────────────────────────────────────────────────────────┐
  │ 入口层（示例路径，依构建目标略有差异）                                     │
  │  • `api/chat/mod.rs`：会话 UI / CLI / WASM 宿主灌入 user 文本             │
  │  • `ext/dispatcher`：`do_chat` / `do_chat_stream`（插件侧统一调 LLM）      │
  └────────────────────────────────┬────────────────────────────────────────┘
                                   │
                                   ▼
  ┌─────────────────────────────────────────────────────────────────────────┐
  │ 上下文汇编：`build_context_from_state` → `Vec<ChatMessage>`               │
  │  • system（system_prompt）                                                │
  │  • 历史 user / assistant / tool                                          │
  └────────────────────────────────┬────────────────────────────────────────┘
                                   │
                                   ▼
  ┌─────────────────────────────────────────────────────────────────────────┐
  │ `AgentLoop` + `tool_exec` + primitives                                   │
  │  • 模型输出 tool_calls → 执行工具 → `ChatMessage::tool` 回填               │
  │  • 循环直到无 tool_calls 或达上限                                         │
  └────────────────────────────────┬────────────────────────────────────────┘
                                   │
                                   ▼
                    ChatRequest { model, messages, tools?, stream, … }
                                   │
                                   ▼
  ┌─────────────────────────────────────────────────────────────────────────┐
  │ `trait LlmProvider`                                                      │
  │   └── 当前唯一主力：`OpenAiProvider`（`src/core/llm/openai.rs`）           │
  └────────────────────────────────┬────────────────────────────────────────┘
                                   │
                                   ▼
              POST {api_base}/v1/chat/completions
                     Authorization: Bearer …
                                   │
                    ┌──────────────┴──────────────┐
                    ▼                             ▼
             stream: true                   stream: false
           SSE chunks 解析                  一次性 JSON body
           StreamEvent::ContentDelta …       ChatResponse
```

**读图要点**：

- **wire 收敛**：线上请求体长期是 **OpenAI Chat Completions JSON**（`OpenAiRequestBody`），**没有**第二套 `/v1/responses` 并行线（与 openclaw §5.4 对比）。
- **「内部协议」**：进程内用 **`ChatMessage` / `ChatRequest`**（ serde 与 OAI 对齐），**尚未**走到 pi_agent_rust 那种独立 `model::Message` AST。
- **扩展缝**：换厂商 = 新 **`impl LlmProvider`** 或同一 trait 下切换配置；**加 vision** = `ChatMessageContentPart` + `openai.rs` 序列化补 **`image_url`** 后再走同一路 POST。

---

## 8. 横向对比：如何「抽象支持多个 LLM 接口」

| 维度 | pi_agent_rust | hermes-agent | openclaw | pi-mono | tomcat（现状） |
|------|----------------|--------------|----------|---------|----------------------|
| **抽象单元** | Rust **`Provider` trait** | Python **`ProviderTransport` + `api_mode`** | **`StreamFn` + `openai` SDK 封装**（见 pi-ai/core） | **`packages/ai` Provider 模块**（TS） | Rust **`LlmProvider`** |
| **对话表示** | 内部 **`model::Message`** | 偏 **OpenAI wire JSON**，再转换 | **`Context` / pi-ai 消息管线** | **`packages/ai` 统一类型与请求构造** | **OpenAI 风格 `ChatMessage`** |
| **路由方式** | **`ModelEntry.api` + `create_provider`** | **`api_mode` → 选 Transport** | **按模型/传输工厂选 Completions vs Responses** | **按 provider / 模型选 `packages/ai` 内实现** | **单一 `OpenAiProvider` 实现** |
| **OpenAI** | **Completions + Responses** | **Completions** + **codex Responses** | **Completions + Responses**（同仓库双工厂函数） | **Completions**（及 Responses 相关测试/模块，见 `packages/ai`） | **仅 Completions** |
| **扩展新厂商** | 新模块 `impl Provider` + `match` | 新 **`ProviderTransport`** | 新增 transport 工厂 / 网关适配 | 新 **`packages/ai` provider** + coding-agent 接线 | 新 **`impl LlmProvider`** |

### 8.1 ASCII：五家「抽象边界」鸟瞰（示意）

```text
pi_agent_rust   hermes-agent      openclaw              pi-mono              tomcat（现状）
─────────────   ────────────      ────────              ───────              ─────────────────────

 model::Message  OAI msgs[]       pi-ai Context +       packages/ai          ChatMessage
       │              │           openai SDK            providers/*
       ▼              ▼                 │                    │                      ▼
 trait Provider   Transport    completions vs       TS Provider           LlmProvider
       │              │           responses                模块
       ▼              ▼                 │                    │                      ▼
 HTTP×N          normalize…      StreamFn              coding-agent           OpenAiProvider
 Anthropic…                     事件流                编排依赖 ai               （仅 completions）
```

---

## 9. 对 tomcat 的启示（非规范，仅技术建议）

1. **若只增加图片**：在 **`ChatMessageContentPart`** 与 **`openai.rs` 序列化** 中补齐 **`image_url`**，仍可保留 **`/v1/chat/completions`**，与 **pi_agent_rust `OpenAIProvider`** 思路一致。  
2. **若要 PDF 走官方「文件输入」**：需评估 **`/v1/responses` + `input_file`**（或上游封装），与 **pi_agent_rust `OpenAIResponsesProvider`**、**hermes `codex_responses` transport**、**openclaw `createOpenAIResponsesTransportStreamFn`** 同属 **Responses 系**，而非仅在 Chat Completions 里塞 base64 字符串。  
3. **对标 hermes**：多厂商时引入 **「中间 wire 格式 + 适配层」** 可降低 Agent 循环复杂度；对标 **pi_agent_rust** 则 **强统一内部 `Message` + trait** 长期更可测。二者差别见 **§9.1**。

### 9.1 附录：两种策略差在哪？（文字 + ASCII）

**一句话**：  
- **Hermes 式**：Agent 循环只认 **一种「长得像 OpenAI」的中间 JSON**；换厂商 = **换 Transport 适配器**，核心循环不改。  
- **pi_agent_rust 式**：Agent 循环只认 **自家 `Message` 类型**；换厂商 = **换 `impl Provider`**，**wire JSON 是各 Provider 的私有细节**，**单测优先打在 `Message` / `StreamEvent` 上**。

#### （A）Hermes：中间 wire + 适配层

```text
  ┌─────────────────────────────────────────────────────────────┐
  │  Agent 循环 / AIAgent：永远只构造同一种结构                    │
  │  messages: [{role, content}, …]   tools: OpenAI function 形状 │
  │  （文档称「OpenAI-format messages」——即中间 wire）             │
  └────────────────────────────┬────────────────────────────────┘
                               │ 不随厂商变
                               ▼
              ┌────────────────────────────────────┐
              │  api_mode → 选 ProviderTransport    │
              └────────────────┬───────────────────┘
                               │
         ┌─────────────────────┼─────────────────────┐
         ▼                     ▼                     ▼
   chat_completions      anthropic_messages    codex_responses
   （近乎 identity）      convert_messages      → Responses input
         │                     │                     │
         └─────────────────────┴─────────────────────┘
                               │
                               ▼
                    各厂商 SDK / HTTP 请求体（长相各异）
                               │
                               ▼
                    normalize_response → 统一 NormalizedResponse
```

**优点**：业务侧 **零概念负担**——「我就按 OpenAI 聊天 JSON 写」。  
**代价**：中间层 **绑定 OpenAI 消息隐喻**（role、content 形态）；极端模型若与 OpenAI 差太远，适配层会 **越来越厚**。

#### （B）pi_agent_rust：内部 Message + Provider trait

```text
  ┌─────────────────────────────────────────────────────────────┐
  │  Agent 循环：只操作 crate::model::Message / ToolCall / …     │
  │  （枚举、强类型，不是「随便一个 JSON」）                      │
  └────────────────────────────┬────────────────────────────────┘
                               │
                               ▼
              ┌────────────────────────────────────┐
              │  Context { messages, tools, … }    │
              │         ↓                         │
              │  Arc<dyn Provider>::stream()       │
              └────────────────┬───────────────────┘
                               │
         ┌─────────────────────┼─────────────────────┐
         ▼                     ▼                     ▼
   AnthropicProvider    OpenAIProvider      OpenAIResponsesProvider
         │                     │                     │
         ▼                     ▼                     ▼
   编码为 Messages API   编码为 chat/completions  编码为 /responses
   （wire 细节封装在模块内）   （wire 细节封装在模块内）
```

**优点**：**编译器 + 单元测试**能锁住「一轮对话里允许出现哪些块」；换 API **不污染** Agent 状态机。  
**代价**：自己要维护 **一套领域模型**（`Message` / `UserContent` / …），入门成本 **略高于**「全民 OpenAI JSON」。

#### （C）并排对照（心智）

```text
              Hermes 倾向                          pi_agent_rust 倾向
         ─────────────────────                  ─────────────────────────
  「真理」在哪    OpenAI 形状的中间 JSON              自有 Message AST
  谁翻译       Transport（按 api_mode）             各 Provider::stream 内部
  Agent 改动    加厂商 → 常不动循环                  加厂商 → 不动 Message，只加 impl
  测试抓手      NormalizedResponse / 集成             Message / StreamEvent 单元测
```

#### （D）和 tomcat 的关系（当下）

- 你们已是 **「固定 OpenAI ChatRequest」**，更接近 **wire 收敛** 一条路；若将来 **Anthropic 直连**，要么像 **hermes** 一样 **在循环外增加一层「统一 wire」+ 多 Transport**，要么像 **pi** 一样 **先引入内部 `Message` 再实现第二个 `LlmProvider`**。演进可视化见 **（E）（F）**。

#### （E）ASCII：若照现状往下长，以后大概两条岔路

**现状（单一路径）**——与 §7.4 一致：

```text
  AgentLoop ──► ChatRequest(OpenAI 形) ──► OpenAiProvider ──► /v1/chat/completions
```

**岔路 A ——继续押「现有协议当真理」（偏 Hermes / 少动循环）**  
多接一个厂商时：**Agent 仍拼同一种中间结构**（可继续是 `ChatMessage[]` 或显式叫「OAI 兼容层」），**按配置** 选不同 **HTTP 客户端**；差异都在 **Adapter / 第二份 `LlmProvider`** 里。

```text
                    同一套 ChatMessage[] / 或固定中间 JSON
                                  │
              ┌───────────────────┼───────────────────┐
              ▼                   ▼                   ▼
       OpenAiProvider       AnthropicAdapter      FooBarProvider
       （现有）              Provider               （new impl）
              │                   │                   │
              ▼                   ▼                   ▼
    POST …/chat/completions   POST …/v1/messages   POST …/vendor
```

**岔路 B ——先造内部 IR，再挂多家 wire（偏 pi_agent_rust）**  
先引入 **`InternalMessage`（示意名）** 或统一 **`ConversationTurn`**，**AgentLoop 只操作 IR**；每种 LLM 一个 **`encode(IR)→HTTP`** / **`decode(SSE)→事件`**。

```text
         AgentLoop 只摸 InternalMessage / ToolCall 枚举
                          │
                          ▼
              ┌───────────────────────┐
              │  LlmBackend trait ?    │
              │  stream(IR) → events   │
              └───────────┬───────────┘
                          │
            ┌─────────────┼─────────────┐
            ▼             ▼             ▼
      OpenAIWire    AnthropicWire   …
```

**怎么选（极简）**：团队更熟 OAI → **岔路 A 成本低**；强要多厂商回归测试、工具语义分叉 → **岔路 B 后期更清晰**。

#### （F）ASCII + 清单：每多接 **一家** LLM API，通常要动哪里？

```text
  配置入口（model / base_url / api_key）
           │
           ▼
  ┌─────────────────────────────────────────────┐
  │ 1. 协议：REST 路径、Auth、流式是否 SSE        │
  │ 2. 请求体：与现有 ChatRequest 差多少？        │
  │ 3. 响应：流式 chunk / tool_calls / usage 解析 │
  │ 4. 与 Agent 的契约：tool 名、role、多模态块   │
  └─────────────────────────────────────────────┘
           │
           ├─ 岔路 A：new `impl LlmProvider` + 配置路由（或 match provider）
         或  ├─ 岔路 B：先加 IR 转换层，再 new backend
           │
           ▼
  单测 / 集成：对真实或 mock 端点跑一轮 tool 环
```

| 步骤 | 岔路 A（多 `LlmProvider`） | 岔路 B（先 IR） |
|------|----------------------------|-----------------|
| 1 | 在 **`types.rs`** 补该协议需要的 **消息 / 多模态** 字段（或保持 OAI 形、在 impl 里转） | 定义/扩展 **IR 类型** + **IR ↔ 现有 `ChatMessage` 迁移** |
| 2 | 新增 **`src/core/llm/anthropic.rs`（示例）`**：`chat` / `chat_stream`、URL、body、header | 新增 **Backend**：**IR → 厂商 JSON**、**SSE → `StreamEvent`** |
| 3 | 在 **选模型/选 provider** 处（如 `preflight` / 配置）**路由到** 新 impl | 在 **进 LLM 前** 把 `ChatMessage` **升成 IR**；出 **再落回** transcript 形 |
| 4 | **工具 schema**：若厂商要求不同 tool 形状，在 **进请求前** 做一层 map | 同上，或把 tool 定义挂在 IR 层 |
| 5 | 流式：**SSE 解析** 与 **工具调用分片** 对该厂实现一遍 | 同左，但测 IR 中间状态 |
| 6 | 文档与 **env**：`base_url`、模型名、限流 | 同左 + **IR 版本** 说明 |

**固定成本**（两条岔路都要）：**密钥、网络、错误码、观测（log / trace）**、**为 CI 准备 mock 或 vcr**。

### 9.2 用哪个 `LlmProvider`：配置 vs 代码？各家怎么做？

**结论先说**：**可以两者并存**——**实现类型（哪一家 API）** 与 **模型名字符串** 常常分开；**tomcat 当前是「代码定实现 + 配置定模型名」**。

#### tomcat（本仓库，现状）

| 维度 | 谁决定 | 说明 |
|------|--------|------|
| **`LlmProvider` 具体类型** | **代码** | `ChatContext::from_config` 里写死 **`Arc::new(OpenAiProvider::new(&config.llm)?)`**（`api/chat/mod.rs`），运行时 **不会** 根据 TOML 换 `AnthropicProvider`。 |
| **模型 ID / `base_url` / key** | **配置** | `config.llm` + 会话级 **`model_override`**（`effective_model`：优先会话，否则 `default_model`）。 |
| **测试** | **代码** | 单测里 **`MockLlmProvider`** 等 **注入** `Arc<dyn LlmProvider>`，不读配置。 |

```text
  配置 / 会话
  model 字符串、api_key、base_url ──► 只喂给 **同一个** OpenAiProvider
  （换「厂商」在现状下 = 换 base_url/模型名，**协议仍是 OpenAI Completions**）

  「换实现类型」──► 要改 **构造处**（new 别的 impl）或引入 **工厂 + 配置键 provider=…**
```

#### 其他仓库（对照）

| 项目 | Provider / 传输「谁来选」 | 典型机制 |
|------|---------------------------|----------|
| **pi_agent_rust** | **配置 + 模型注册表 + 动态工厂** | **`ModelEntry`**（含 **`api`**：`openai-completions` / `openai-responses` / …）→ **`create_provider(entry)`** 返回 **`Arc<dyn Provider>`**；换厂商主要是 **换 registry 元数据**，而非手写一堆 `if`。 |
| **hermes-agent** | **配置 + 运行时解析** | **`provider` / `model` / `api_mode`**（及兼容 **`base_url`**）决定 **Transport** 与 SDK 调用；一轮对话里可调 env/config。 |
| **openclaw** | **模型清单 + 代码里的 StreamFn 注册** | **`Model`**（来自 inventory / pi-ai）带 **`api`** 字段；**`createOpenAICompletionsTransportStreamFn`** vs **`createOpenAIResponsesTransportStreamFn`** 等 **按模型元数据选用**；网关 HTTP 层另有路由。 |
| **pi-mono** | **配置 / UI 选模型 + `packages/ai` 路由** | 模型与 provider 定义在 **AI 包**；coding-agent **按所选模型** 走对应 **provider 模块**（具体文件名随版本变，语义与 **registry + 工厂** 同类）。 |

#### 一句话

- **tomcat**：**实现类固定**（当前仅 OpenAI 适配器），**参数靠配置**；要「配置切换 Anthropic」需 **产品化改造**（配置里增加 `provider` + **工厂**或 **match**）。  
- **pi_agent_rust / pi-mono / hermes / openclaw**：普遍 **「元数据驱动」**——**模型/厂商条目** 决定 **走哪条 API / 哪个 Transport**，而不是在业务入口写死一个 concrete class。

### 9.3 场景化模型：用配置区分「主对话 / 压缩 / PDF…」

你的想法与 **「同一 `LlmProvider`，不同调用路径选不同 `model` 字符串」** 完全一致；这在 tomcat 里 **不与 §9.2 冲突**——换厂商仍是 trait/工厂问题，**换场景仍是模型 ID + 请求体差异**。

#### 现状（代码已落地的）

| 场景 | 配置键（示例） | 优先级 / 说明 |
|------|------------------|----------------|
| **主对话（Agent 轮询）** | `[llm] default_model` + 会话 **`model_override`** | `effective_model`：**会话覆盖优先**，否则默认模型（见 `api/chat/mod.rs`）。 |
| **Compaction / 预摘要** | **`[context] compaction_model`** | `core/compaction/preheat.rs` 里 **`generate_summary(..., compaction_model)`** 单独组 `ChatRequest`；默认与 `DEFAULT_LLM_MODEL` 同源，可改成 **`gpt-4o-mini`** 等低成本模型。 |

`config_get` / `config_set` 白名单已包含 **`context.compaction_model`**（见 `core/tools/config.rs`）。

#### 尚未单独建模、可按同一模式扩展的

| 方向 | 说明 |
|------|------|
| **PDF / 多模态 / 附件解析** | 当前主线仍以文本 chat 为主；若未来某条路径（例如专门 OCR、vision）需要 **不同模型或 max_tokens**，宜新增 **`[context]` 或 `[llm]` 下的专用键**（如 `vision_model`），在 **该路径组请求时** 读配置——与 compaction 同一套路，**无需**换 `LlmProvider` 实现。 |
| **优先级惯例** | 可约定：**会话 override > 场景键 > `default_model`**（仅主对话需要会话覆盖；压缩路径通常只认 `compaction_model`，避免与用户临时换主模型纠缠）。 |

```text
  [llm] default_model ───────────► 主对话 AgentLoop
  session.model_override ───────►（覆盖 default_model）

  [context] compaction_model ───► compaction / preheat 摘要请求（已与主模型解耦）

  （未来）pdf_model / vision_model ─► 专门入口读配置，同一 OpenAiProvider
```

---

## 10. 参考路径（便于代码跳转）

- **pi_agent_rust**：`pi_agent_rust/src/provider.rs`，`pi_agent_rust/src/providers/openai.rs`，`pi_agent_rust/src/providers/openai_responses.rs`，`pi_agent_rust/src/providers/mod.rs`（`create_provider`）  
- **hermes-agent**：`hermes-agent/agent/transports/base.py`，`chat_completions.py`，`anthropic.py`，`codex.py`  
- **openclaw**：`openclaw/src/agents/openai-transport-stream.ts`（`createOpenAICompletionsTransportStreamFn`、`createOpenAIResponsesTransportStreamFn`），`openclaw/src/gateway/openai-http.ts`（OpenAI 兼容 HTTP + 图片解析）  
- **pi-mono**：`pi-mono/packages/ai/src/providers/`（如 **`openai-completions.ts`**），`pi-mono/packages/coding-agent/`（编排入口侧）  
- **tomcat**：`tomcat/src/core/llm/provider.rs`，`tomcat/src/core/llm/openai.rs`，`tomcat/src/core/llm/types.rs`；**Provider 构造**：`tomcat/src/api/chat/mod.rs`（`ChatContext::from_config` 内 `OpenAiProvider::new`）；**主对话模型**：同文件 **`effective_model`**；**压缩摘要模型**：`tomcat/src/infra/config/types.rs` **`ContextConfig.compaction_model`** + `tomcat/src/core/compaction/preheat.rs` **`generate_summary`**

---

## 11. 修订记录

| 日期 | 说明 |
|------|------|
| 2026-05-04 | 初稿：基于工作区源码检索与模块阅读 |
| 2026-05-04 | 补充 §2.1、§3.3.1、§4.2.1、§7.3、§8.1 ASCII 示意图 |
| 2026-05-04 | 更正：**`openclaw` 位于 `Tomcat/openclaw/`**；新增 §5 openclaw；§6 pi-mono；章节顺延；对比表加入 openclaw 列 |
| 2026-05-04 | 更正：**`pi-mono` 在磁盘 `Tomcat/pi-mono/` 为完整 monorepo**（非仅占位）；§1 / §6 / §8 / §10 同步；区分 **`pi_agent_rust/legacy_pi_mono_code` stub** |
| 2026-05-04 | 新增 **§9.1**：Hermes「中间 wire + 适配层」vs pi「内部 Message + trait」对照（ASCII） |
| 2026-05-04 | 新增 **§7.4**：tomcat 全链路 ASCII（入口 → AgentLoop → `LlmProvider` → `/v1/chat/completions`） |
| 2026-05-04 | §9.1 增补 **（E）（F）**：tomcat 演进两条岔路 + 每接一家 LLM 的清单与 ASCII |
| 2026-05-04 | 新增 **§9.2**：`LlmProvider` 选型（配置 vs 代码）+ 与 hermes / openclaw / pi_agent_rust / pi-mono 对照表 |
| 2026-05-04 | 新增 **§9.3**：场景化模型（默认 / 压缩 / PDF 等）— 与配置一致；写明 **`context.compaction_model` 已落地**、PDF/vision 可按同模式扩展；§10 参考路径补 compaction |
