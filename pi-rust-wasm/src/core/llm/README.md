# LLM 统一接入模块说明 (LLM Module)

## 1. 概述 (Overview)

- **职责**：为宿主 API 与 chat 提供统一的 LLM 能力：`LlmProvider` Trait、OpenAI Chat Completions / Responses 适配器、流式/非流式调用、限流与指数退避重试、Token 统计、会话级模型配置（model_override）。
- **所在层级**：宿主核心能力层（`src/core/llm`），依赖基础设施层（AppError、LlmConfig、load_config 等）。
- **核心文件**：
  - `src/core/mod.rs` — core 层聚合，re-export llm 对外类型
  - `src/core/llm/mod.rs` — LLM 模块聚合与 re-export（含 **`resolve_llm`**）
  - `src/core/llm/registry.rs` — **`PROVIDERS` 表**：`[llm] provider` 字符串 → `Arc<dyn LlmProvider>`
  - `src/core/llm/types.rs` — ChatMessage、ChatRequest、ChatResponse、StreamEvent、TokenUsage
  - `src/core/llm/provider.rs` — LlmProvider Trait 定义
  - `src/core/llm/openai.rs` — **OpenAiProvider**（`POST …/v1/chat/completions`）
  - `src/core/llm/openai_responses.rs` — **OpenAiResponsesProvider**（`POST …/v1/responses`）
  - `src/core/llm/token_usage.rs` — SessionTokenUsage 会话级汇总结构

### 1.1 Provider 注册表（`provider` 字符串）

运行时根据 **`LlmConfig.provider`** 选型（默认 **`openai-responses`**）：

| `provider` id | 实现 | HTTP |
|---------------|------|------|
| **`openai-responses`**（默认） | `OpenAiResponsesProvider` | `POST {base}/v1/responses` |
| **`openai`** | `OpenAiProvider` | `POST {base}/v1/chat/completions` |

装配入口：**`crate::core::llm::resolve_llm(&config.llm)`**（例如 `ChatContext::from_config`）。未知 id 返回 **`AppError::Config`** 并列出已注册 id。

详见 openspec **[`architecture/llm-multiprovider-integration.md`](../../../docs/architecture/llm-multiprovider-integration.md)**。

### 1.2 LLM 调用路径（ASCII）

```text
  ChatRequest (model / messages / model_override)
            |
            v
     +------+------+
     | resolve_llm   |
     | Semaphore 限流   |
     | 重试 + fallback base |
     +------+------+
            |
     +------v------+       +------------------+
     | chat()      |       | chat_stream()     |
     | ChatResponse|       | Stream<StreamEvent> |
     +-------------+       +-------------------+
```

- **配置来源**：`LlmConfig` 来自 `AppConfig`（见 [infra/README.md](../../infra/README.md) 中配置与代理说明）。
- **数据面总览**：与 [src 模块索引](../../README.md)「图 2」中 `LlmProvider` 与 `SessionManager` 的衔接关系一致。

## 2. 使用方式

- **选型**：在聊天入口使用 **`resolve_llm(&app_config.llm)?`** 得到 **`Arc<dyn LlmProvider>`**，不要手写 `OpenAiProvider::new` / `OpenAiResponsesProvider::new`（除非是测试或直接构造单一后端）。
- **构造具体实现（测试 / 工具）**：`OpenAiProvider::new(&config)` 或 `OpenAiResponsesProvider::new(&config)`，其中 `config` 为 `LlmConfig`（含 api_base、api_key_env、default_model、max_concurrent_requests、retry_count、stream_timeout_sec；可选 **proxy** 显式代理、**api_base_fallback** 自动降级用备用 base）。api_key 从 `api_key_env` 指定环境变量读取，未设置则返回错误。若配置 `proxy`，所有 LLM 请求经该代理；未配置时 reqwest 仍尊重环境变量 `HTTPS_PROXY`/`HTTP_PROXY`。代理与降级 URL 可通过配置文件（见项目根 **pi.config.toml.example**）或环境变量 `PI_WASM__LLM__PROXY`、`PI_WASM__LLM__API_BASE_FALLBACK` 配置，详见 [infra/README.md](../../infra/README.md) 中「代理与降级 URL 的配置方式」。
- **非流式调用**：`provider.chat(request).await`，请求中 `model_override` 优先于 `request.model` 选模型；支持限流（Semaphore）与可重试错误的指数退避重试；当对主 api_base 请求发生连接/网络错误且配置了 `api_base_fallback` 时，自动用 fallback URL 重试一次。
- **流式调用**：`provider.chat_stream(request).await` 返回 `Box<dyn Stream<Item = Result<StreamEvent, AppError>>>`，消费端可通过 drop 提前结束以释放连接；同样支持主 base 不通时自动用 `api_base_fallback` 重试。
- **Token 统计**：`ChatResponse.usage` / `StreamEvent::Usage` 提供单次 usage；会话级汇总由调用方使用 `SessionTokenUsage` 累加，并写入 SessionEntry（当 003 可用时）。

## 3. 会话级模型配置

- `ChatRequest.model_override: Option<String>` 与 SessionEntry.model_override 约定一致；为 None 时使用请求的 model 字段（通常由上层从 LlmConfig.default_model 或 SessionEntry 填入）。

## 3.5 多模态 parts（图片 / PDF 附件）

`ChatMessageContentPart` 是 `#[serde(tag = "type", rename_all = "snake_case")]` 三态枚举：`InputText` / `InputImage` / `InputFile`，对齐 OpenAI Responses 的 `input_text` / `input_image` / `input_file` content part 形状。**默认 provider `openai-responses` 完整支持**；`provider = "openai"`（Completions）遇到非文本 part 立即结构化拒绝并把诊断指向 `provider=openai-responses`。

### 通道与 helper

| 通道 | helper | 校验 |
|------|--------|------|
| **A · inline base64**（同一请求内附带字节） | `ChatMessageContentPart::image_b64(mime, &Path)` | metadata 字节 `<= IMAGE_MAX_BYTES` (4.5 MB) + MIME ∈ {png,jpeg,gif,webp}；helper 内部 `read + base64` |
| | `ChatMessageContentPart::file_b64(filename, mime, &Path)` | metadata 字节 `<= FILE_MAX_BYTES` (25 MB)；helper 内部 `read + base64` |
| **B · 已知 file_id 透传**（已经从 OpenAI Files API 拿到 id） | `ChatMessageContentPart::image_file_id(id)` | 非空 |
| | `ChatMessageContentPart::file_file_id(id, filename?)` | 非空 |

> **PR-RJ-0 重构**：`image_b64` / `file_b64` 已统一为 `(mime, &Path)` 签名，让 helper 自己读盘 + base64，避免「`read` 工具读一遍 + LLM 客户端再读一遍」的重复 IO 与重复校验。已知 `file_id` 通道（B）保持不变。

> **「读字节 → 上传 → 拿 file_id」一站式 helper 不在本期**：归到独立任务 **T2-P0-015 | llm-files-upload-manager**（multipart `POST /v1/files` 客户端 + 生命周期 + reuse cache + 异步 helper），见 [`agents/TASK_BOARD_002/tasks/T2-P0-015.md`](../../../agents/TASK_BOARD_002/tasks/T2-P0-015.md)。

### 最小调用示例

```rust
use pi_wasm::{resolve_llm, ChatMessage, ChatMessageContentPart, ChatRequest, LlmConfig};

let cfg = LlmConfig { provider: "openai-responses".to_string(), ..LlmConfig::default() };
let provider = resolve_llm(&cfg)?;

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
    model: cfg.default_model.clone(),
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

- **新增其它 OpenAI 形后端**：实现 `LlmProvider`，在 **`registry.rs`** 的 **`PROVIDERS`** 表追加一行 `(id, ctor)`。
- **新增其它厂商**：同上；保持 `LlmConfig` 横切字段不无限膨胀（见 architecture spec §6.5.2）。
