# LLM 统一接入模块说明 (LLM Module)

## 1. 概述 (Overview)

- **职责**：为宿主 API 与 chat 提供统一的 LLM 能力：`ModelCatalog`、`DefaultLlmResolver`、`admin.rs` 模型管理中枢、`LlmProvider` Trait、OpenAI Chat Completions / Responses / Anthropic Messages 适配器、流式/非流式调用、限流与指数退避重试、Token 统计、会话级模型切换。
- **所在层级**：宿主核心能力层（`src/core/llm`），依赖基础设施层（`AppConfig` / `LlmConfig` / `LlmRuntimeConfig` / `AppError`）。
- **核心文件**：
  - `src/core/llm/builtin_models.toml` — 内嵌预置模型事实源；`tomcat init` 直接从这里释放 seed
  - `src/core/llm/catalog.rs` — `ModelEntry` / `ModelCatalog`；解析内嵌预置并合并 `models.toml`
  - `src/core/llm/resolver.rs` — `DefaultLlmResolver` / `ResolvedCall`；按 scene + session override 选模型
  - `src/core/llm/auth.rs` — 凭证解析；优先 `api_key_env`，否则推断 `<PROVIDER>_API_KEY`
  - `src/core/llm/admin.rs` — 模型管理共享中枢；`upsert/remove/set_key/list_keys/default`
  - `src/core/llm/endpoint.rs` — path-aware endpoint 拼接；bare host 自动补 `/v1`，显式路径保留原样
  - `src/core/llm/registry.rs` — `entry.api` → `Arc<dyn LlmProvider>`
  - `src/core/llm/openai.rs` — `OpenAiProvider`（`POST …/v1/chat/completions`）
  - `src/core/llm/openai_responses/mod.rs` — `OpenAiResponsesProvider`（`POST …/v1/responses`）
  - `src/core/llm/anthropic/mod.rs` — `AnthropicProvider`（`POST …/v1/messages`）

### 1.1 当前配置模型

现在分成两层：

1. **`[llm]`（`tomcat.config.toml`）**：只负责“选哪个模型”与全局运行时旋钮。
2. **`models.toml`**：负责每个模型怎么连（`api` / `provider` / `base_url` / `api_key_env` / `model_name` / `capabilities`）。

这意味着旧的 `[llm].provider` / `[llm].api_base` / `[llm].api_key_env` 已经删除；如果用户继续写，会直接得到迁移错误，并提示改到 `models.toml`。

### 1.2 Provider 注册表（按 `api` 路由）

注册表现在按 **`ModelEntry.api`** 选 wire adapter，而不是按厂商名选：

| `api` | 实现 | HTTP |
|------|------|------|
| `openai-responses` | `OpenAiResponsesProvider` | `POST {base}/v1/responses` |
| `openai` | `OpenAiProvider` | `POST {base}/v1/chat/completions` |
| `anthropic-messages` | `AnthropicProvider` | `POST {base}/v1/messages` |

`provider` 现在只表示**逻辑厂商**，用于凭证推断、展示和审计；例如 `provider = "deepseek"` 仍然可以配 `api = "openai"`。

### 1.2.1 Path-aware endpoint 规则

- **bare host**：`https://api.openai.com` + `responses` → `https://api.openai.com/v1/responses`
- **显式 provider 路径**：`https://open.bigmodel.cn/api/paas/v4` + `chat/completions` → `https://open.bigmodel.cn/api/paas/v4/chat/completions`
- **Anthropic**：`https://api.anthropic.com/v1` + `messages` → `https://api.anthropic.com/v1/messages`

说人话：如果 `base_url` 只是主机，就按历史兼容自动补 `/v1`；如果用户已经明确写了路径，就别再帮倒忙多拼一层。

### 1.3 `models.toml` 与内置模型

- **常用预置的运行时事实源只有一份内嵌 `builtin_models.toml`**：OpenAI（`gpt-5.2` / `gpt-5.4` / `gpt-5.5` / `gpt-5.6`）、DeepSeek（`deepseek-v4-pro` / `deepseek-v4-flash` / `utility-flash`）、MiMo（`mimo-v2.5-pro`）、GLM（`glm-5.2`）、Kimi（`kimi-k2.7-code`）、Anthropic Messages（`claude-opus-4-8` / `4-7` / `4-6`）
- **`tomcat init` 会把这份内嵌预置原样 seed 到 `models.toml`**：这样用户能直接看 / 改 / 删，但不会再维护第二份手写模型清单
- **同 id 覆盖内置，新 id 直接新增**
- **新增用户条目必须显式写 `api` 和 `provider`**；不再按模型家族猜协议/厂商/能力

`model_name` 用来解决“本地 id”和“上游真名”并存的问题。例如公司网关场景可写：

```toml
[[models]]
id = "gpt-5.4_gateway"
model_name = "gpt-5.4"
api = "openai-responses"
provider = "openai-gateway"
base_url = "https://gateway.example.com"
```

这样本地可以同时保留 `gpt-5.4` 与 `gpt-5.4_gateway` 两条模型，但真正发给上游的 `model` 仍然是 `gpt-5.4`。

### 1.4 模型管理入口

- **共享后端**：`core/llm/admin.rs` 是唯一写盘入口，统一负责 `models.toml` / `.env`、文件锁、原子写、权限与热刷新。
- **CLI 门面**：`tomcat model add/list/remove/key/default` 全部复用这套共享逻辑。
- **serve 门面**：`list_models` / `upsert_model` / `remove_model` / `set_provider_key` / `list_provider_keys` 走同一个中枢。
- **安全边界**：协议与状态里只暴露 `envName` / `keyPresent`，从不回显明文 key。

### 1.2 LLM 调用路径（ASCII）

```text
  ChatRequest (model / messages / model_override)
            |
            v
     +------+------+
     | resolve(model) |
     | Semaphore 限流   |
     | 重试 + fallback base |
     +------+------+
            |
     +------v------+       +------------------+
     | chat()      |       | chat_stream()     |
     | ChatResponse|       | Stream<StreamEvent> |
     +-------------+       +-------------------+
```

- **配置来源**：`AppConfig.llm` 负责默认模型与全局运行时旋钮；`ModelCatalog` 负责模型条目事实源。
- **数据面总览**：与 [src 模块索引](../../README.md)「图 2」中 `LlmProvider` 与 `SessionManager` 的衔接关系一致。

## 2. 使用方式

- **聊天入口**：优先用 `DefaultLlmResolver::resolve(scene, session_override)`，拿到 `ResolvedCall { provider_impl, model, ... }`。上层把 `ResolvedCall.model` 作为 wire `model` 传给 provider。
- **直接构造 provider（测试 / 工具）**：`OpenAiProvider::new(entry, runtime, credential)` 或 `OpenAiResponsesProvider::new(entry, runtime, credential)`。其中：
  - `entry: &ModelEntry` 提供 `api` / `provider` / `base_url` / `model_name`
  - `runtime: &LlmRuntimeConfig` 提供重试、超时、proxy、files、continuity 等全局旋钮
  - `credential: &Credential` 提供已经解析好的 key 值；provider 自己不再读 env
- **Files 上传配置**：`[llm.files] expires_after_seconds` 控制上传时 `expires_after.seconds`（默认 `86400`，`0` 表示不传该字段）；环境变量覆盖键为 `TOMCAT__LLM__FILES__EXPIRES_AFTER_SECONDS`。
- **Continuity 默认值**：`[llm.reasoning_continuity] enabled` 默认就是 `true`；只有想显式退回“只带可见历史、不做 opaque replay”的旧行为时才需要关。
- **chat-completions `reasoning_content` continuity 语义（数据驱动）**：`reasoning_content` 的 **capture** 与 **replay** 明确解耦，且**不再按厂商名硬编码**。「哪个模型走 `reasoning_content` 续传」由 `replay_policy.rs` 的数据表 `CHAT_COMPLETIONS_CONTINUITY_RULES` 决定（当前含 `deepseek-v4` 与 `mimo-v2.5-pro` 两行，共用同一条逻辑）；新增同类模型 = 加一行数据，`maybe_snapshot` / `is_compatible` / `transport_messages` 等 continuity 各道门只读 `ProviderCompatProfile` 字段（`capture_mode` / `api_family` / `provider`+`model_family`），无需修改。只要响应里抓到 snapshot 就照常写进 transcript；后续**同 profile**（provider + model_family 一致）请求会优先回放兼容的 `reasoning_content`，`same_profile` 比对保证 DeepSeek / MiMo 互不串档。`had_tool_call` / `replay_requirement` 仍保留在 transcript metadata 里，用于审计和表达 tool turn 的 replay 强约束。
  > 架构约束说明：provider 由 `LlmConfig` 装配（registry §6.5.2「稳定 schema」），运行期只拿到 model 字符串、拿不到 catalog 条目，故 continuity 的运行期事实源是上面这张按 model family 索引的数据表；`models.toml` 是面向用户的声明层。两者对内置厂商（deepseek/mimo）保持一致。
- **账本全量 vs 出站精简**：transcript 是 continuity 的**全量账本**——hydrate（`chat_message_from_entry` 整条反序列化）与 `/model` 切换（`switch_current_model` 只改 `model_override` + 落 `model_change` 事件）都**不会**清洗历史里的 `reasoning_continuation` / `continuity`。真正的“精简”只发生在**出站 wire 克隆**上，绝不回写主账本。
- **可 replay 窗口（出站收敛）**：wire builder 出站时按 `ReplayWindow` 收敛——只有**最新 assistant turn** 与**当前 turn**（最后一条真实 user 之后的消息）内的 continuity 才参与 opaque/文本 replay；更早的历史轮次一律 `StripOpaque`（只留可见内容、丢弃隐藏 blob、**不转文本**、**不告警**）。这样既保住当前轮的高保真续传，又从根上避免对整段历史逐条降级判定与刷屏。
- **Replay warning 语义**：逐消息 warn 已改为**每请求至多一条汇总告警**（`ReplayDowngradeReport::emit`），且只在窗口内出现「真正降级失败」时触发：**A** 同 profile 却没能 `KeepOpaque`（任何非 keep 动作都算异常 → `SameProfileIncompatible`）；**B** 跨 profile 且连 fallback 文本都救不回、落到 `StripOpaque`（continuity 彻底丢失）。跨 profile 的 `ConvertToText` 属设计内的优雅降级，**不告警**；窗口外老历史的静默 strip 仅计数、**从不告警**。不再使用进程内“问题指纹”缓存压重复 warning。
- **结构化错误模型**：provider 不再把 `503/429/400` 等语义只塞进一段字符串；统一构造 `LlmError { provider, stage, http_status, summary, source }`，并由 `infra/error/llm.rs` 作为 `is_retryable_llm_error` / `llm_connect_or_network` / `is_context_overflow` 的单一事实来源。
- **非流式调用**：`provider.chat(request).await`
- **流式调用**：`provider.chat_stream(request).await`
- **base fallback**：当对主 `base_url` 请求发生连接/网络错误且配置了 `api_base_fallback` 时，自动用 fallback URL 重试一次。
- **Token 统计**：`ChatResponse.usage` / `StreamEvent::Usage` 提供单次 usage；会话级汇总由调用方使用 `SessionTokenUsage` 累加，并写入 SessionEntry（当 003 可用时）。

## 3. 会话级模型配置

- `ChatRequest.model_override: Option<String>` 与 SessionEntry.model_override 约定一致；为 None 时使用请求的 model 字段（通常由上层从 `ResolvedCall.model` 或 SessionEntry 填入）。
- `SessionManager::switch_current_model(provider, model_id)` 会同时更新当前 session 的 `model_override`，并落一条 `model_change` transcript 事件；当前仅作为最小切换链路与测试/会话审计入口，**不是**完整多 LLM 产品化方案。

## 3.5 多模态 parts（图片 / PDF 附件）

`ChatMessageContentPart` 是 `#[serde(tag = "type", rename_all = "snake_case")]` 三态枚举：`InputText` / `InputImage` / `InputFile`，对齐 OpenAI Responses 的 `input_text` / `input_image` / `input_file` content part 形状。**默认 provider `openai-responses` 完整支持**；`provider = "openai"`（Completions）遇到非文本 part 立即结构化拒绝并把诊断指向 `provider=openai-responses`。

### 通道与 helper

| 通道 | helper | 校验 |
|------|--------|------|
| **A · inline base64**（同一请求内附带字节） | `ChatMessageContentPart::image_b64(mime, &Path)` | metadata 字节 `<= IMAGE_MAX_BYTES` (4.5 MB) + MIME ∈ {png,jpeg,gif,webp}；helper 内部 `read + base64` |
| | `ChatMessageContentPart::file_b64(filename, mime, &Path)` | metadata 字节 `<= FILE_MAX_BYTES` (25 MB)；helper 内部 `read + base64` |
| **B · Files 上传后 `file_id`** | `ChatMessageContentPart::image_upload(client, mime, bytes, filename)` | provider 必须支持 OpenAI Files API；失败可回退 A 通道 |
| | `ChatMessageContentPart::file_upload(client, filename, mime, bytes)` | provider 必须支持 OpenAI Files API；失败可回退 A 通道 |
| **B · 已知 file_id 透传**（已经从 OpenAI Files API 拿到 id） | `ChatMessageContentPart::image_file_id(id)` | 非空 |
| | `ChatMessageContentPart::file_file_id(id, filename?)` | 非空 |

> **PR-RJ-0 重构**：`image_b64` / `file_b64` 已统一为 `(mime, &Path)` 签名，让 helper 自己读盘 + base64，避免「`read` 工具读一遍 + LLM 客户端再读一遍」的重复 IO 与重复校验。已知 `file_id` 通道（B）保持不变。

> **T2-P0-015 已落地**：`OpenAiFilesClient`（`upload/get/delete/list`）+ `ChatMessageContentPart::{image_upload,file_upload}` + 会话级 cache/cleanup 编排；`file_id` 翻译优先级仍由 `OpenAiResponsesProvider::part_to_responses_value` 保持不变。

### 最小调用示例

```rust
use std::sync::Arc;
use tomcat::{
    AppConfig, ChatMessage, ChatMessageContentPart, ChatRequest, DefaultLlmResolver, LlmResolver,
    LlmScene, ModelCatalog,
};

let cfg = AppConfig::default();
let catalog = Arc::new(ModelCatalog::load(&cfg)?);
let resolver = DefaultLlmResolver::new(cfg.clone(), catalog);
let resolved = resolver.resolve(LlmScene::Main, None)?;
let provider = resolved.provider_impl;

// A 通道：inline 图片（PR-RJ-0：直接传路径，helper 自动读盘 + base64）
let parts = vec![
    ChatMessageContentPart::text("Describe this image:"),
    ChatMessageContentPart::image_b64("image/png", "photo.png")?,
];

// B 通道：已知 file_id
// let parts = vec![
//     ChatMessageContentPart::text("Summarize this PDF:"),
//     ChatMessageContentPart::file_file_id("file-abc", Some("notes.pdf".to_string()))?,
// ];

let req = ChatRequest {
    messages: vec![ChatMessage::user_with_parts(parts)],
    model: resolved.model.clone(),
    max_tokens: Some(96),
    ..Default::default()
};
let resp = provider.chat(req).await?;
```

### 角色与 wire

`OpenAiResponsesProvider::part_to_responses_value` 翻译规则：
- `InputText` → `{type: "input_text", text}`
- `InputImage` → `{type: "input_image", image_url: "data:..."}`（A 通道）或 `{type: "input_image", file_id}`（B 通道）；`file_id` 优先
- `InputFile` → `{type: "input_file", file_data: "data:..."}`（A 通道）或 `{type: "input_file", file_id}`（B 通道）

仅 `User` 角色把非文本 part 透传 Responses；`System` / `Assistant` / `Tool` 角色出现非文本 part 时 **`tracing::warn!` 一次并丢弃非文本部分**（保留 wire 兼容）。

## 4. 扩展

- **新增其它 OpenAI 形后端**：默认先评估能否直接复用现有 `api = "openai"` 或 `api = "openai-responses"`，把差异收进 `models.toml` 条目即可。
- **新增其它厂商**：同上；优先保持 `LlmConfig` 只存“选哪个模型”和全局旋钮，不把单模型连接字段重新塞回主配置。
