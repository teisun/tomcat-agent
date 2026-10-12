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

### 1.5 能力、默认值与个人选择

模型目录描述的是**这个模型能做什么**，个人偏好描述的是**这次要怎么用它**。两者不能混在 `models.toml`，否则一个人的 Context 或 Reasoning 选择会改写所有人的模型定义。

```text
builtin_models.toml / models.toml             model-thinking.json
(能力与默认值，团队可审阅)                    (个人选择，可持久化)

context_window + context_window_options       { reasoning, contextWindow }
supported_reasoning_levels                    └─ 按 catalog model id 存储
             │                                             │
             └──────── ModelCatalog + ModelPrefsStore ─────┘
                                      │
                                      v
                         EffectiveModelLimits / serve list_models
```

- `context_window` 是默认上限；`context_window_options` 是可选档位，空数组表示没有可选档位。选项必须升序、唯一，并且默认值必须在选项中。
- `supported_reasoning_levels` 同样是能力数据。`reasoning` 与 `contextWindow` 是独立选择：换 Reasoning 不会偷偷改 Context，反之亦然。
- 用户档位失效（例如模型更新后删掉一个档位）会安全回落到模型默认 Context；内置 TOML 的无效档位是发布错误，加载会失败，避免把错误预置静默带给用户。
- 偏好文件仍叫 `model-thinking.json`。真实模型键继续写入旧格式的 Reasoning 字符串；Context 选择写在同一个 `models` 映射中的保留 `__tomcat_context_window__:<model>:<tokens>` 键，值固定为 `"off"`。因此新代码可读旧值，旧二进制也只会把保留键当成一个未使用、关闭推理的模型，不会因对象值而重置整个文件。
- Chat 可用 `/effort <level>` 选择 Reasoning，用 `/context <tokens>` 选择当前模型声明的 Context 档位；两条命令都只写个人偏好。serve/UI 则使用 `set_thinking_level` 与 `set_context_window`，随后以 `list_models` 回显实际选中的值。

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
- **chat-completions `reasoning_content` continuity 语义（数据驱动）**：`reasoning_content` 的 **capture** 与 **replay** 明确解耦，且**不再按厂商名硬编码**。「哪个模型走 `reasoning_content` 续传」由 `replay_policy.rs` 的数据表 `CHAT_COMPLETIONS_CONTINUITY_RULES` 决定（当前含 `deepseek-v4`、`mimo-v2.5-pro`、`kimi-k3`，共用同一条逻辑）；新增同类模型 = 加一行数据，`maybe_snapshot` / `is_compatible` / `transport_messages` 等 continuity 各道门只读 `ProviderCompatProfile` 字段（`capture_mode` / `api_family` / `provider`+`model_family`），无需修改。只要响应里抓到 snapshot 就照常写进 transcript；后续**同 profile**（provider + model_family 一致）请求会优先回放兼容的 `reasoning_content`，`same_profile` 比对保证 DeepSeek / MiMo 互不串档。`had_tool_call` / `replay_requirement` 仍保留在 transcript metadata 里，用于审计和表达 tool turn 的 replay 强约束。
  > 架构约束说明：provider 由 `LlmConfig` 装配（registry §6.5.2「稳定 schema」），运行期只拿到 model 字符串、拿不到 catalog 条目，故 continuity 的运行期事实源是上面这张按 model family 索引的数据表；`models.toml` 是面向用户的声明层。两者对内置厂商（deepseek/mimo）保持一致。
- **账本全量 vs 出站精简**：transcript 是 continuity 的**全量账本**——hydrate（`chat_message_from_entry` 整条反序列化）与 `/model` 切换（`switch_current_model` 只改 `model_override` + 落 `model_change` 事件）都**不会**清洗历史里的 `reasoning_continuation` / `continuity`。真正的“精简”只发生在**出站 wire 克隆**上，绝不回写主账本。
- **可 replay 窗口（出站收敛）**：wire builder 出站时按 `ReplayWindow` 收敛——只有**当前 turn**（最后一条真实 user 之后的消息）内的 continuity 才参与 opaque replay；包括上一轮最后一条 assistant 在内的更早历史一律 `StripOpaque`（只留可见内容、丢弃隐藏 blob、**绝不转成正文**、**不告警**）。这样既保住当前轮的高保真续传，又从根上避免对整段历史逐条降级判定与刷屏。
  - Kimi K3 例外由同一策略表的 `preserve_history` 标记决定：按[官方 Preserved Thinking 契约](https://platform.kimi.ai/docs/guide/use-thinking-models)，同 profile 的历史 `reasoning_content` 也需保留；仍严格禁止跨模型串用。DeepSeek/MiMo 的当前轮窗口规则不变。临时拆分图片 user 不参与内部窗口和逻辑轮计算。
- **Replay warning 语义**：逐消息 warn 已改为**每请求至多一条汇总告警**（`ReplayDowngradeReport::emit`），且只在窗口内出现同 profile 却没能 `KeepOpaque` 时触发（`SameProfileIncompatible`）。跨 profile 的 opaque reasoning 无法安全重放，一律静默 `StripOpaque`；窗口外老历史的静默 strip 仅计数、从不告警。不再使用进程内“问题指纹”缓存压重复 warning。
- **结构化错误模型**：provider 不再把 `503/429/400` 等语义只塞进一段字符串；统一构造 `LlmError { provider, stage, http_status, summary, source }`，并由 `infra/error/llm.rs` 作为 `is_retryable_llm_error` / `llm_connect_or_network` / `is_context_overflow` 的单一事实来源。
- **非流式调用**：`provider.chat(request).await`
- **流式调用**：`provider.chat_stream(request).await`
- **base fallback**：当对主 `base_url` 请求发生连接/网络错误且配置了 `api_base_fallback` 时，自动用 fallback URL 重试一次。
- **Token 统计**：`ChatResponse.usage` / `StreamEvent::Usage` 提供单次 usage；会话级汇总由调用方使用 `SessionTokenUsage` 累加，并写入 SessionEntry（当 003 可用时）。

## 3. 会话级模型配置

- `ChatRequest.model_override: Option<String>` 与 SessionEntry.model_override 约定一致；为 None 时使用请求的 model 字段（通常由上层从 `ResolvedCall.model` 或 SessionEntry 填入）。
- `SessionManager::switch_current_model(provider, model_id)` 会同时更新当前 session 的 `model_override`，并落一条 `model_change` transcript 事件；当前仅作为最小切换链路与测试/会话审计入口，**不是**完整多 LLM 产品化方案。

## 3.5 多模态 parts（图片 / PDF 附件）

`ChatMessageContentPart` 使用 `#[serde(tag = "type", rename_all = "snake_case")]`，包括文本、上下文引用、图片、文件及持久化图片引用。User 与 Tool 都可持有媒体；发送形状按 API 决定，非视觉/非文件模型由能力降级处理，不把图片字节伪装为文本。

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

User 媒体沿用原路径。Tool 媒体经 `tool_result_media.rs::tool_result_media_mode` 选择：Anthropic 原生 `tool_result.content[]`，Responses 原生 `function_call_output.output[]`；Chat Completions 先闭合整批 tool 结果，再追加仅用于该请求的一条 user 媒体消息。历史账本不写入该临时 user；System/Assistant 的非文本 part 仍按原规则处理。纯文本工具保持原字符串形状。

正常工具附件（read图片/PDF、MCP图片）不自动上传为file_id；内联仍遵守原大小限制，超限明确提示。file_id构造器、客户端与wire转换为兼容/显式调用保留。图片由dispatcher写入既有blob仓库，档案用input_image_ref，provider内存版内联；live事件media与历史引用均不带图片字节。L0/L1压缩只替换工具文字，不丢附件引用；GC同时标记主会话与子agent会话，缓存键分命名空间。


## 3.6 工具结果媒体探针（2026-10-08）

阶段2直接向中转站发原始 HTTP JSON，开启 SSE；使用现有小狗 fixture，不在提问或工具文本中泄漏答案。N 是工具结果内原生图片，S 是工具文本后附图片的拆分对照。以下为阶段3默认策略的实测依据；file_id上传缺口不宣称通过。

| API / 目标 | 载荷 | N 原生 | S 拆分 |
|---|---|---|---|
| Anthropic / fcodex `claude-opus-5` | inline | HTTP 200，Dog，通过 | HTTP 200，Dog，通过 |
| Responses / idatatlas `gpt-6.1-sol` | inline | HTTP 200，Dog，通过 | HTTP 200，Dog，通过 |
| Responses / fcodex `gpt-6.1-sol` | inline | HTTP 200，dog，通过 | HTTP 200，dog，通过 |
| Responses / idatatlas `gpt-6.1-sol` | file_id | **上传阶段 HTTP 404；未发模型请求** | **上传阶段 HTTP 404；未发模型请求** |
| Chat Completions / idatatlas `gpt-6.1-sol` | inline | HTTP 200，但回答 unavailable，未看见图 | HTTP 200，Dog，通过 |
| Chat Completions / Kimi `kimi-k3` | inline | HTTP 200，Dog（记录型） | HTTP 200，dog，通过 |

结论与边界：

- 已验证的 inline 路径支持 Anthropic/Responses 默认原生；Chat Completions 仍按协议约定统一拆分，不能因 Kimi 的扩展支持而把 idatatlas 上会丢图的原生形状推给所有 Chat 模型。
- Anthropic/Responses 的原生结果未出现中转站分歧，目前没有依据增加按模型的 `tool_result_media` 覆盖项。
- **file_id 仍未验证**：现成 `OpenAiFilesClient` 向当前 `https://sub2api.idatatlas.com/v1/files` 上传返回 `404 page not found`。这是上传前置条件失败，不是模型拒绝原生 output 数组；两条测试保持失败，不以 inline 成功冒充 file_id 成功。没有取得文件 id，也没有远端文件需要删除。
- 用户已确认：**保留file_id实现但实际不走上传**。B的正常工具附件将走内联：支持原生工具结果的API直接放tool_result/function_call_output，不支持则发送时拆分成user媒体；file_id客户端、构造/适配及显式探针保留。由此闸门2的所选运行路径已有依据，404实测缺口继续公开，不擅自修改用户模型的`files`声明；也不引入每请求失败探测/重复调用机制。

证据：`tests/tool_result_media_real_llm_tests.rs`；`task:1791464642472-63828c`（12例，10通过、2条file_id上传失败）；原始矩阵 `.agents/acceptance/tool-result-media-probe-matrix.jsonl`。Chat N 是记录型，测试绿不表示所有原生形状受支持，须看 `saw_image`。

复跑：`cargo test --test tool_result_media_real_llm_tests -- --ignored --nocapture --test-threads=1`。目标可用 `TOMCAT_MEDIA_PROBE_<RELAY>_<ANTHROPIC|RESPONSES|CHAT>_{MODEL,BASE_URL,KEY_ENV}` 覆盖；`TOMCAT_MEDIA_PROBE_OUTPUT` 指定矩阵 JSONL。缺 key 会明确打印 SKIP；不得把 SKIP 计入已验证能力。上传成功的用例会在请求结束后调用 delete 清理，即使模型请求失败也一样。

## 4. 扩展

- **新增其它 OpenAI 形后端**：默认先评估能否直接复用现有 `api = "openai"` 或 `api = "openai-responses"`，把差异收进 `models.toml` 条目即可。
- **新增其它厂商**：同上；优先保持 `LlmConfig` 只存“选哪个模型”和全局旋钮，不把单模型连接字段重新塞回主配置。
