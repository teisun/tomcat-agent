# 多 LLM 产品化技术方案（架构 spec）

> **实现状态更新（2026-06）**
>
> 本文里原先很多“计划新增”已经落地，当前实现以代码为准：
>
> - `LlmConfig` 现在只保留“选哪个模型”和全局运行时旋钮；旧 `[llm].provider` / `[llm].api_base` / `[llm].api_key_env` 已删除，继续使用会直接报迁移错误。
> - `ModelEntry` 现在显式包含 `api`、`provider`、`base_url`、`api_key_env`、`model_name`、`capabilities`。
> - `LlmRuntimeConfig` 已从 `LlmConfig` 拆出，provider 构造统一走 `new(entry, runtime, credential)`。
> - provider registry 现在按 `entry.api` 路由；`provider` 只表示逻辑厂商，用于凭证推断、展示、审计。
> - 当前内置模型只剩 `gpt-5.4` 与 `deepseek-v4-pro`；`gpt-5.2`、`deepseek-v4-flash`、`mimo-v2.5-pro` 由 `tomcat init` 补进 `models.toml`。
> - OpenAI 直连与 LiteLLM 网关并存的推荐范式是：保留 `gpt-5.4`，另加 `gpt-5.4_litellm-sunmi`，并用 `model_name = "gpt-5.4"` 把本地 id 与上游真名解耦。
>
> 下文保留大量历史设计推导，方便理解为什么会收敛到当前形态；但如果与代码冲突，请以实现与 `docs/user-guide.md` / `src/core/llm/README.md` 为准。

> **范围**：把 tomcat 当前「全局单 `LlmProvider` + 若干 model 字符串」升级为 **元数据驱动（Model Catalog）+ 场景化路由 + 多后端鉴权/降级** 的可产品化形态。
>
> **承接**：本文聚焦「**多模型/多后端如何变成用户可选、可降级、可计量的产品能力**」；provider 主骨架与 wire 接线见 [`llm-multiprovider-integration.md`](llm-multiprovider-integration.md)，跨 turn 推理续传见 [`llm-openai-deepseek-reasoning-continuity.md`](llm-openai-deepseek-reasoning-continuity.md)，stream 事件管线见 [`llm-stream-events-cli-pipeline.md`](llm-stream-events-cli-pipeline.md)。三者是本方案的**前置事实**，本文不重复其 wire 细则。
>
> **写法**：遵循 [`ARCHITECTURE_SPEC.md`](../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)：文首总图 → §1 术语 → §2 竞品调研 → §3 目标 → §4 已定稿选型与实施 → §5 协议 → §6 One-Glance → §7 时序 → §8 状态机 → §9 配置 → §10 错误模型 → §11 测试矩阵 → §12 风险 → §13 历史决策。
>
> **调研对象（本工作区磁盘可稽核）**：`pi_agent_rust/`、`hermes-agent/`、`openclaw/`、`pi-mono/`、`codex/`。所有外部证据均落到具体仓库文件路径。

---

## 先看总图：从「选模型」到「打哪条 HTTP」

> 图例：**【现有】** = tomcat 今天已有的代码；**【新增】** = 本方案要补的。核心跃迁是把「**选 model 只改请求体里的字符串**」升级为「**选 model 自动重定向 api/base_url/key**」。

```text
  输入（每次调用）                事实源（新增两层）                       选实现 + 拼包（复用现有）              出站（不变）
  ───────────────                ──────────────────────────              ──────────────────────              ──────────
  scene: main|compaction|        ┌────────────────────────────┐
        vision|title …           │ ModelCatalog【新】           │
  session.model_override         │  by_id: model_id →          │
  [llm].default_model            │   { api, provider,          │
        │                        │     base_url?, caps,        │
        │ ① 选 model_id          │     cost?, ctx_window? }     │
        │  override>scene>default │  lookup(id):                │
        ▼                        │   命中 → ModelEntry          │
  ┌──────────────────┐  查表 ───► │   缺失 + 显式选模 → Err      │
  │  LlmResolver【新】 │           │     「模型未收录，请补      │
  │                   │◄── Entry ─┤      models.toml / 切回已收录」│
  │                   │           │   缺失 + 旧配置兼容 → Legacy │ ← G7 老配置照常跑
  │                   │           │     Entry([llm].provider/    │
  │                   │           │            api_base)         │
  │ ② 能力校验(caps)   │           └────────────────────────────┘
  │   不匹配→引导错误   │           ┌────────────────────────────┐
  │ ③ 取 key          │◄── key ───│ AuthStore【新】              │ provider→<PROVIDER>_API_KEY
  │                   │           │  缺失 → 「请设置 X」可读错误  │   / OAuth（首期回落 api_key_env）
  │ ④ 拼 LlmConfig{    │           └────────────────────────────┘
  │    provider = api, │           ┌────────────────────────────┐
  │    api_base =      │  ⑤ resolve│ resolve_llm(LlmConfig)【现有】│
  │      entry.base_url,│─────────► │  按 provider(=api 串) 查表    │
  │    api_key_env }   │           │  → Arc<dyn LlmProvider>     │
  │                   │◄──────────┤  〔按 api+base+key 缓存复用〕 │
  └────────┬──────────┘           └─────────────┬──────────────┘
           │ ResolvedCall{ provider_impl, model,│ thinking_fmt =
           │   base_url, key, thinking_fmt }    │ thinking_format_for_model(model)【现有】
           │                                    ▼  （deepseek-*→Deepseek, 其它→Openai…）
           │                        ┌───────────────────────────────┐
           ├─ 正常 ─────────────────►│ provider.chat_stream(          │─ /v1/responses ───────┐
           │                        │   ChatRequest{ model, … } )【现有】│─ /v1/chat/completions ┤
           │                        └───────────────────────────────┘                       │ SSE/NDJSON
           └─ 402 / 连接失败 / 能力不匹配【新】                                                ▼
                → mark_unhealthy(provider, TTL) → FallbackChain.next(entry) → 回到 ①    StreamEvent（统一,【现有】）
```

**看图顺序（说人话）**：

1. **① 选 model_id**：每次调用先按 `会话已选 model_override > scene 键 > default_model` 定出一个**模型名**——这一步**今天已有**（`effective_model`），变化在后面。
2. **查 Catalog（新）**：拿 model_id 查 **ModelCatalog**，得到 `ModelEntry`，里面带 `api`（走哪条 wire）、`provider`（取哪把 key）、`base_url`、能力位。**catalog miss 不允许静默**：若是用户显式选中的模型（会话 override / 未来 `/model` / TUI / scene 键），直接给结构化错误「模型 `<id>` 未收录，请补 `models.toml` 或切回已收录模型」；只有**旧配置兼容路径**才允许回落成「今天的单 provider」legacy entry（G7）。
3. **② 能力校验 + ③ 取 key（新）**：vision/files 能力不匹配在**调用前**就给引导式错误；按 `provider` 从 **AuthStore** 取对应 key（多家 key 各取各的），缺失给「请设置 X」。
4. **④ 拼 LlmConfig + ⑤ resolve_llm（复用现有）**：Resolver 用 `entry.api` 当 `provider` 串、`entry.base_url` 当 `api_base`，组一份 `LlmConfig` 调**既有** `resolve_llm` 拿到 `Arc<dyn LlmProvider>`；相同 `(api, base_url, key)` 的 provider 实例缓存复用，避免每次重建 HTTP client。**例子**：`gpt-5.4` → `entry{ api=openai-responses, base_url=https://api.openai.com, provider=openai }` → `resolve_llm` 选 `OpenAiResponsesProvider`；`deepseek-reasoner` → `entry{ api=openai, base_url=https://api.deepseek.com, provider=deepseek }` → `resolve_llm` 选 `OpenAiProvider`。
5. **wire 与 thinking 自动**：thinking 字段仍由**现有** [`thinking_format_for_model`](../../src/core/llm/thinking_policy.rs) 按 model 名推断（`deepseek-*` 自动走 DeepSeek wire），调用方仍组**一份** `ChatRequest`。
6. **失败降级（新）**：402/连接/能力不匹配时把坏 provider 记进 TTL 黑名单并走 FallbackChain 换下一个 entry，回到 ①。
7. **关键跃迁**：路由键从「provider 名」变成「**model_id → catalog → api**」。**例子**：今天若 `[llm] provider = "openai"`、`api_base = https://api.deepseek.com`，那你把会话模型从 `deepseek-reasoner` 切到 `gpt-5.4`，请求仍会走**同一个** `OpenAiProvider + 同一个 DeepSeek endpoint/key`，只是 `ChatRequest.model` 从 `deepseek-reasoner` 变成 `gpt-5.4`，所以必然 4xx；改造后前者会经 catalog 命中 `api=openai / base_url=api.deepseek.com / key=DEEPSEEK_API_KEY`，后者会命中 `api=openai-responses / base_url=api.openai.com / key=OPENAI_API_KEY`。也就是说，「选 model」不再只是换字符串，而是会**自动重定向 api/base_url/key**——这正是 §4.2.0 要解决的 openai+deepseek 共存问题。

---

## 1. 术语统一（MUST）

| 术语 | 语义（大白话） | 数据载体 | 行为约束 / 互斥 |
|------|----------------|----------|------------------|
| **Provider impl** | 一份 `impl LlmProvider`，负责把 `ChatRequest` 翻成某条 HTTP wire 并解析流 | `Arc<dyn LlmProvider>`（[`provider.rs`](../../src/core/llm/provider.rs)） | 由 `resolve_llm` 构造；调用方不感知具体类型 |
| **`api`（协议族）** | 「走哪条 wire」的标识：`openai-responses` / `openai-completions` / `anthropic-messages` … | 计划新增 `ModelEntry.api`；当前等价物是 `LlmConfig.provider` 字符串 | **路由键**；与「厂商名」正交（pi-mono `Model.api`） |
| **provider（厂商）** | 商业/逻辑厂商：`openai` / `deepseek` / `anthropic`，用于 **auth 分组、catalog 分类与展示** | `ModelEntry.provider` | **不**直接决定 wire；例如 `provider=deepseek` 时仍可由 `api="openai"` 命中 `OpenAiProvider`。当前 registry 里也**没有**独立 `"deepseek"` provider id。 |
| **Model Catalog** | 模型清单：id → 元数据（api、provider、base_url、能力、成本、窗口） | 计划新增 `core/llm/catalog.rs` + 内置表 + 用户 `models.toml` 覆盖 | 单一事实源；UI/Resolver/预算都读它 |
| **ModelEntry** | catalog 中一行模型的解析结果 | `struct ModelEntry`（计划） | 由 catalog 查表得到；驱动 Resolver |
| **scene（场景）** | 这次调用属于哪类任务：`main` / `compaction` / `vision` / `title` … | 计划 `enum LlmScene` | 决定默认取哪个模型键；与 override 叠加 |
| **LlmResolver** | 把 (scene + override + catalog + auth + health) 解析成一次具体调用 | 计划 `trait LlmResolver` / `ResolvedCall` | 注入 `ChatContext`；替代当前「from_config 时固定一个 llm」 |
| **ResolvedCall** | 一次调用的全部已定参数：provider_impl、model、base_url、key、thinking_fmt | 计划 `struct ResolvedCall` | per-call 生成；不再 per-process 固定 |
| **AuthStore** | 多 key / OAuth token 的统一读取与缓存 | 计划 `core/llm/auth.rs`；现状是单 `api_key_env` | 按 provider 取凭证；缺失给可读错误 |
| **FallbackChain** | 主模型失败时按序尝试的备选模型/后端 | 计划配置 + Resolver 逻辑；现状仅 `api_base_fallback`（同模型换 base） | 触发条件：402 / 连接失败 / 能力不匹配 |
| **unhealthy cache** | 最近失败的 provider 在 TTL 内被跳过 | 计划 `Mutex<HashMap<provider, Instant>>` | 借鉴 hermes 600s TTL |
| **能力位（capability）** | 模型支不支持 vision / files / tools / reasoning | `ModelEntry.capabilities` | 路由前校验；不匹配给引导式错误 |

> **时间点钉死**：本文「**调用前**」= `AgentLoop` 组好 `ChatRequest`、尚未调用 `LlmProvider::chat*` 之时；「**调用后失败**」= provider 返回 `Err` 或流内 `LlmError` 之后、本轮重试/降级决策之时。

---

## 2. 竞品 / 选型对比（调研，MUST）

五个仓库都解决了同一问题：**让一套 agent 循环跑在多家 LLM 上**。它们的共同骨架可抽象为四层（与本文总图对应）：

```text
  Catalog（模型元数据，含 api 字段）
        │
  Resolver / Route（按 api 选实现，按 provider 选 auth）
        │
  Provider/Transport impl（拼 wire + 解析流）
        │
  统一事件（StreamEvent / AssistantMessageEvent）
```

### 2.1 五仓横向对照

| 仓库 | 抽象单元 | 路由键 | Catalog 来源 | 鉴权 | 场景化/降级 | 我们借鉴的点 | 说人话 |
|------|----------|--------|--------------|------|--------------|---------------|--------|
| **pi-mono** (`packages/ai`) | `ApiProvider{api,stream,streamSimple}`（[`api-registry.ts`](../../../pi-mono/packages/ai/src/api-registry.ts)） | **`Model.api`**（与 provider 正交） | 生成物 `models.generated.ts` + 用户 `models.json` 合并（[`models.ts`](../../../pi-mono/packages/ai/src/models.ts)） | coding-agent 层 `getApiKeyAndHeaders` 注入（[`sdk.ts`](../../../pi-mono/packages/coding-agent/src/core/sdk.ts)） | `setModel`/`cycleModel`；scoped models | **api≠provider 正交**；`streamSimple` 稳定契约；catalog 生成+override 分层 | 路由只看协议族，厂商名只管认证。 |
| **openclaw** | `StreamFn`（按 `model.api` 二选一，[`provider-transport-stream.ts`](../../../openclaw/src/agents/provider-transport-stream.ts)） | **`model.api`** | pi `ModelRegistry` + manifest + 用户 `models.providers`（[`model-catalog.ts`](../../../openclaw/src/agents/model-catalog.ts)） | auth-profiles + pi auth.json + env 轮换 | 三层 StreamFn 来源：plugin > transport > streamSimple（[`stream-resolution.ts`](../../../openclaw/src/agents/pi-embedded-runner/stream-resolution.ts)） | **Catalog 与 Runtime model 分离**；plugin-first 扩展；网关不直连 LLM | UI 用一份清单，跑模型时用另一份带 api 的对象。 |
| **pi_agent_rust** | `Provider{stream}`（[`provider.rs`](../../../pi_agent_rust/src/provider.rs)） | `ModelEntry.model.api` → `resolve_provider_route` → `ProviderRouteKind`（[`providers/mod.rs`](../../../pi_agent_rust/src/providers/mod.rs)） | 内嵌生成 TS + 上游快照 + 用户 `models.json` 三层（[`models.rs`](../../../pi_agent_rust/src/models.rs)） | `provider_metadata` env keys + `/login`（[`provider_metadata.rs`](../../../pi_agent_rust/src/provider_metadata.rs)） | **base_url 归一化**；未知 provider 按 api fallback 路由；无模型级 failover | **`normalize_*_base` 容忍用户抄错 endpoint**；api 驱动工厂；Rust 同构最近 | Rust 版工厂 + 注册表，跟我们 registry 最像。 |
| **hermes-agent** | `ProviderProfile`（声明）+ `ProviderTransport`（wire，[`transports/base.py`](../../../hermes-agent/agent/transports/base.py)） | **`api_mode`** + URL 启发式（[`providers.py`](../../../hermes-agent/hermes_cli/providers.py)） | 内置/models.dev + `config.providers` + 插件（[`runtime_provider.py`](../../../hermes-agent/hermes_cli/runtime_provider.py)） | 30+ 插件 provider，env/OAuth/auth.json（[`auth.py`](../../../hermes-agent/hermes_cli/auth.py)） | **`auxiliary` per-task 模型** + auto 链 + **402 fallback + 600s unhealthy cache**（[`auxiliary_client.py`](../../../hermes-agent/agent/auxiliary_client.py)） | **场景化辅助模型**与主对话解耦；**付费/连接失败降级链**；URL→api_mode 检测 | 不同小任务配不同便宜模型，余额没了自动换家。 |
| **codex** | `ModelProvider` trait + `ModelClient`（crate 分层，[`client.rs`](../../../codex/codex-rs/core/src/client.rs)） | **`WireApi`**（当前仅 Responses） | 内置 4 provider + 用户 `[model_providers.*]`（[`config_toml.rs`](../../../codex/codex-rs/config/src/config_toml.rs)） | ChatGPT OAuth/APIKey/command/AWS（[`auth/manager.rs`](../../../codex/codex-rs/login/src/auth/manager.rs)） | **分层重试**：HTTP 4 + 流 5 + WS→HTTP fallback；429→usage-limit 语义；ProfileV2 | **crate 边界（元数据/运行时/客户端/传输/认证分离）**；重试分层；reasoning first-class | 把元数据、传输、认证拆干净，最适合做大。 |

### 2.2 关键共识（5 条，为什么这么选）

1. **路由键是「协议族 `api`」而非「厂商名」**（pi-mono、openclaw、pi_agent_rust、hermes 全部如此）：同一 `openai-completions` wire 服务 DeepSeek/Groq/OpenRouter/Kimi…，换厂商只是换 `base_url + key`，**不**新增实现。tomcat 现状 `registry.rs` 已是此形态（`provider` 字符串即 api），但**缺 catalog 把 model→api 自动绑定**。
2. **Catalog = 生成/内置表 + 用户 override 两层**（pi-mono `models.generated.ts`+`models.json`、pi_agent_rust 三层、openclaw `ModelRegistry`）：用户选 model id，系统查出 api/base_url/能力，不必让用户同时懂 `provider` 与 `default_model`。
3. **元数据 vs 运行时 vs 鉴权分离**（codex 的 crate 拆分最干净）：tomcat 应把 `ModelEntry`/`AuthStore`/`ResolvedCall` 从 `LlmConfig` 的横切字段里独立出来。
4. **场景化模型是产品刚需**（hermes `auxiliary.*`）：vision / compaction / title / triage 各配不同（常常更便宜的）模型；tomcat 已有 `compaction_model` 先例，需推广为通用 scene 键。
5. **失败要降级而非硬报错**（hermes 402 链 + unhealthy cache、codex WS→HTTP fallback、pi_agent_rust api fallback）：tomcat 现状只有「同模型换 base」（`api_base_fallback`），缺「换模型/换厂商」与「能力不匹配引导」。

### 2.3 为什么不走「全局统一 IR（岔路 B）」

[`llm-multiprovider-integration.md` §6.2](llm-multiprovider-integration.md) 已冻结 **岔路 A**（多 `impl LlmProvider` + 元数据路由）。五仓里只有 pi_agent_rust 维护了内部 `model::Message` IR，但其 wire 翻译仍在各 provider 内；pi-mono/openclaw/hermes/codex 都直接以 **OpenAI 形消息** 作中间表示。tomcat 的 `ChatMessage` 已是 OpenAI 形，**本方案不引入新 IR**，只在其上加 Catalog + Resolver 两层。

---

## 3. 目标与设计原则（MUST）

### 3.1 观察指标（落地后用户可感知）

| 目标 | 观察指标 | 说人话 |
|------|----------|--------|
| **G1 选模型即可用** | 用户只填 `model = "deepseek-reasoner"`，无需手配 `provider`/`api`/`base_url`，系统从 catalog 补齐并成功对话 | 填个模型名就能跑，别让我背一堆配置。 |
| **G2 场景化模型** | `vision`/`compaction`/`title` 可各配模型；未配则回落主模型；compaction 仍走低成本模型 | 不同活儿用不同模型，省钱。 |
| **G3 能力前置校验** | 给非 vision 模型发图片时，**调用前**返回结构化错误并建议可用模型，而非上游 4xx | 模型不支持图片就提前说，别等服务器报错。 |
| **G4 失败降级** | 主模型 402/连接失败时，按 FallbackChain 自动换下一个可用模型；坏 provider 进 TTL 黑名单 | 一家挂了自动换下一家，别整轮失败。 |
| **G5 多 key/凭证** | OpenAI/DeepSeek/Anthropic 各用各的 key env；缺失时报「请设置 X」可读错误 | 多家 key 各管各的，缺哪个说哪个。 |
| **G6 可计量** | 每次调用 trace 带 `scene/provider/api/model/latency/retry`；usage 按 model 聚合 | 每次调用都能看清用了谁、花了多少。 |
| **G7 不破坏现状** | 未配 catalog/scene 时，行为与今天「单 provider + default_model」完全一致 | 老配置照常跑，不强制迁移。 |

### 3.2 非目标（本方案不做 / 推给谁）

| 非目标 | 推给 | 说人话 |
|--------|------|--------|
| 跨 turn reasoning 续传细则 | [`llm-openai-deepseek-reasoning-continuity.md`](llm-openai-deepseek-reasoning-continuity.md) | 续传规则那篇已经管了。 |
| 新增 Anthropic 等具体 provider impl 的 wire | [`llm-multiprovider-integration.md`](llm-multiprovider-integration.md) §6.5 岔路 A | 真要接 Anthropic 按主骨架那篇做。 |
| Mixture-of-Agents 多模型协作 | 后续看板（Wave 3） | 多模型投票/聚合以后再说。 |
| 网关（OpenAI 兼容 HTTP server） | 后续看板（Wave 3） | 对外暴露 OpenAI API 那套先不做。 |
| 模型自动发现/在线拉取 catalog | 后续看板 | 先用内置表 + 本地覆盖，不联网拉清单。 |
| 引入全局 IR（岔路 B） | 远期评估（§2.3） | 不重写消息中间层。 |

### 3.3 设计原则

1. **api 与 provider 正交**：路由看 `api`，鉴权看 `provider`。
2. **Catalog 单一事实源**：model 元数据只在 catalog 定义一次，UI/Resolver/预算/能力校验共用。
3. **稳定 schema**：`LlmConfig` 不为每个厂商加专属字段（沿用 [`llm-multiprovider-integration.md`](llm-multiprovider-integration.md) §6.5.2 约定）；新增后端 = catalog 一行 + 可能的 registry 一行。
4. **零迁移成本兜底**：所有新键 `#[serde(default)]`；缺省时退化为今天的单 provider 路径（G7）。
5. **降级优先于失败**：可恢复错误走 FallbackChain，不可恢复才 `Err`。

---

## 4. 落地选型与实施（已定稿）（MUST）

### 4.1 落地选型决策表（七列）

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
|------|------|------|------|----------|----------------|--------|
| **R1 路由键** | 按厂商名还是按协议族路由？ | **采用 `api`（协议族）作路由键**，`provider` 仅用于 auth/catalog 分组；拒绝「provider 名 = 实现」 | tomcat [`registry.rs`](../../src/core/llm/registry.rs)（`provider` 字符串查表）；pi-mono [`api-registry.ts`](../../../pi-mono/packages/ai/src/api-registry.ts)（注册表 key=api）；openclaw [`provider-transport-stream.ts`](../../../openclaw/src/agents/provider-transport-stream.ts)（`switch(model.api)`） | 设计：catalog 给每个 model 标 `api`，Resolver 用 `api` 调 `resolve_llm`；理由：DeepSeek/Groq/Kimi 共用 `openai-completions`，换厂商只换 base+key，**0 新实现** | 拒 hermes 的「URL 启发式推 api_mode」为**主**路径（[`providers.py`](../../../hermes-agent/hermes_cli/providers.py)）：隐式、难审计；仅作 catalog 缺失时兜底 | 路由认协议不认牌子。 |
| **R2 元数据载体** | model 元数据塞进 `LlmConfig` 还是独立 Catalog？ | **采用独立 `ModelCatalog`（内置表 + 用户 `models.toml` 覆盖）**；拒绝在 `LlmConfig` 加 per-model 字段 | tomcat [`llm.rs`](../../src/infra/config/types/llm.rs)（现状横切字段）；pi-mono [`models.ts`](../../../pi-mono/packages/ai/src/models.ts)；pi_agent_rust [`models.rs`](../../../pi_agent_rust/src/models.rs)（三层叠加） | 设计：`catalog.rs` 内置常用模型 + 合并 `~/.tomcat/models.toml`；理由：用户填 model id 即得 api/base_url/能力，schema 不随厂商膨胀 | 拒「全部塞 config.toml」：每加一家就改中央结构体，违背 §6.5.2 稳定 schema | 模型清单单独放一处，配置文件不爆炸。 |
| **R3 选型时机** | per-process 固定 provider 还是 per-call 解析？ | **采用 per-call `LlmResolver` → `ResolvedCall`**；保留 `from_config` 兜底单 provider | tomcat [`context.rs`](../../src/api/chat/context.rs)（现状 `from_config` 固定 `llm`）；codex [`client.rs`](../../../codex/codex-rs/core/src/client.rs)（turn 级参数显式传）；hermes [`auxiliary_client.py`](../../../hermes-agent/agent/auxiliary_client.py) | 设计：每次调用按 scene+override 解析；理由：compaction/vision 可走不同 model/base_url 甚至不同 impl，per-process 固定做不到 | 拒「只换 model 字符串」现状：无法跨 provider/ base_url 差异化 scene | 每次调用现算用谁，不写死一个。 |
| **R4 场景化模型** | vision/压缩等是否独立模型键？ | **采用 `LlmScene` + 场景模型键**（`vision_model`/`title_model`…），未配回落主模型 | tomcat [`context.rs`](../../src/infra/config/types/context.rs)（`compaction_model` 已落地）；hermes [`config.py`](../../../hermes-agent/hermes_cli/config.py)（`auxiliary.*`） | 设计：沿用 compaction_model 模式推广到通用 scene；理由：省成本 + 能力匹配（vision 用多模态模型） | 拒「所有场景共用主模型」：贵且未必支持图片 | 小任务配小模型，看图配看图模型。 |
| **R5 鉴权** | 单 key env 还是多凭证 Store？ | **采用 `AuthStore`（按 provider 取 key/OAuth）**；现状单 `api_key_env` 作 fallback | tomcat [`llm.rs`](../../src/infra/config/types/llm.rs)（`api_key_env`）；codex [`auth/manager.rs`](../../../codex/codex-rs/login/src/auth/manager.rs)；hermes [`auth.py`](../../../hermes-agent/hermes_cli/auth.py) | 设计：provider→key env/OAuth 映射 + 缺失可读错误；理由：多家并存必须多 key，缺失要引导而非泛 Config 错 | 拒 codex 全套 OAuth/PKCE/keyring **首期**全做：工程量大，先 env 多 key，OAuth 留 Wave 3 | 每家 key 分开放，缺了告诉你设哪个。 |
| **R6 降级** | 失败硬报错还是 FallbackChain？ | **采用 FallbackChain + unhealthy cache(TTL)**；扩展现有 `api_base_fallback` | tomcat [`openai.rs`](../../src/core/llm/openai.rs)（现 `api_base_fallback` 同模型换 base）；hermes [`auxiliary_client.py`](../../../hermes-agent/agent/auxiliary_client.py)（402+600s TTL）；codex [`session/turn.rs`](../../../codex/codex-rs/core/src/session/turn.rs)（WS→HTTP） | 设计：402/连接失败→按链换 model/后端，坏 provider 进 TTL 黑名单；理由：单点故障不应整轮失败 | 拒「无脑重试同一家」：余额耗尽时纯浪费 RTT（hermes 注释明示） | 一家挂了换下一家，挂过的先晾一会。 |
| **R7 能力校验** | 何时校验 vision/files 支持？ | **采用 catalog `capabilities` 调用前校验**，不匹配给引导式结构化错误 | tomcat [`openai.rs`](../../src/core/llm/openai.rs)（`reject_multimodal_parts` 已有思路）；pi-mono [`transform-messages.ts`](../../../pi-mono/packages/ai/src/providers/transform-messages.ts)（`downgradeUnsupportedImages`）；openclaw payload policy | 设计：catalog 标能力位，Resolver 校验；理由：把上游 4xx 提前成本地可读引导 | 拒「发出去让上游报错」：错误信息差、浪费请求 | 模型不支持就提前拦，并告诉你换谁。 |
| **R8 运行时用户入口** | 聊天模式下用户如何查询/切换 model？ | **采用 chat 本地 `/model` 命令族作为主入口**（`current` / `list` / `use <id>`）；`/model use` 把所选 model **持久化到当前 session 的 `model_override`（`sessions.json`）**，程序重入 / resume 仍生效；CLI prompt/横幅显示当前对话模型；TUI picker 作为同能力外观 | tomcat [`commands/parse.rs`](../../src/api/chat/commands/parse.rs)（当前 slash-command 入口）、[`cmd_help.rs`](../../src/api/chat/commands/cmd_help.rs)（当前命令列表无 `/model`）、[`session_impl.rs::switch_current_model`](../../src/core/session/manager/session_impl.rs)、[`store.rs::SessionEntry`](../../src/core/session/store.rs)、[`prompt.rs`](../../src/api/chat/prompt.rs)；pi-mono [`model-resolver.ts`](../../../pi-mono/packages/coding-agent/src/core/model-resolver.ts)（`setModel` / `cycleModel`） | 设计：`/model current` 展示当前 effective model + resolved `{api, provider, base_url, key_source}`；`/model use <id>` 校验 catalog 命中后更新当前 session 的 `model_override`、落 `model_change` 审计，并让 CLI 立即显示新的当前模型；理由：当前对话模型应是**会话级持久化选择**，而不是一次进程内的临时变量 | 拒把 `config_set("llm.default_model")` 作为主入口：它改的是**全局默认值**，不是当前对话；拒保留 `/model clear`：它会把产品心智拉回「临时覆盖/清空覆盖」，与“程序重入依旧有效”冲突；拒依赖 dispatcher `llm.setModel`：[`ops.rs`](../../src/ext/dispatcher/ops.rs) 目前仅 MVP stub | 在聊天里直接 `/model use xxx`，退出再进也还是这个会话模型。 |
| **R9 初始化向导** | `tomcat init` 是否应让用户选择 model 并配置多 provider key？ | **采用 model-first 的交互式 init 向导**：先选 `default_model`，再按 catalog 推导所需 `provider/api` 并提示写入对应 key；可选顺手配置额外 provider 凭证 | tomcat [`api/cli/init.rs`](../../src/api/cli/init.rs)（当前仅写默认 `openai-responses + DEFAULT_LLM_MODEL` 并提示 `OPENAI_API_KEY`）；codex [`config_toml.rs`](../../../codex/codex-rs/config/src/config_toml.rs) + [`auth/manager.rs`](../../../codex/codex-rs/login/src/auth/manager.rs)（配置与认证分离）；pi-mono [`auth-storage.ts`](../../../pi-mono/packages/coding-agent/src/core/auth-storage.ts) | 设计：`tomcat init` 不再先问 provider，而是先让用户选 model；选完 `gpt-5.4` 就提示 `OPENAI_API_KEY`，选 `deepseek-reasoner` 就提示 `DEEPSEEK_API_KEY`；理由：符合本方案的 model-first 产品心智，也避免用户理解 `api/provider` 细节 | 拒维持当前「只问 OPENAI_API_KEY」：无法初始化 DeepSeek/多 provider；拒要求用户首次启动后手改 `tomcat.config.toml` + `.env`：门槛高、易配错 | 初始化时就把模型和钥匙配好，开箱能聊。 |

### 4.2 实施点（已闭环规划）

> 分三波（Wave）。Wave 1 是低成本高复用的地基，可在「单 Agent 完善期」并行认领；Wave 2/3 视产品需求推进。每行与 §4.1 的 R 维度映射见末列括注。

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| **W1-1 Catalog 骨架** (R1/R2/R7) | `ModelCatalog`/`ModelEntry` 类型 + 内置常用模型表（gpt-5.x/deepseek-*/…）+ `model→api/provider/base_url/caps` 解析；用户 `models.toml` 合并 | 新建 `src/core/llm/catalog.rs`；`core/llm/mod.rs` re-export；内置表与 `registered_provider_ids` 对齐 | `catalog::tests::resolve_known_model_*`、`merge_user_override_*` | 先把「模型清单」这张表建起来。 |
| **W1-2 Resolver + scene** (R3/R4) | `LlmScene` 枚举 + `trait LlmResolver`/`ResolvedCall`；`vision_model`/`title_model` 等 scene 键（沿用 `compaction_model`） | 新建 `src/core/llm/resolver.rs`；`api/chat/context.rs` 注入 Resolver；`infra/config/types/context.rs` 增 scene 键 | `resolver::tests::scene_fallback_to_main_*`、`override_priority_*` | 每次调用按场景挑模型。 |
| **W1-3 能力校验前移** (R7) | catalog `capabilities` + 调用前校验；非 vision 模型遇附件→结构化引导错误（含「建议改用 X」） | `src/core/llm/resolver.rs`（校验）；复用 [`openai.rs::reject_multimodal_parts`](../../src/core/llm/openai.rs) 错误风格 | `resolver::tests::reject_vision_on_text_model_*` | 不支持图片就提前拦下来。 |
| **W1-4 运行时 `/model` 入口 + CLI 显示** (R3/R8) | `/model current|list|use` 命令族；`/model use` 持久化 `model_override` 到 `sessions.json` + `model_change` 审计；CLI 横幅/输入 prompt 持续显示当前对话 model；TUI picker 后续复用同一后端 | 新建 `src/api/chat/commands/cmd_model.rs`；扩展 [`commands/parse.rs`](../../src/api/chat/commands/parse.rs) / [`cmd_help.rs`](../../src/api/chat/commands/cmd_help.rs)；复用 [`session_impl.rs::switch_current_model`](../../src/core/session/manager/session_impl.rs)；调整 [`prompt.rs`](../../src/api/chat/prompt.rs) / [`run_loop/mod.rs`](../../src/api/chat/run_loop/mod.rs) | `commands::tests::cmd_model_*`、`session::crud_test::switch_current_model_*`、`chat::tests::prompt_shows_current_model_*` | 聊天里直接查和切模型，且界面上看得见。 |
| **W1-5 `tomcat init` 模型与凭证向导** (R5/R9) | `tomcat init` 可选择 `default_model`，并按 catalog 推导并写入所需 key（OpenAI / DeepSeek / …）；可选补充额外 provider 凭证 | [`api/cli/init.rs`](../../src/api/cli/init.rs)；新建 `src/api/cli/init_model_wizard.rs`（建议）+ `core/llm/catalog.rs` / `core/llm/auth.rs` | `cli::tests::init_prompts_default_model_*`、`init_writes_provider_keys_*` | 初始化时就把模型和 key 配好。 |
| **W2-1 AuthStore 多 key** (R5) | provider→key env 映射 + 缺失可读错误；保留单 `api_key_env` 兜底 | 新建 `src/core/llm/auth.rs`；`resolver.rs` 消费 | `auth::tests::missing_key_message_*`、`per_provider_env_*` | 多家 key 各取各的。 |
| **W2-2 FallbackChain + 健康度** (R6) | FallbackChain 配置 + 402/连接失败降级 + unhealthy cache(TTL)；扩展 `api_base_fallback` | `resolver.rs`（链）；[`openai.rs`](../../src/core/llm/openai.rs)/`openai_responses` 错误分类回传 | `resolver::tests::fallback_on_402_*`、`unhealthy_ttl_skip_*` | 一家挂了自动换。 |
| **W2-3 可观测/计量** (R6/G6) | 每次调用 trace span（scene/provider/api/model/latency/retry）；usage 按 model 聚合 | [`token_usage.rs`](../../src/core/llm/token_usage.rs)；provider 调用处加 `tracing` 字段 | `token_usage::tests::aggregate_by_model_*` | 看得清用了谁花了多少。 |
| **W3-x 扩展面** (R1/R5) | 第二家非 OpenAI 形 provider（如 Anthropic）；OAuth；网关；MoA | 按 [`llm-multiprovider-integration.md`](llm-multiprovider-integration.md) §6.5 岔路 A 新增 impl | 各自 spec | 以后要接 Anthropic/对外网关再说。 |

#### 4.2.0 选模型：今天的 openai/deepseek 现状 → 目标

> 直接回答「**怎么选模型？现在的 openai provider 不是已经支持 openai 和 deepseek 多个模型了吗？**」

**今天的真相（部分对、但有硬约束）**：tomcat 只有**一个** provider 实例，它在 [`OpenAiProvider::new`](../../src/core/llm/openai.rs)（约 L413–446）构造时就把 **`base_url` / `api_key` / `default_model` 各锁成单值**：

```text
[llm] provider     → resolve_llm 选 OpenAiProvider 还是 OpenAiResponsesProvider（进程级，1 个）
[llm] api_base     → OpenAiProvider.base_url（构造时定死，1 个）
[llm] api_key_env  → OpenAiProvider.api_key  （构造时读 env，1 个）
[llm] default_model + 会话 model_override → effective_model → ChatRequest.model（运行时可变）
```

所以「选模型」**今天只做一件事**：改 `ChatRequest.model` 这个**字符串**（[`effective_model`](../../src/api/chat/context.rs)：会话 override 优先，否则 default_model）。请求体的 model 变了，但 **endpoint 和 key 不会跟着变**。

| 维度 | 今天会不会随「选 model」变 | 证据 |
|------|---------------------------|------|
| `ChatRequest.model` 字符串 | **会**（override > default_model） | [`context.rs::effective_model`](../../src/api/chat/context.rs) |
| thinking wire（reasoning_effort vs deepseek thinking） | **会**（按 model 名自动推断） | [`thinking_format_for_model`](../../src/core/llm/thinking_policy.rs)：`deepseek-*`→Deepseek |
| continuity profile（replay/strip 规则） | **会**（按 model family） | [`replay_policy.rs::model_family`](../../src/core/llm/replay_policy.rs) |
| **`base_url`（打到哪家）** | **不会**（构造时锁死） | [`openai.rs` L414–419](../../src/core/llm/openai.rs) |
| **`api_key`（用哪把钥匙）** | **不会**（构造时锁死） | [`openai.rs` L420–422](../../src/core/llm/openai.rs) |

**结论**：今天的 `OpenAiProvider` 是「**OpenAI-compatible 通用客户端**」，能接 OpenAI **或** DeepSeek——取决于你把 `[llm] api_base` 指到 `api.openai.com` 还是 `api.deepseek.com`、`api_key_env` 设成 `OPENAI_API_KEY` 还是 `DEEPSEEK_API_KEY`。`.env` 里 `OPENAI_API_KEY` 和 `DEEPSEEK_API_KEY` 两把 key 都在，但**同一时刻只有一家生效**：

- 配 DeepSeek 时把 `default_model` 设成 `deepseek-reasoner` 能跑；
- 但此时把会话 `model_override` 改成 `gpt-5.4`，请求仍会被发到 **DeepSeek 的 endpoint + DeepSeek 的 key**，结果 4xx。
- 反之亦然。**`gpt-5.4` 与 `deepseek-reasoner` 无法在同一进程/会话里共存切换**——这就是要补 catalog 的根因。

**目标（catalog + resolver 后）**：把「model_id → (api, base_url, provider→key)」的绑定关系沉到 **ModelCatalog**，`LlmResolver` 每次按选中的 model **重新拼 `LlmConfig` 再调 `resolve_llm`**，于是：

```text
  选 "gpt-5.4"        → entry{ api=openai-responses, base_url=api.openai.com,  provider=openai }   → OPENAI_API_KEY
  选 "deepseek-reasoner" → entry{ api=openai,         base_url=api.deepseek.com, provider=deepseek } → DEEPSEEK_API_KEY
        （同一会话内来回切，endpoint/key 自动跟着模型走；thinking wire 仍由 model 名自动适配）
```

> 注意：这里的 `provider=deepseek` **不是** `resolve_llm` 要查找的 provider id。当前 [`registry.rs`](../../src/core/llm/registry.rs) 里只有 `"openai"` 与 `"openai-responses"`；真正决定命中 `OpenAiProvider` 还是 `OpenAiResponsesProvider` 的是 **`api`**，而 `provider` 只负责告诉 AuthStore「去拿 `DEEPSEEK_API_KEY`」、并作为 catalog/展示/审计标签。

即「选 model」从「只换字符串」升级为「**自动重定向 api + base_url + key**」，真正实现 openai/deepseek（乃至更多家）**共存可切换**（G1）。`thinking_format_for_model` / continuity profile 这两块**今天已经按 model 名自动分派、无需改动**，catalog 只负责补上 endpoint+key 这一环。

#### 4.2.1 W1-1 Catalog 骨架（技术要点）

```text
  models.toml(用户)         内置表(catalog.rs)
        │                        │
        └──── merge(覆盖) ───────┘
                 │  ModelCatalog { by_id: HashMap<String, ModelEntry> }
                 ▼
        ModelEntry { id, api, provider, base_url?, capabilities, cost?, context_window? }
                 │  lookup(model_id) → 命中: ModelEntry
                 │                    未命中 + 显式选模: Err(模型未收录)
                 │                    未命中 + 旧配置兼容: Legacy Entry
                 ▼
        api 字段 → 喂给 resolve_llm（与 registry.rs 的 provider id 对齐）
```

- `api` 取值与 [`registry.rs`](../../src/core/llm/registry.rs) 已注册 id 对齐（`openai-responses`/`openai`），新增后端先在 registry 登记再进 catalog。
- `thinking` 格式不必进 catalog：现有 [`thinking_policy::thinking_format_for_model`](../../src/core/llm/thinking_policy.rs) 已按 model 名推断（`deepseek-*`/`qwen*`/`doubao*`），catalog 只在特例时覆盖。
- **catalog miss 策略分两路**：显式选择的 model（会话 override / 未来 `/model` / TUI / scene 键）→ 结构化错误；仅在**旧配置兼容模式**（尚未迁移到 catalog、仍走今天的 `[llm].provider/api_base` 语义）才构造 Legacy Entry，保证 G7。

#### 4.2.2 W1-2 Resolver + scene（技术要点）

```text
  resolve(scene, session_override) →
     1. model_id = override(会话) ?? scene_key(config) ?? default_model
     2. entry = catalog.lookup(model_id)
     3. 能力校验（W1-3）
     4. key = AuthStore.get(entry.provider)        // W2-1，首期回落 api_key_env
     5. provider_impl = resolve_llm(LlmConfig{ provider: entry.api, base_url: entry.base_url, ... })
     6. → ResolvedCall { provider_impl, model: entry.id, base_url, key, thinking_fmt }
```

- **优先级**（与 [`llm-multiprovider-integration.md`](llm-multiprovider-integration.md) 报告 §9.3 一致）：`会话持久化 model_override > scene 键 > default_model`；压缩路径通常只认 `compaction_model`，不受主模型已选 model 影响。
- `effective_model`（[`context.rs`](../../src/api/chat/context.rs)）逻辑并入 Resolver 的 main scene 分支，保持向后兼容。

#### 4.2.3 运行时用户入口与初始化向导（技术要点）

**当前事实（便于和目标对照）**：

- `tomcat chat` 的本地 slash-command 入口在 [`commands/parse.rs`](../../src/api/chat/commands/parse.rs)，当前只认 `/path`、`/help`、`/thinking`、`/ckpt`、`/restore`、`/plan`；**没有 `/model`**。
- 聊天启动横幅只会打印一次当前模型：[`run_loop/mod.rs`](../../src/api/chat/run_loop/mod.rs) `println!("tomcat 对话模式 (模型: {})", model);`
- 用户输入 prompt 当前只显示 mode：[`prompt.rs`](../../src/api/chat/prompt.rs) 目前是 `u[Chat]> ` / `u[Plan:planning]> `，**不显示当前对话 model**。
- 底层已经有 `SessionManager::switch_current_model(...)`（[`session_impl.rs`](../../src/core/session/manager/session_impl.rs)），会更新 `model_override` 并落一条 `model_change` transcript 事件；而 `model_override` 本身就放在 [`store.rs::SessionEntry`](../../src/core/session/store.rs) 的 `sessions.json` 里，所以天然支持**程序重入 / resume 后仍生效**，只是目前**没有用户入口和 UI 呈现接它**。
- dispatcher 侧 `llm.setModel` 仍是 [`ops.rs`](../../src/ext/dispatcher/ops.rs) 的 **MVP stub**，不能作为产品化入口。
- `tomcat init` 现在只会写默认 `provider = "openai-responses"` + `default_model = DEFAULT_LLM_MODEL`，并提示输入 **`OPENAI_API_KEY`**（[`api/cli/init.rs`](../../src/api/cli/init.rs)）；**不能**选模型、也**不能**配置 DeepSeek 等额外 provider key。

**设计落地**：

```text
运行时（chat）：
  /model current
    → 展示 current effective model
    → 同时展示 resolved { api, provider, base_url, key_source }

  /model list [--provider <vendor>] [--all]
    → 读 ModelCatalog，列出可选模型（可标记 current/default）

  /model use <model_id>
    → catalog.lookup(model_id)
       命中    → SessionManager::switch_current_model(Some(entry.provider), Some(model_id))
               → 持久化到 SessionEntry.model_override（sessions.json）
               → 追加 model_change transcript 事件
               → 程序重入 / resume 后仍恢复为该会话 model
       未命中  → 结构化错误「模型未收录，请补 models.toml」

  CLI UI
    → 启动/恢复横幅显示 current conversation model
    → 输入 prompt 持续带 model 标识（例如 u[Chat|gpt-5.4]> ）
    → /model use 成功后下一次 prompt 立即刷新

初始化（tomcat init）：
  [1/3] 环境初始化（保留）
  [2/3] 资源检查（保留）
  [3/3] 模型与凭证配置（扩展）
       A. 从内置 catalog 选择 default_model（可跳过）
       B. 根据选中的 model 推导 provider/api
       C. 询问并写入所需 key（如 OPENAI_API_KEY / DEEPSEEK_API_KEY）
       D. 可选补充额外 provider 凭证（供后续切换/降级）
```

**为什么 chat 内要有 `/model`，而不是只靠改配置**：

- `/model use <id>` 是**当前对话级、持久化**的选择：写入 `SessionEntry.model_override`，程序重入 / resume 后仍生效；
- `[llm].default_model` 仍是**全局默认值**：只在“当前会话尚未选过模型”时生效，和会话已选 model 不冲突；
- `config_set("llm.default_model", ...)` 就算未来允许，也属于**全局默认值**修改，不适合作为当前对话的选模型入口；
- 不保留 `/model clear`：它会重新引入“临时覆盖/清空覆盖”的双重心智；若需要回到默认，可显式 `/model use <default_model>` 或新开会话；
- 这与 pi-mono 的 `setModel` / `cycleModel` 同类：用户运行时切换模型，产品层负责校验 auth 与 catalog 命中。

**为什么 init 要先选 model，再推导 provider/key**：

- 本方案是 **model-first**，不是 provider-first；
- 用户真正懂的是“我要用 `gpt-5.4` 还是 `deepseek-reasoner`”，而不是“我要配 `openai-responses` 还是 `openai`”；
- 选 model 后，由 catalog 推导 `api/base_url/provider`，再提示需要哪把 key，能把首次上手成本降到最低。

---

## 5. 协议（MUST，涉及新类型）

### 5.1 `ModelEntry`（catalog 单一事实源）

单一事实源：计划 `src/core/llm/catalog.rs`。

| 字段 | 类型 | 必填 | 默认 | 说明 | 说人话 |
|------|------|------|------|------|--------|
| `id` | `String` | 是 | — | 模型 id，如 `gpt-5.4` / `deepseek-reasoner` | 模型叫啥。 |
| `api` | `String` | 否 | `LlmConfig.provider` | 协议族；与 [`registry.rs`](../../src/core/llm/registry.rs) 注册 id 对齐 | 走哪条 wire。 |
| `provider` | `String` | 否 | 由 `id` 推断 | **逻辑厂商名**（auth/分类/展示用），**不**喂给 `resolve_llm`；例如 `provider="deepseek"` 仍可搭配 `api="openai"` 命中 `OpenAiProvider` | 哪家的钥匙/标签。 |
| `base_url` | `Option<String>` | 否 | `LlmConfig.api_base` | 该模型 endpoint | 打到哪个地址。 |
| `capabilities` | `Capabilities` | 否 | `{tools:true}` | `vision`/`files`/`tools`/`reasoning` 能力位 | 支持图片/附件/工具/推理不。 |
| `context_window` | `Option<u32>` | 否 | `ContextConfig.context_window` | 上下文窗口 | 能塞多少 token。 |
| `cost` | `Option<Cost>` | 否 | `None` | `input_per_mtok`/`output_per_mtok`，仅计量展示 | 多少钱（可选）。 |
| `thinking_format` | `Option<String>` | 否 | 按 model 名推断 | 覆盖 [`thinking_policy`](../../src/core/llm/thinking_policy.rs) | 思考字段怎么发（特例才填）。 |

三态语义：`api`/`base_url` 缺省 = 「用 `LlmConfig` 的全局值」（保证 G7）；显式值 = 覆盖。

### 5.2 `LlmScene` 与 scene 键

单一事实源：计划 `src/core/llm/resolver.rs` + [`infra/config/types/context.rs`](../../src/infra/config/types/context.rs)（沿用 `compaction_model` 落点）。

| scene | 配置键 | 回落 | 说人话 |
|-------|--------|------|--------|
| `Main` | `[llm] default_model` + 会话 `model_override`（持久化） | — | 主对话。 |
| `Compaction` | `[context] compaction_model` | `default_model` | 压缩摘要（已落地）。 |
| `Vision` | `[llm] vision_model`（计划） | `Main`（若支持 vision）否则 Err | 看图。 |
| `Title` | `[llm] title_model`（计划） | `compaction_model` | 起标题（便宜模型）。 |

### 5.3 `ResolvedCall`（调用样例）

```jsonc
// resolve(scene=Vision, override=None) 的概念输出
{
  "provider_impl": "openai-responses",   // 实际是 Arc<dyn LlmProvider>
  "model": "gpt-5.4",
  "base_url": "https://api.openai.com",
  "key_source": "OPENAI_API_KEY",
  "thinking_fmt": "openai",
  "capabilities": { "vision": true, "files": true, "tools": true, "reasoning": true }
}
```

调用方仍组**一份** `ChatRequest`（[`types.rs`](../../src/core/llm/types.rs)）；`ResolvedCall.provider_impl` 决定 wire 翻译（与 [`llm-multiprovider-integration.md`](llm-multiprovider-integration.md) §3.2.1 一致，**不**组两套请求）。

---

## 6. 文件职责总览（One-Glance Map，MUST）

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│ src/api/chat/context.rs        ChatContext::from_config                        │
│   · 注入 LlmResolver（替代固定 Arc<dyn LlmProvider>）【新】                    │
│   · effective_model 逻辑并入 Resolver main scene【改】                         │
├──────────────────────────────────────────────────────────────────────────────┤
│ src/core/llm/resolver.rs       【新】LlmScene · ResolvedCall · trait LlmResolver│
│   · resolve(scene, override) → ResolvedCall                                    │
│   · 能力校验（W1-3）· FallbackChain + unhealthy cache（W2-2）                  │
├──────────────────────────────────────────────────────────────────────────────┤
│ src/core/llm/catalog.rs        【新】ModelCatalog · ModelEntry · Capabilities  │
│   · 内置表 + merge(models.toml) · lookup(model_id) → ModelEntry                │
├──────────────────────────────────────────────────────────────────────────────┤
│ src/core/llm/auth.rs           【新, W2-1】AuthStore · provider→key/OAuth      │
│   · get(provider) → Credential · 缺失可读错误                                  │
├──────────────────────────────────────────────────────────────────────────────┤
│ src/core/llm/registry.rs       resolve_llm(LlmConfig) → Arc<dyn LlmProvider>   │
│   · 【不改】Resolver 用 ModelEntry.api 组 LlmConfig 后调用                      │
├──────────────────────────────────────────────────────────────────────────────┤
│ src/core/llm/thinking_policy.rs thinking_format_for_model（已按 model 推断）   │
│   · 【不改】catalog.thinking_format 仅在特例覆盖                                │
├──────────────────────────────────────────────────────────────────────────────┤
│ src/core/llm/openai.rs / openai_responses/  Provider impl                      │
│   · 【小改】错误分类回传（402/连接失败）供 FallbackChain 判定                   │
├──────────────────────────────────────────────────────────────────────────────┤
│ src/core/llm/token_usage.rs    SessionTokenUsage                               │
│   · 【改, W2-3】按 model/provider 维度聚合 usage                               │
├──────────────────────────────────────────────────────────────────────────────┤
│ src/infra/config/types/{llm.rs,context.rs}  LlmConfig · ContextConfig          │
│   · 【小改】增 vision_model/title_model 等 scene 键（全 serde default）         │
├──────────────────────────────────────────────────────────────────────────────┤
│ src/api/chat/{commands/cmd_model.rs,prompt.rs,run_loop/mod.rs}                │
│   · 【新/改】/model 入口 + 启动/恢复横幅与 prompt 显示当前会话 model            │
└──────────────────────────────────────────────────────────────────────────────┘
  配套测试：src/core/llm/tests/{catalog_test,resolver_test,auth_test}.rs + src/api/chat/commands/tests/cmd_model_*（按 UNIT_TEST_LAYOUT_SPEC）
```

**阅读顺序（说人话）**：`ChatContext` 不再在启动时锁死一个 provider，而是注入 **Resolver**；Resolver 每次调用查 **catalog** 拿到 `ModelEntry`，做能力校验、从 **AuthStore** 取 key，再用 `entry.api` 组一份 `LlmConfig` 调既有 **`resolve_llm`** 拿到 provider impl；失败时按 **FallbackChain** 换下一个 entry。`thinking_policy`/`registry` 基本不动，体现「加两层、不动主骨架」。

---

## 7. 调度时序（SHOULD）

### 7.1 主对话一次调用（含降级）

```text
用户输入 / AgentLoop
   │ 组 ChatRequest（一份，messages+tools）
   ▼
ChatContext → LlmResolver.resolve(scene=Main, session.model_override[persisted])
   │  1 model_id = override ?? default_model
   │  2 entry = catalog.lookup(model_id)         [catalog.rs]
   │  3 能力校验（main 无附件则跳过）
   │  4 key = AuthStore.get(entry.provider)       [auth.rs]    缺失 → Err(可读)
   │  5 provider = resolve_llm(LlmConfig{provider: entry.api, base_url: entry.base_url,…})
   ▼
ResolvedCall → provider.chat_stream(ChatRequest{ model: entry.id, … })
   │
   ├─ Ok(stream) → StreamEvent…（与今天一致，进 CLI/transcript）
   └─ Err(402 | 连接失败)
        │  mark_unhealthy(entry.provider, now)    [resolver.rs, TTL]
        ▼
      FallbackChain.next(entry) → 回到步骤 2（下一个 model/后端）
        │  链尽 → Err（终局，向上抛 LlmError）
```

每条迁移的发布/订阅点：`mark_unhealthy` 写入 Resolver 内 `Mutex<HashMap>`，下次 `resolve` 步骤 2 前读取并跳过 TTL 内 provider。

### 7.2 压缩场景（与主对话解耦，已部分落地）

```text
usage_ratio 触发 preheat
   → LlmResolver.resolve(scene=Compaction)   // 只认 compaction_model，不受主模型 override 影响
   → provider.chat(ChatRequest{ model: compaction_model, tools: None, stream:false })
   → 摘要文本 → transcript（见 context-management.md）
```

---

## 8. 状态机（SHOULD）：FallbackChain provider 健康度

```text
┌──────────┐  resolve 命中     ┌──────────┐  chat Ok      ┌──────────┐
│ unknown  │─────────────────▶│ in_use   │──────────────▶│ healthy  │
└──────────┘                  └────┬─────┘               └────┬─────┘
                                   │ 402/连接失败              │ 下次 resolve
                                   ▼                          │ 命中
                              ┌──────────┐  TTL 内 resolve     │
                              │unhealthy │◀────────────────────┘（被跳过）
                              └────┬─────┘
                                   │ TTL 过期
                                   ▼
                              ┌──────────┐
                              │ unknown  │（重新可选）
                              └──────────┘
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| unknown/healthy | resolve 命中 | in_use | — | 这次就用它。 |
| in_use | chat Ok | healthy | 清除 unhealthy 标记 | 跑通了，正常。 |
| in_use | 402/连接失败 | unhealthy | 记 `Instant`，触发 FallbackChain | 挂了，晾一会换下一家。 |
| unhealthy | TTL 内被 resolve | unhealthy（跳过） | Resolver 选下一个 entry | 还在冷却期就先不选它。 |
| unhealthy | TTL 过期 | unknown | 移除标记 | 冷却完可以再试。 |

---

## 9. 配置与环境变量（SHOULD）

总则：**env > config > 默认**。所有新键 `#[serde(default)]`，缺省退化为现状（G7）。

| 变量 / 键 | 取值 | 含义 | 优先级 | 说人话 |
|-----------|------|------|--------|--------|
| `[llm] default_model` | model id | 主对话默认模型（现状） | config | 主模型。 |
| 会话 `model_override` | model id | 当前对话已选模型（持久化于 `sessions.json`，现状） | 高于 default | 当前对话固定用谁。 |
| `[context] compaction_model` | model id | 压缩摘要模型（现状） | config | 压缩用谁。 |
| `[llm] vision_model` | model id | vision 场景模型（计划） | config | 看图用谁。 |
| `[llm] title_model` | model id | 标题生成模型（计划） | config | 起标题用谁。 |
| `~/.tomcat/models.toml` | catalog 覆盖 | 用户自定义/覆盖模型条目（计划） | 合并入内置表 | 自己加模型。 |
| `[llm.fallback] chain` | `[model_id,…]` | 降级链（计划，W2-2） | config | 挂了按这个顺序换。 |
| `[llm.fallback] unhealthy_ttl_sec` | u64，默认 600 | unhealthy 冷却时长（计划） | config | 晾多久。 |
| `<PROVIDER>_API_KEY` | key | 按 provider 取（AuthStore，计划 W2-1） | env（最高） | 各家 key。 |
| `[llm] api_key_env` | env 名 | 单 key 兜底（现状） | config | 没配多 key 时用这个。 |
| `[llm] api_base_fallback` | url | 同模型换 base（现状） | config | 同模型换地址。 |
| `TOMCAT__LLM__VISION_MODEL` 等 | model id | env 覆盖 scene 键 | env | 环境变量直接改。 |

---

## 10. 错误模型 / 降级（SHOULD）

```text
正常                         → StreamEvent…（与现状一致）
能力不匹配（如 text 模型收图） → 调用前 Err(可重路由)：结构化「provider/model 不支持 vision，建议改用 X」（G3/R7）
凭证缺失                     → 调用前 Err(可读)：「请设置 <PROVIDER>_API_KEY」（G5/R5）
402 余额 / 连接失败           → 不直接 Err；mark_unhealthy + FallbackChain（G4/R6）
                              链尽 → 终局 LlmError（向上抛，转 transcript/CLI）
429 限流                     → 走现有重试（retry_count）；与 codex 一致不盲目算 failover
catalog 缺失 model            → 显式选模：调用前 Err「模型未收录，请补 models.toml」；旧配置兼容：LlmNotice + Legacy Entry（G7）
不可重试（参数/4xx 非 402）   → Err（既有 LlmError 分类）
```

错误风格复用既有 [`infra/error/llm.rs`](../../src/infra/error/llm.rs) 与 [`openai.rs::reject_multimodal_parts`](../../src/core/llm/openai.rs) 的结构化拒绝。

---

## 11. 测试矩阵（MUST）

| 维度 | 用例 / 编号 | 状态 | 说人话 |
|------|-------------|------|--------|
| 单元 · Catalog | `core::llm::catalog::tests::{resolve_known_model,merge_user_override,missing_explicit_model_errors,legacy_fallback_entry}` | PENDING | 清单查得对、覆盖生效、显式 miss 报错、兼容模式兜底。 |
| 单元 · Resolver | `core::llm::resolver::tests::{scene_fallback_to_main,override_priority,reject_vision_on_text_model}` | PENDING | 场景/优先级/能力校验。 |
| 单元 · Fallback | `core::llm::resolver::tests::{fallback_on_402,unhealthy_ttl_skip,chain_exhausted_errs}` | PENDING | 降级与冷却。 |
| 单元 · Auth | `core::llm::auth::tests::{per_provider_env,missing_key_message}` | PENDING | 多 key 与缺失提示。 |
| 单元 · Usage | `core::llm::token_usage::tests::aggregate_by_model` | PENDING | 按模型聚合计量。 |
| 集成 | `tests/llm_tests.rs::{catalog_resolver_e2e,scene_compaction_uses_low_cost_model}`（mock provider） | PENDING | 拼起来跑一遍。 |
| 集成 · 现状兼容 | 既有 `tests/plan_e2e_with_mock_llm_tests.rs` 等**仍绿**（未配 catalog 时行为不变，G7） | PENDING | 老配置照常跑。 |
| 观察指标 | G1（选 model 即用）/G2（场景模型）/G3（能力校验）/G4（降级）/G5（多 key）/G6（计量）/G7（兼容）各对应上表用例 | PENDING | 吹的牛都有测试钉。 |
| 文档 | 本文定稿 + 与 [`llm-multiprovider-integration.md`](llm-multiprovider-integration.md) §6.5.2、报告 §9.3 交叉引用同步 | ✅ 2026-06-01 | 字和代码别两张皮。 |

> 状态均为 PENDING：本文是**方案**，实现按 Wave 在看板拆任务卡后落测试函数名。

---

## 12. 风险与应对（MUST）

| 风险 | 影响 | 应对（具体动作） | 说人话 |
|------|------|--------------------|--------|
| **Catalog 与上游漂移** | 中：内置表里 model id/能力过期 | 显式选中的 miss 直接报「模型未收录」并提示补 `models.toml`；旧配置兼容路径仍可走 Legacy Entry；用户可即时覆盖 `models.toml` | 清单别写死太细，但也别静默配错。 |
| **schema 膨胀复发** | 中：又往 `LlmConfig` 加厂商字段 | 守住 R2：per-model 元数据只进 catalog；CI 加 review 约定（§6.5.2） | 别又把配置塞爆。 |
| **per-call 解析开销** | 低：每次 resolve 查表 | catalog 用 `HashMap` + `Arc` 缓存 provider impl（同 (api,base_url,key) 复用 `Arc<dyn LlmProvider>`） | 别每次都重建客户端。 |
| **FallbackChain 误降级** | 中：把可重试错误当致命换家 | 严格分类：仅 402/连接失败/能力不匹配触发换家；429 走 retry_count（对齐 codex `retry_429:false` 语义） | 分清是限流还是真挂了。 |
| **unhealthy 误杀** | 低：偶发失败拉黑 10 分钟 | TTL 可配（默认 600s，对齐 hermes）；成功立即清标记 | 冷却时间能调。 |
| **凭证泄漏到日志/transcript** | 高 | trace 只记 `key_source`（env 名）不记值；`without_completion_metadata` 出站剥离（现状已有） | 日志别打 key。 |
| **破坏现状** | 高：迁移成本 | 全键 serde default；catalog/scene/auth 缺省时严格等价单 provider 路径；现状集成测试纳入回归（§11） | 老用户零改动。 |
| **能力位不准** | 中：标了 vision 实则不支持 | 校验失败的结构化错误同时引导「改用 X」；保留上游 4xx 作最终兜底（不静默吞） | 标错了也有上游兜底。 |

---

## 13. 历史决策 / 跨文档修订（SHOULD）

- ~~路由按 provider 名硬编码（现状 `from_config` 固定一个 `Arc<dyn LlmProvider>`）~~ → **否**：改为 catalog `api` 路由 + per-call Resolver（R1/R3），理由见 §2.2、§4.1。
- ~~每接一家厂商往 `LlmConfig` 加专属字段~~ → **否**：守 §6.5.2 稳定 schema，元数据进 catalog（R2）。
- ~~失败仅「同模型换 base」（`api_base_fallback`）~~ → **保留并扩展**：上层叠加 FallbackChain「换模型/换厂商」+ unhealthy cache（R6）。
- **岔路 B（全局 IR）** 仍不采纳（§2.3），与 [`llm-multiprovider-integration.md` §6.2/§6.3](llm-multiprovider-integration.md) 一致。

**跨文档修订意图**：

- [`llm-multiprovider-integration.md`](llm-multiprovider-integration.md)：本文是其 §2.4「场景化扩展惯例」与报告 §9.3 的**实现化展开**；落地后应在该文 §6 增一行回链本文，并把「`vision_model`/`pdf_model` 建议键」标注为「详见多 LLM 产品化方案」。
- [`context-management.md`](context-management.md)：`compaction_model` 被纳入本文 `LlmScene::Compaction`，语义不变，仅说明「压缩是 scene 的一个特例」。

---

## 一句话总结

多 LLM 产品化的核心不是再写一个 OpenAI-compatible adapter，而是补齐 **Model Catalog（以 `api` 为路由键、与厂商正交）+ per-call Resolver（场景化模型 + 能力校验 + 降级）+ AuthStore（多凭证）+ 可观测计量** 四层——这正是 pi-mono / openclaw / pi_agent_rust / hermes / codex 五仓的共同收敛形态；tomcat 的 `ChatMessage`/`resolve_llm`/`thinking_policy`/continuity 主骨架已就位，本方案只在其上加「选模型 → 自动补齐 provider/wire/鉴权/降级」的产品层，且全程 serde default 兜底、老配置零改动。
