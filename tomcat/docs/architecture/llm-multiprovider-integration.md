# 多 LLM / OpenAI 对接技术方案（架构 spec）

本文档承接技术报告 [`docs/reports/multi-agent-openai-api-integration.md`](../reports/multi-agent-openai-api-integration.md)：将其中 **与 tomcat 架构冻结相关的结论** 收敛为 openspec 条目；**调研过程、五仓长篇对照与修订履历** 仍以报告为准。

> **位置说明**：报告记录检索范围、ASCII 示意与横向评分式叙述；本文为 **实现边界 + 类型契约 + 代码锚点 + 演进约束**，便于 Agent Loop / 配置 / 新 Provider 实施时对齐。
>
> **边界说明**：若问题是“同一份 transcript 如何在 OpenAI / DeepSeek 间续传 reasoning、如何 downgrade、何时 replay `reasoning_content` / `encrypted_content`”，请转 [**OpenAI / DeepSeek 推理续传架构方案**](llm-openai-deepseek-reasoning-continuity.md)。本文只讲 **多 provider 主骨架与 wire 接线**，不展开 cross-turn reasoning continuity 细则。

---

## 1. 背景与对标

### 1.1 范围与材料来源

| 仓库 | 本工作区可用性 | 说明 |
|------|----------------|------|
| **pi_agent_rust** | 完整 | `Provider` trait、`create_provider`、`providers/*` |
| **hermes-agent** | 完整 | `ProviderTransport`、`api_mode`、`agent/transports/*` |
| **tomcat** | 完整 | `LlmProvider`、`OpenAiProvider` → Chat Completions |
| **pi-mono** | 完整（磁盘 `Tomcat/pi-mono/`） | `packages/ai` 多 Provider；**非** `pi_agent_rust/legacy_pi_mono_code` stub |
| **openclaw** | 完整（磁盘 `Tomcat/openclaw/`） | `openai-transport-stream.ts`、网关 `openai-http.ts` |

### 1.2 OpenAI 官方接口划分（本话题）

| 接口 | 典型路径 | 用途（简述） |
|------|-----------|--------------|
| **Chat Completions** | `POST /v1/chat/completions` | `messages` + 可选 `tools`；多模态常用 **`content` 数组**（`text` + `image_url`） |
| **Responses** | `POST /v1/responses` | 新一代统一接口：`input` 条目；文档侧常与 **PDF `input_file`** 等一并描述 |
| **其它** | Embeddings、Realtime、Files… | 本 spec 聚焦主对话循环，不展开 |

```text
                         同一账号 / Key（示意）
                                    │
        ┌───────────────────────────┼───────────────────────────┐
        ▼                           ▼                           │
 POST /v1/chat/completions          POST /v1/responses          │  其它 /v1/…
        │                           │                           │
  messages[]                         input[]                     │
  tools                              tools（映射策略不同）          │
```

### 1.3 五项目「多 LLM 抽象」横向对照

| 维度 | pi_agent_rust | hermes-agent | openclaw | pi-mono | tomcat（现状） |
|------|----------------|--------------|----------|---------|----------------------|
| **抽象单元** | `Provider` trait | `ProviderTransport` + `api_mode` | `StreamFn` + `openai` SDK 封装 | `packages/ai` Provider | **`LlmProvider`** |
| **对话表示** | 内部 `model::Message` | 偏 OpenAI wire，再转换 | `Context` / pi-ai 管线 | `packages/ai` 类型 | **`ChatMessage`（OpenAI 形）** |
| **路由方式** | `ModelEntry.api` + `create_provider` | `api_mode` → Transport | 模型元数据选 Completions vs Responses | 模型 / provider 选实现 | **`resolve_llm` + [`registry.rs`](../../../src/core/llm/registry.rs)**（`[llm] provider` 字符串 → `Arc<dyn LlmProvider>`） |
| **OpenAI** | Completions + Responses | Completions + codex Responses | Completions + Responses | Completions（+ Responses 相关模块） | **Completions（`OpenAiProvider`）+ Responses（`OpenAiResponsesProvider`，默认）** |
| **扩展新厂商** | 新 `impl Provider` + match | 新 Transport | 新 transport 工厂 | 新 `packages/ai` provider | **新 `impl LlmProvider` + 接线** |

```text
pi_agent_rust   hermes            openclaw         pi-mono              tomcat（现状）
─────────────   ───────           ────────         ───────              ────────────────────
Message AST     OAI-ish wire      pi-ai Context    packages/ai          ChatMessage
     │              │                  │                 │                      │
trait Provider   Transport         StreamFn          TS Provider          LlmProvider
     │              │                  │                 │                      │
HTTP×N          normalize…        事件流             coding-agent          registry → OpenAi*
Anthropic…                                                           Completions / Responses
```

### 1.4 对标仓库要点（摘要）

- **pi_agent_rust**：统一 **`Context` + `Provider::stream`**；**`openai-completions`** 与 **`openai-responses`** 两条 HTTP 由 **`create_provider`** 按 **`ModelEntry.api`** 分发。详见报告 §3。
- **hermes-agent**：**`api_mode`** 选择 **`ProviderTransport`**；**`convert_*` → `build_kwargs` → `normalize_response`**。详见报告 §4。
- **openclaw**：**`createOpenAICompletionsTransportStreamFn`** 与 **`createOpenAIResponsesTransportStreamFn`** 双轨；网关可将类 OpenAI 请求转为内部 `command`。详见报告 §5。
- **pi-mono**：**`packages/ai`** 承担多厂商协议；**`packages/coding-agent`** 编排。详见报告 §6。

---

## 2. tomcat 当前实现（冻结描述）

### 2.1 抽象层

- **`LlmProvider`**（[`src/core/llm/provider.rs`](../../../src/core/llm/provider.rs)）：**`chat` / `chat_stream` / `count_tokens`**。
- **`resolve_llm`**（[`src/core/llm/registry.rs`](../../../src/core/llm/registry.rs)）：按 **`LlmConfig.provider`** 字符串构造 **`Arc<dyn LlmProvider>`**（当前登记 **`openai`** → Completions、**`openai-responses`** → Responses）。
- **`ChatMessage` / `ChatRequest`**（[`src/core/llm/types.rs`](../../../src/core/llm/types.rs)）：统一承载内部对话语义；`ChatMessageContent` 支持 **Parts**，**`ChatMessageContentPart` 为三态枚举**：`InputText` / `InputImage` / `InputFile`。当前三条 wire 家族都从它翻译：**Responses**（`input_image` / `input_file`）、**Chat Completions**（`image_url` / `file` content 数组）、**Anthropic Messages**（`image` / `document` block）。

### 2.2 HTTP 与能力边界

- **`OpenAiProvider`**（[`src/core/llm/openai.rs`](../../../src/core/llm/openai.rs)）：固定 **`POST {base}/v1/chat/completions`**，body 为 **`OpenAiRequestBody`**（model、messages、temperature、max_tokens、tools、stream）。**`[llm] provider = "openai"`** 时选用。
- **`OpenAiResponsesProvider`**（[`src/core/llm/openai_responses.rs`](../../../src/core/llm/openai_responses.rs)）：固定 **`POST {base}/v1/responses`**；同一套 **`ChatRequest` / `ChatMessage`** 在实现内翻译为 **`input` + `instructions` + tools 映射**；流式 **SSE / NDJSON** → **`StreamEvent`**。**默认** **`[llm] provider = "openai-responses"`**。
- **结论（架构边界）**：主线默认 **Responses**；但多模态不再是 Responses 独占。当前 **OpenAI Responses / OpenAI Completions / Anthropic Messages** 都能消费同一份 `ChatMessageContentPart`，差异被切成两层：**wire 函数**负责各家消息 JSON，**`FilesApiAdapter`** 负责各厂商上传/删除与极小的引用提示（如 Moonshot `ms://`、Anthropic beta 头）。

### 2.3 全链路 ASCII（入口 → Agent → LLM HTTP）

```text
  入口层（api/chat · ext/dispatcher …）
         │
         ▼
  build_context → Vec<ChatMessage>（system / 历史 / tool）
         │
         ▼
  AgentLoop + tool_exec + primitives
         │
         ▼
  ChatRequest { model, messages, tools?, stream, … }
         │
         ▼
  resolve_llm(&config.llm) → Arc<dyn LlmProvider>
         │
         ├─ OpenAiProvider ─────────► POST …/v1/chat/completions
         └─ OpenAiResponsesProvider ─► POST …/v1/responses
         │
         ├─ stream: true  → SSE / NDJSON → StreamEvent::…
         └─ stream: false → JSON → ChatResponse
```

### 2.4 配置：Provider 类型 vs 模型字符串 vs 场景键

| 维度 | 谁决定 | 说明 |
|------|--------|------|
| **`LlmProvider` 具体类型** | **配置 + 注册表** | **[`llm`] `provider`** 字符串 → [`resolve_llm`](../../../src/core/llm/registry.rs) → **`Arc<dyn LlmProvider>`**（[`ChatContext::from_config`](../../../src/api/chat/mod.rs)）；新增后端 **登记表一行**，**不**在入口手写长篇 `match`。Anthropic 等非 OpenAI 形后端仍按 **岔路 A** 新增 `impl LlmProvider`。 |
| **主对话模型 ID** | **配置 + 会话** | **[`llm`] `default_model`** + **`SessionEntry.model_override`** → **`effective_model`**（会话优先）。 |
| **Compaction 摘要模型** | **配置** | **[`context`] `compaction_model`** → [`generate_summary`](../../../src/core/compaction/preheat.rs)；与主对话解耦，可配低成本模型。 |
| **测试** | **注入** | `MockLlmProvider` 等替换 **`Arc<dyn LlmProvider>`**。 |

**场景化扩展惯例（建议）**：未来 **vision / PDF 专用路径** 可新增 **`vision_model` / `pdf_model`** 等键，在**该路径**组 `ChatRequest` 时读取——仍由 **当前选中的 `LlmProvider` impl** 负责 wire（Completions 或 Responses）。详见报告 §9.3。

#### 2.4.1 复用 OpenAI adapter 的 vendor 案例：DeepSeek 与 Xiaomi MiMo

DeepSeek 与 **Xiaomi MiMo（`mimo-v2.5-pro`，Token Plan）** 都是「复用 `provider="openai"` Chat Completions adapter，不新增 provider」的范例——区别只是凭证 + base URL + thinking wire + continuity：

| 维度 | DeepSeek | MiMo (`mimo-v2.5-pro`) |
|------|----------|------------------------|
| 注册表 id（`api`） | `openai` | `openai` |
| 逻辑 `provider` | `deepseek` | `mimo` |
| 凭证 env | `DEEPSEEK_API_KEY` | `MIMO_API_KEY`（`auth.rs::env_name_for_provider` 通用推导，`tp-xxxxx` 不与 `sk-xxxxx` 混用） |
| `base_url`（只填 host） | `https://api.deepseek.com` | `https://token-plan-cn.xiaomimimo.com` |
| endpoint 后缀 | `/v1/chat/completions`（由 `openai.rs` 拼接，不可配） | 同左 |
| thinking 线格式 | `deepseek` | `doubao`（`thinking: {"type":"enabled"}`） |
| 能力 | text/tools/reasoning | text/tools/reasoning（**无 vision/files**，官方文档定死） |
| `reasoning_content` continuity | 数据表行 `deepseek-v4` | 数据表行 `mimo-v2.5-pro`（同一条逻辑，见续传文档 §4.2.3.1） |
| 事实源 | 内嵌 `builtin_models.toml`（由 `catalog.rs` 解析成 builtin） | **`tomcat init` 生成的 `~/.tomcat/models.toml`**（用户可见 seed / 覆盖层） |

要点：**MiMo 全程零新增 provider / 零改 transport / 零改 continuity 5 道门**，只靠一条 `models.toml` 数据 + `MIMO_API_KEY` 即可上线，是「数据驱动接入同类 LLM」的活样板。`tomcat init` 会幂等生成这条 `models.toml`（缺则补、不覆盖用户内容）。

---

## 3. 进程内协议与 wire 形状

### 3.1 `LlmProvider` 契约

调用方（Agent Loop、Compaction、测试）**只依赖 trait**，不依赖 `OpenAiProvider` 具体类型：

- **`chat(ChatRequest) -> ChatResponse`**：非流式。
- **`chat_stream(ChatRequest) -> Stream<StreamEvent>`**：流式；SSE 解析在实现内完成。
- **`count_tokens`**：预算 / 观测用（实现精度依模型而定）。

### 3.2 `ChatRequest` / `ChatMessage` 与 OpenAI JSON

- 序列化目标与 **`OpenAiRequestBody`** 对齐（见 [`openai.rs`](../../../src/core/llm/openai.rs)）。
- **工具**：`tools` 为 OpenAI function 形状；Compaction 路径 **显式 `tools: None`**（见 `generate_summary` 注释——双保险不加 tool schema）。
- **当前冻结结论**：调用方继续只组 **一份** `ChatRequest` / `ChatMessageContentPart`；多模态差异不再由上层分支。具体该发 `input_file`、`image_url.file_id`、`image_url.url=ms://...`、还是 Anthropic `document.source={type:file,...}`，统一由各 wire 家族函数在 provider 内部决定（见 **§6.5.3–§6.6**）。

#### 3.2.1 Completions / Responses：**注册表 id 管的是「谁翻译」，不是「两套 ChatRequest」**

| 问题 | 推荐结论 |
|------|----------|
| 换 API 要不要组 **两套** `ChatRequest` 字段？ | **通常不要。** Agent 仍按现有习惯组 **一份** `ChatRequest`（`model`、`messages`、`tools`、`stream` 等）。 |
| `provider` / 注册表 id 干什么用？ | 只决定 **`Arc<dyn LlmProvider>` 用哪一个实现**：例如 **`openai`** → Completions 适配器；**`openai-responses`** → Responses 适配器。**差别在 Provider 内部**：同一坨 **`messages`**，前者序列化成 **`messages[]`** POST，后者 **翻译成 `input` + `instructions`（及工具形状）** 再 POST。 |
| 调用方要不要知道当前是 Completions 还是 Responses？ | **不需要。** 协议差异 **封装在 `XxxProvider::chat` / `chat_stream`** 里。 |
| 何时才要在 `ChatRequest` 上 **加新字段**？ | 仅当引入 **现有 `ChatMessageContentPart` 无法表达** 的新能力时才扩类型。图片 / PDF / `file_id` / inline base64 现在都能由现有 part 表达；差异留在 provider 内部 wire，而不是再裂出多套 `ChatRequest`。 |

### 3.3 流式与非流式

- **stream: true**：字节级 SSE → **`StreamEvent`**（content delta、tool_calls 分片、usage 等，以代码为准）。
- **stream: false**：单次 JSON **`ChatResponse`**。

### 3.4 配置-driven 的客户端参数

**[`LlmConfig`](../../../src/infra/config/types.rs)**：`api_base`、`api_key_env`、`retry_count`、`stream_timeout_sec`、`proxy`、`api_base_fallback` 等——**与「选哪个 Provider impl」正交**；**`OpenAiProvider` / `OpenAiResponsesProvider`** 等实现 **共用**上述横切字段。

---

## 4. One-Glance Map（文件聚合）

> 一图聚合：**谁在组装消息、谁调用 trait、谁打 HTTP**。

```text
┌─────────────────────────────────────────────────────────────────────────┐
│  src/api/chat/mod.rs          ChatContext::from_config                   │
│    · resolve_llm(&config.llm) → Arc<dyn LlmProvider>                     │
│    · effective_model（会话 model_override 优先）                         │
├─────────────────────────────────────────────────────────────────────────┤
│  src/core/llm/registry.rs    PROVIDERS 表 · resolve_llm / registered ids │
├─────────────────────────────────────────────────────────────────────────┤
│  src/core/agent_loop/*        AgentLoop · preflight · tool_exec          │
│    · 组装 ChatRequest（主对话）                                           │
├─────────────────────────────────────────────────────────────────────────┤
│  src/core/llm/types.rs        ChatMessage · ChatRequest · StreamEvent    │
│  src/core/llm/provider.rs     trait LlmProvider                          │
│  src/core/llm/openai.rs       Completions · /v1/chat/completions          │
│  src/core/llm/openai_responses.rs   Responses · /v1/responses            │
├─────────────────────────────────────────────────────────────────────────┤
│  src/core/compaction/preheat.rs                                          │
│    · generate_summary → ChatRequest { model: compaction_model, tools: None } │
├─────────────────────────────────────────────────────────────────────────┤
│  src/infra/config/types.rs    LlmConfig（provider 字符串 · 横切字段）· ContextConfig │
└─────────────────────────────────────────────────────────────────────────┘
```

**阅读路径**：

- **换模型（主对话）** → `api/chat/mod.rs` **`effective_model`** + 当前 Provider 内 **`ChatRequest.model`**。
- **换模型（压缩）** → `ContextConfig.compaction_model` + `preheat::generate_summary`。
- **Completions → Responses** → **`impl LlmProvider`（新建）** + **§6.5** 清单；**构造处** `ChatContext::from_config` 按配置选型。

---

## 5. 调度时序

### 5.1 主对话（简化）

```text
User input
    → build_context (system + transcript + tool results)
    → AgentLoop: chat_stream(ChatRequest { model: effective_model, tools: Some(...) })
    → LlmProvider（注入实现：默认 OpenAiResponsesProvider）
    → SSE / NDJSON chunks
    → 若有 tool_calls → tool_exec → 回填 tool message → 循环
    → 无 tool_calls → 返回 assistant 文本
```

### 5.2 Compaction 摘要（并行概念路径）

```text
usage_ratio / policy 触发 preheat
    → 上层解析 compaction_provider + compaction_model
    → generate_summary(snapshot, compaction_provider, compaction_model)
    → chat(ChatRequest { model: compaction_model, tools: None, stream: false })
    → 摘要文本 → transcript / compaction 状态机（与主对话模型独立）
```

详细上下文预算、滑窗与 Compaction 产品语义见 [**上下文管理**](context-management.md)。

---

## 6. 演进路线与选型冻结

### 6.1 两种策略（与 Agent 循环的耦合度）

| 策略 | 真理来源 | 换厂商时 |
|------|----------|----------|
| **Hermes 式** | 中间 wire（OpenAI 形 JSON） | 换 **Transport / Adapter**，循环少动 |
| **pi_agent_rust 式** | 内部 **Message AST** | 换 **`impl Provider`**，wire 封装在模块内 |

tomcat **当下**已固定 **OpenAI 形 `ChatMessage`**，更接近 **wire 收敛**；若引入 **全局 IR**，属 **大改版**，需单独设计与迁移测试。

### 6.2 架构选型（冻结）：**岔路 A**

**决策**：采用 **岔路 A**——**多个 `impl LlmProvider` + 配置/元数据路由**，Agent Loop 与 transcript 仍组装 **`ChatMessage` / `ChatRequest`**；各后端在 **Provider 内部** 把同一套类型 **编码** 为 Completions、Responses 或其它 **OpenAI 风格 HTTP JSON**。

**理由**：多数推理网关与官方接口提供 **与 OpenAI 高度同形的请求/流式语义**（`messages` 或 `input[]`、SSE、function tools）。先为每条线路实现 **薄适配层**，比对 **先统一 IR 再挂多家** 迭代更快；**岔路 B（内部 IR）** 留作远期、仅在多协议分叉难以维护时再评估。

### 6.3 岔路 B（备查，不优先）

**岔路 B**：引入 **内部 IR**，AgentLoop 只操作 IR，各后端 **encode(IR)→HTTP** / **decode(SSE)→事件**。本阶段 **不采纳** 为默认路径。

### 6.4 每新增一条 HTTP 能力时的共通检查

至少覆盖：**REST 路径与 Auth**、**请求体字段与现有 `ChatRequest` 差异**、**流式（SSE / NDJSON）与 tool_calls 分片**、**与工具环的契约**、**错误码与重试**、**观测与 CI mock**（与报告 §9.1（F）清单一致）。

### 6.5 OpenAI **Responses** API（`POST /v1/responses`）接入——对标锚点与 tomcat 落地点

> **协议差异摘要**：Completions 使用 **`messages[]`**；Responses 使用 **`input[]`（items）** + 常见单独字段 **`instructions`**（system），工具 shape 与流式 **event 类型** 亦不同于 Chat Completions。实施前应对照下列仓库 **同一功能的已实现分支**，避免只凭 REST 文档手写。

#### 6.5.1 其它 Agent 中的实施点（代码锚点，便于跳转）

| 仓库 | 路径 / 符号 | 白话（这篇代码能帮到你什么） |
|------|----------------|------------------------------|
| **pi_agent_rust** | [`pi_agent_rust/src/providers/openai_responses.rs`](../../../../pi_agent_rust/src/providers/openai_responses.rs) | **Rust 版「怎么打 `/v1/responses`、怎么读回流」的完整范例**：拼请求（model、`input`、system 单独字段、工具、是否流式）；发 POST；服务端既可能推 **SSE** 也可能推 **一行一条 JSON**，这里两种都接住了；流里怎么拆 **正文增量** 和 **工具调用**。另：**Codex 那条线**换 URL、加特殊 header 也写在这里。 |
| **pi_agent_rust** | [`pi_agent_rust/src/providers/mod.rs`](../../../../pi_agent_rust/src/providers/mod.rs) | **用户填的「模型元数据」怎么走到上面的 Provider**：名字里带 **`openai-responses`** 就造 Responses 这份实现；**用户抄错的 base_url**（多写了 `/v1/chat/completions` 之类）怎么 **拧回** 正确的 **`…/v1/responses`**，避免重复路径。 |
| **pi_agent_rust** | 同上 `openai_responses.rs` 内 **`build_openai_responses_input`** | **把「一整段对话」掰成 Responses 要的 `input` 列表**：谁进列表、system 放哪、带 tool 的几轮怎么排——跟普通 Chat Completions 不是同一套形状，照这个捋清楚就不容易漏消息。 |
| **openclaw** | [`openclaw/src/agents/openai-transport-stream.ts`](../../../../openclaw/src/agents/openai-transport-stream.ts) | **TypeScript + 官方 Node SDK**：`responses.create` 怎么传参；**对话怎么转成 Responses 参数**、**工具怎么转**、**流回来怎么接着解析**；同目录还有 **payload 策略、工具 schema、reasoning** 等边角，适合对照官方行为。 |
| **hermes-agent** | [`hermes-agent/agent/transports/codex.py`](../../../../hermes-agent/agent/transports/codex.py) | **Python 侧「Transport」样板**：仍然拿 **长得像 OpenAI Chat 的 messages**，在调用前转成 Responses 要的格式，再交给下层去请求；**instructions、最大输出、各家开关** 在这里捏进 kwargs。 |
| **hermes-agent** | `hermes-agent/agent/codex_responses_adapter.py`（被上文 import） | **专门干翻译**：Chat 那套 **messages → Responses 的 `input`**，以及 **tools → Responses 认的函数定义**——和 pi_agent_rust 里是同一类脏活，换语言参考用。 |

#### 6.5.2 tomcat 建议实施清单（与岔路 A 对齐）

> **每增加一个 LLM API，是否都要做表格里全部事情？**  
> **通常不用。** 岔路 A 的**常态增量**是：**只新增一个 `XxxProvider` 模块**（`impl LlmProvider`），把该厂的 **HTTP 请求组装、流式解析、`StreamEvent` 输出** 封在模块内部；**Agent Loop、`ChatRequest`、工具环** 不改或极少改。
>
> | 增量类型 | 白话（你在干什么） | 频率 |
> |----------|-------------------|------|
> | **每个新 API（最小集）** | 多写一个 Rust 文件（例如 `src/core/llm/某某.rs`）：里面负责「把内存里的对话 **`ChatRequest`** 变成对方服务器要的 HTTP 正文」「把对方返回的一坨 **SSE / 分行 JSON** 掰成我们现成的 **`StreamEvent`**」，并把 **`LlmProvider` 要求的三个入口（一次性对话、流式对话、粗算 token）都实现掉」；最后给这个文件配上 **单测 / mock**。 | **每接一家新协议或新 URL 路径** 做一次 |
> | **横切（平台）** | **`[llm] provider = "某个 id"`**（字符串）对应 **注册表里的一个构造入口**——新实现的 **`XxxProvider` 只在该 id 上登记一次**（专用 **`registry.rs`** / 或在 Provider 模块末尾 **`inventory`/静态表一行**），**不要**每加一个 Provider 就去 **`LlmConfig` 结构体**里加新字段（横切项如 `api_base`、密钥 env、代理保持共用）。启动时用 **`provider` 字符串查表** → **`Arc<dyn LlmProvider>`**。代理、重试、fallback 仍在 **`LlmConfig`** 或共享 helper，由各 Provider 按需读取。 | **注册表架子** 搭一次；之后新增后端 ≈ **登记表一行 + 新文件**，**不改** `types.rs` 结构体 |
> | **Compaction / 摘要** | 压缩摘要是靠 **`LlmProvider::chat`** 打的——只要你换接口时 **还是走这个 trait**，`preheat` 里那坨 **一般不用动**。只有当你 **故意规定**：摘要必须用 **另一家便宜接口**（和主对话不是同一个 HTTP），才要在配置里 **单独写「摘要用谁」**，并且运行时要把 **`compaction_model` 与对应 provider 成对注入**；不能只改模型名、不改 provider。 | **听产品**，不是每接一个 LLM 都要改 |
> | **为何 Responses 首接显得「长」** | 第一条 **`/v1/responses`** 跟老的 **`/v1/chat/completions`** 不是同一种 JSON：`messages` 要改成 **`input` 列表**、system 往往拆成 **`instructions`**、工具字段也不一样——所以 **第一次接 Responses** 会多出「翻译对话格式」这块活；**以后再接同类网关**（也是 Responses 那一套），往往就是 **再写一个 Provider 文件**，并在 **注册表里多登记一个 id**。 | 第一次偏烦，后面接近「一个 xxxProvider + 一行注册」 |
>
> **与上一行的关系**：这里的「翻译」发生在 **Provider 实现内部**，**不是**让 Agent 组两套 `ChatRequest`。详见 **§3.2.1**。

##### `LlmConfig` 与 TOML：**不为每个 Provider 扩字段**

现状 **`LlmConfig`** 已有 **`provider: String`**（[`types.rs`](../../../src/infra/config/types.rs)）——与 **`[llm] provider = "…"`** 对齐。**推荐约定**：

| 做法 | 说明 |
|------|------|
| **稳定 schema** | **`types.rs` 里只保留横切字段**：`provider`、`api_base`、`api_key_env`、`default_model`、并发 / 重试 / 代理 / fallback 等；**不因新接一个厂商就增加「专属布尔 / 子结构体」**。 |
| **选用后端** | TOML：**`provider = "openai"`** / **`"openai-responses"`** / ……（字符串 id 由注册表约定）；运行时 **`resolve_llm(&config)`**：**查表** → 对应 **`fn(&LlmConfig) -> Result<Arc<dyn LlmProvider>>`**。 |
| **登记新后端** | 实现 **`XxxProvider`** 后，在 **单一注册点**（例如 **`src/core/llm/registry.rs`**）为该 id **追加一行** mapping；**或在 Provider 子模块用 `ctor`/模块加载时注册**——任选其一写进代码规范即可。**目标**：新增 LLM 线 **不改 `LlmConfig` derive**，只改 **registry + 新文件**。 |
| **Provider 私有参数** | 若某厂必须多几个旋钮：优先 **环境变量** / **该 Provider 从 `[llm]` 已有字段推导**；仍不够时再讨论 **`[llm.extra]` 表格** 或 **按 provider id 分 `[llm.some_vendor]`**（那是第二层扩展，仍可避免「中央结构体无限膨胀」）。 |

下表是 **首次把 Responses 接入 tomcat** 时的**项目级核对单**（含注册表与映射）；**基础设施稳定后**，新增同类 API 应 **收敛为：`XxxProvider` + 注册 id + 测试**。

| 落地点 | 路径 / 约定 | 工作项 |
|--------|-------------|--------|
| **配置（schema）** | [`src/infra/config/types.rs`](../../../src/infra/config/types.rs) **`LlmConfig`** | **尽量不增加**仅某一 Vendor 需要的字段；用已有 **`provider` 字符串** 区分 **Completions / Responses / …**（例如 `"openai"` vs `"openai-responses"`）。若首接需补充横切项，只加 **通用** 字段。Responses 的 URL 归一化可在 **`OpenAiResponsesProvider` 或 registry** 内完成（对齐 **`normalize_openai_responses_base`** 思路）。 |
| **Provider 解析** | 建议 **`src/core/llm/registry.rs`**（新建）+ [`src/api/chat/mod.rs`](../../../src/api/chat/mod.rs) **`ChatContext::from_config`** | **`from_config` 调用 `resolve_llm(config)`**：按 **`config.llm.provider`** **查注册表** 得到 **`Arc<dyn LlmProvider>`**；**禁止**在此处手写长篇 **`match` 每增一行改一次**——新增后端只 **登记表 + 新模块**。 |
| **新实现（核心）** | 建议 **`src/core/llm/openai_responses.rs`**（新建） | **`impl LlmProvider`**：`chat` / **`chat_stream`** 调用 **`POST {base}/v1/responses`**；请求体从 **`ChatRequest`** 映射为 **`model` + `input` + `instructions` + `tools` + `stream`**（字段名以 OpenAI 当前 REST 为准）；流式 **Accept**、**SSE vs NDJSON** 分流与 **chunk 解析** 可参考 pi_agent_rust **同文件** 的状态机，输出统一到现有 **`StreamEvent`**。 |
| **消息映射** | 新建模块或 [`types.rs`](../../../src/core/llm/types.rs) 旁 helper | **`Vec<ChatMessage>` → Responses `input` items**；**首条 system → `instructions`**（或与 openclaw/hermes 一致：仅 user/assistant 进 `input`）。tool results / assistant tool_calls 轮次需与 **pi_agent `build_openai_responses_input`** 语义对齐，避免静默丢轮次。 |
| **工具** | `openai_responses.rs` | **function 定义** 从现有 tool JSON 转为 Responses 期望的 tool 列表（参考 pi_agent **`convert_tool_to_openai_responses`**、openclaw **`convertResponsesTools`**）。 |
| **Compaction** | [`src/core/compaction/preheat.rs`](../../../src/core/compaction/preheat.rs) **`generate_summary`** | 今日为非流式 **`chat`** + **`tools: None`**。若全局切换 Responses：在本函数内 **显式走同一 trait**（由上层注入的 `compaction_provider` 决定 HTTP），或 **保留专用 Completions 端点**（配置项：`compaction 仍用 completions`）——需在配置层写清，避免摘要路径与主对话协议不一致。 |
| **Token 估算** | [`provider.rs`](../../../src/core/llm/provider.rs) 实现 | **`count_tokens`** 对 Responses 是否沿用同一启发式或标注「近似」，避免预算误判。 |
| **测试** | `src/core/llm/tests/`、集成 | **Mock SSE/NDJSON** fixtures；可选对齐 pi_agent **VCR** 模式或 `httptest` 断言路径与 **Authorization**。 |

```text
  ChatRequest (现有 Agent 组装)
       │
       ├─ OpenAiProvider ──────► POST …/v1/chat/completions   (messages[])
       │
       └─ OpenAiResponsesProvider (new)
               │  ChatMessage → input[] + instructions
               └────────────► POST …/v1/responses   (stream, tools 映射, SSE/NDJSON 解析)
```

#### 6.5.3 多模态 `Parts`（统一内部表示）— 已实现 wire

**当前冻结实现**：`ChatMessageContentPart::{InputText, InputImage, InputFile}` 继续是**唯一内部表示**；调用方不区分 Responses / Completions / Anthropic。当前已存在三条 wire 翻译函数：

- **Responses**：`part_to_responses_value` → `input_text` / `input_image` / `input_file`
- **Chat Completions**：`part_to_completions_content` → `text` / `image_url` / `file`
- **Anthropic Messages**：`content_part_to_block` → `text` / `image` / `document`

同一条 `InputImage` / `InputFile`，最终 JSON 由**当前 provider 所属 wire 家族**决定；上层不再因为“这次走的是 completions / anthropic”而改组消息。

构造 helper（[`types.rs`](../../../src/core/llm/types.rs)）：

| Helper | 通道 | 校验 |
|--------|------|------|
| `ChatMessageContentPart::text(s)` | — | — |
| `image_b64(mime, &Path)` | A：inline | metadata 字节 `<= IMAGE_MAX_BYTES` (4.5 MB) + MIME ∈ {png,jpeg,gif,webp}；helper 内部读盘 + base64（PR-RJ-0 重构） |
| `file_b64(filename, mime, &Path)` | A：inline | metadata 字节 `<= FILE_MAX_BYTES` (25 MB)；helper 内部读盘 + base64（PR-RJ-0 重构） |
| `image_file_id(file_id)` | B：已知 id | 非空 |
| `file_file_id(file_id, filename?)` | B：已知 id | 非空 |

**降级规则**：若模型能力 `vision/files=false`，wire 不会硬发非法 JSON，而是统一退化为占位文本（共享 `degrade_placeholder(part)`）；因此“能力门控”仍在 provider 内收口，而不是由上层删 part。

**Files 上传管理**：详见 **[§6.5.4](#654-files-上传管理)** 与专文 [`llm-files-upload-manager.md`](llm-files-upload-manager.md)。核心原则是：**上传/删除是厂商适配层，消息 JSON 是 wire 家族职责**。

#### 6.5.4 Files 上传管理

**任务与专文**：[`docs/agents/TASK_BOARD_002/tasks/T2-P0-015.md`](../agents/TASK_BOARD_002/tasks/T2-P0-015.md)；架构冻结版 [`docs/architecture/llm-files-upload-manager.md`](llm-files-upload-manager.md)。

**目标摘要**：在 A（inline）/ B（`file_id`）两条通道不变的前提下，把“怎么上传/删除文件”从 wire 里抽走，形成 **`FilesApiAdapter`**：

- `upload(...)` / `delete(...)`：厂商存储差异
- `reference_token(file_id)`：给 wire 一个最小提示（OpenAI / Anthropic 原样；Moonshot 变 `ms://...`）
- `image_ref_slot()`：告诉 wire 图片引用该落 `file_id` 还是 URL 槽（Moonshot 用 URL 槽）

于是：

- **`OpenAiFilesRuntime`** 继续负责缓存/去重/清理，但现在持有 `Arc<dyn FilesApiAdapter>`
- **`LlmProvider::files_adapter()`** 代替旧的 OpenAI 专用 files client 能力门闸
- **OpenAI / Moonshot / Anthropic** 各自实现上传端点、鉴权、purpose 映射与删除
- **具体消息 JSON 仍由 wire 家族拼装**，避免把 Responses / Completions / Anthropic 三种形状塞回适配层

这层拆分解决了两个过去绑死的问题：一是 **Moonshot `ms://`** 与 **Anthropic beta header** 不再污染通用消息层；二是同一个上传适配器可以同时服务 **Responses** 和 **Completions**，但两条 wire 仍能各自生成不同 JSON。

**inline vs upload 默认决策树**（与专文 §3.3 一致，实现可配置）：

| 条件 | 通道 |
|------|------|
| 小附件（默认 **< 1 MiB** 图片等） | A：`image_b64` / `file_b64` |
| **1–10 MiB** 或需多轮复用 | B：`POST /v1/files` → `file_id` + cache |
| **> 10 MiB** 或超出 inline helper 上限 | **必须** upload；仍超 OpenAI 官方文件上限则结构化拒绝 |

**竞品结论（索引）**：专文 §2 记录五仓 **负向检索**结论——无主路径将 `/v1/files` 作为 Responses **输入**链路的可抄实现；本实现属 **自研增量**。

### 6.6 Completions / Anthropic 多模态（现状）

**Chat Completions 路径**（`OpenAiProvider`）现已支持多模态 `user` 消息：

- inline 图片 → `image_url.url = data:...`
- inline 文件/PDF → `file.file_data = base64`
- uploaded 图片/文件 → wire 读取 adapter 提示，自行拼 `file_id` 或 Moonshot `ms://...`

**Anthropic Messages 路径**现也已打通：

- inline 图片 → `image.source = { type: "base64", ... }`
- uploaded 图片 → `image.source = { type: "file", file_id }`
- inline PDF/文件 → `document.source = { type: "base64", ... }`
- uploaded PDF/文件 → `document.source = { type: "file", file_id }`

换句话说，当前不再是“只有 Responses 能吃附件”。真正的边界变成：**哪条 wire 家族负责把统一的 `Parts` 翻译成那家 API 认得的 JSON**。

---

## 7. 关联文档

| 文档 | 用途 |
|------|------|
| [`docs/reports/multi-agent-openai-api-integration.md`](../reports/multi-agent-openai-api-integration.md) | 五仓完整对照、ASCII、mermaid、修订记录 |
| [**OpenAI / DeepSeek 推理续传架构方案**](llm-openai-deepseek-reasoning-continuity.md) | cross-turn reasoning continuity、共享 transcript、provider replay / downgrade 规则 |
| [**上下文管理**](context-management.md) | token 预算、Compaction、`compaction_model` 语义 |
| [**Agent Loop**](agent-loop.md) | 主循环、工具、容错 |
| [**宿主核心层**](host-core-layer.md) | LLM 在分层中的位置 |
| [**Architecture.md**](../openspec/specs/Architecture.md) | 总目录入口 |
| [**Files API 上传管理**](llm-files-upload-manager.md) | `FilesApiAdapter`、缓存、生命周期、OpenAI/Moonshot/Anthropic 上传差异 |

---

## 8. 修订记录

| 日期 | 说明 |
|------|------|
| 2026-05-04 | 初稿：按 `tools/read.md` 主体结构（§1 背景与对标 · §2 本项目 · §3 协议 · §4 OGM · §5 时序 · §6 演进 · §7 关联）从 `multi-agent-openai-api-integration.md` 收敛为 openspec 架构 spec |
| 2026-05-04 | **§6 重组**：冻结 **岔路 A**；新增 **§6.5** OpenAI Responses 接入——pi_agent_rust / openclaw / hermes **对标文件锚点** + tomcat **实施清单**；§4 OGM 与 §2/§3 交叉引用更新 |
| 2026-05-04 | §6.5.2 增补：**常态增量 = 每 API 一个 `XxxProvider`**；区分横切首接成本 vs 每 Provider 最小集；下表定位为 Responses **首接核对单** |
| 2026-05-04 | §6.5.2 增量类型表：**「包含什么」改为白话列**（「白话（你在干什么）」） |
| 2026-05-04 | §6.5.2：横切行改为 **provider 字符串 + 注册表**；新增 **`LlmConfig` 与 TOML** 小节；核对单 **配置/工厂** 两行改为 **不改中央结构体、registry 解析** |
| 2026-05-04 | §6.5.1 表第三列：「职责」改为 **「白话（这篇代码能帮到你什么）」** 并重写各格 |
| 2026-05-04 | 新增 **§3.2.1**：注册表 id 选 **Provider**；**同一 `ChatRequest`** 由不同实现翻译 wire；何时才扩展类型 |
| 2026-05-05 | **实施落档**：`registry.rs` + **`resolve_llm`**；**`OpenAiResponsesProvider`**（`/v1/responses`）；默认 **`provider = openai-responses`**；更新 **§2 / §4 OGM / §5.1**；**§1.3** 横向表「路由 / OpenAI」行 |
| 2026-05-05 | **多模态 wire（T2-P0-012）**：`ChatMessageContentPart` 升级为 `#[serde(tag="type")]` 三态枚举（`InputText` / `InputImage` / `InputFile`）；`OpenAiResponsesProvider` 翻译 `input_image` / `input_file`（inline base64 + 已知 file_id 双通道）；`OpenAiProvider` 入口结构化拒绝非文本 part；**Files 上传管理** 拆出至 **T2-P0-015**；更新 **§1.3 / §2.1 / §6.5.3 / §6.6** |
| 2026-05-09 | **§6.5.4** 落地：OpenAI Files 上传子系统索引 + 关联专文 [`llm-files-upload-manager.md`](llm-files-upload-manager.md)；任务号 **T2-P0-013→T2-P0-015** 勘误（013 为拖拽/CWD 任务）；**§7** 关联表增一行 |
| 2026-05-10 | **§6.5.4 摘要**：TOML **仅** `[llm.files] expires_after_seconds`；是否走 `/v1/files` 由 **`LlmProvider` 实现声明支持 OpenAI Files API** 决定（对齐专文 **§9 / §4.1 U11–U12**） |
| 2026-05-31 | 新增 continuity 边界说明与回链：OpenAI / DeepSeek 跨 turn reasoning continuity 另立 [**`llm-openai-deepseek-reasoning-continuity.md`**](llm-openai-deepseek-reasoning-continuity.md)，避免本方案同时承担 provider 骨架与续传细则两类职责 |
