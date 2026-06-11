# `image_to_video` 工具：Seedance 2.0 图生视频、异步轮询与可取消长任务

本文档是内置 **`image_to_video`** 工具的技术方案（OpenSpec **B 类**：`docs/architecture/tools/`）。与兄弟文档 [`generate_image.md`](generate_image.md) **拆为两份独立满额文档**——图片是同步 HTTP、结果可回灌模型；**视频是异步任务（提交→轮询→下载）、单次耗时 60–120s+、且结果无法回灌**（Tomcat 无 `InputVideo` content part）。两者风险表、状态机、PR 节奏完全不同，必须分篇。

**文首声明（路线图口吻，与 [`web_fetch.md`](web_fetch.md) 一致）**：

- 本工具**尚未落地**；全文描述 **PR-IV-A/B/D 合入后的目标态行为**。凡与 `src/` 现状不一致处，以**本文为设计真相、落地 PR 为最终真相**，实现期就地更新状态列（[ARCHITECTURE_SPEC §14 No-Stale](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)）。
- §10 测试矩阵全部 **PENDING**。
- 后端选型已确认：**国内火山方舟 Ark**（`https://ark.cn-beijing.volces.com/api/v3`，模型 `doubao-seedance-2-0-260128` / `-fast-`），异步任务 API。
- 写作约定见 [`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)。

> **核心约束（视频区别于图片的三件事，全篇围绕展开）**：
>
> 1. **异步任务**：Ark 是 `POST tasks` 拿 `task_id` → `GET tasks/{id}` 轮询到 `succeeded` → 拿 `video_url`。Tomcat **没有后台任务工具框架**（[`BashTaskRegistry`](../../../src/core/tools/primitive/types.rs) 仅服务 bash），故 MVP 在 `handle_image_to_video` 内**同步阻塞轮询**，但**必须监听 [`ctx.cancel`](../../../src/core/agent_loop/tool_exec/mod.rs)（`CancellationToken`）+ 墙钟上限**，避免长任务挂死 agent。
> 2. **24 小时下载窗口**：`video_url` 指向火山对象存储，**24h 后 403 失效**。必须在 `succeeded` 后**立即下载落盘** `tool-results/<task_id>.mp4`。
> 3. **不可回灌**：[`ChatMessageContentPart`](../../../src/core/llm/types.rs) 只有 `InputText`/`InputImage`/`InputFile`，**无视频类型**。故视频**只返回落盘路径 + 元数据 JSON**（`model_text`），`follow_up_parts` **恒为空**——与 [`generate_image`](generate_image.md) 的回灌路径相反，与 [`web_fetch`](web_fetch.md) 二进制落盘一致。
>
> **复用现状**：`reqwest 0.12`（[`Cargo.toml`](../../../Cargo.toml)）；[`http_client.rs`](../../../src/core/llm/http_client.rs) timeout/proxy；落盘 `resolve_agent_trail_dir(cfg)?/tool-results/`；配置子表加 [`infra/config/types/tools.rs`](../../../src/infra/config/types/tools.rs)；`tool_exec` 分发同 [`web_fetch`](web_fetch.md)（`branches/image_to_video.rs` + `mod.rs` match + `ToolExecCtx` 注入 runtime）。

---

## 先看总图：image_to_video 异步三段链

```text
  LLM tool_call: image_to_video { prompt, image, last_image?, duration, resolution }
        │
        ▼
┌──────────────────────────────────────────────────────────────────────┐
│ tool_exec/mod.rs  match "image_to_video"                               │
│   → branches/image_to_video.rs::handle_image_to_video                  │
│   （返回 String —— follow_up_parts 恒空，视频不可回灌）                  │
└───────────────────────────────┬────────────────────────────────────────┘
                                │
                                ▼
┌──────────────────────────────────────────────────────────────────────┐
│ core/tools/video_gen/  VideoGenRuntime                                 │
│                                                                        │
│  ① 提交  POST {ark}/contents/generations/tasks                          │
│     body.content = [ {type:text}, {type:image_url, image_url:{url}} ]   │
│     image 本地路径 → base64 data URL；http(s) → 直接用                   │
│     → task_id                                                          │
│                                                                        │
│  ② 轮询  loop: GET {ark}/contents/generations/tasks/{task_id}           │
│     status: queued/running → 指数退避 sleep（监听 ctx.cancel + 墙钟）    │
│            succeeded → break；failed/expired/cancelled → Err            │
│                                                                        │
│  ③ 下载  GET video_url（24h 内）→ fs::write tool-results/<task_id>.mp4   │
└───────────────────────────────┬────────────────────────────────────────┘
                                │ model_text (JSON)
                                ▼
┌──────────────────────────────────────────────────────────────────────┐
│ tool 消息：{ video_url, persisted_output_path, duration,               │
│             resolution, usage_tokens, task_id }                         │
│ （无 follow_up_parts —— 模型靠路径知晓，用户/UI 自行播放）              │
└──────────────────────────────────────────────────────────────────────┘
```

**看图顺序（说人话）**：模型喊 `image_to_video` 给一张首帧图 + 运镜描述；runtime 先把图（本地路径转 base64、URL 直接用）和 prompt 拼进 Ark 的 `content` 数组**提交任务**拿 `task_id`；然后**轮询**任务状态，每次 sleep 都盯着取消信号和墙钟上限，直到 `succeeded`；拿到 24 小时有效的 `video_url` **立刻下载**落盘到 `tool-results/`。最后只给模型一段 JSON 回执（路径 + 时长 + token 消耗），**不回灌视频本体**——因为对话协议里压根没有「视频」这种 part，模型知道路径就行，真正播放交给用户/UI。

---

## 1. 目标与设计原则

**一句话**：让模型一句 `prompt` + 一张首帧图，拿到一段**真实生成的视频文件**——接火山方舟 Seedance 2.0 异步 API（提交→轮询→下载），结果落盘 `tool-results/<task_id>.mp4`，回执只给路径 + 元数据；轮询全程可取消、有墙钟上限；视频**不回灌**模型。

### 1.1 观察指标表（与 §11 验收一一对应）

| 目标 | 观察指标（落地后用户可感知） | 说人话 |
|------|------------------------------|--------|
| G1 图生视频闭环 | catalog 注册 `image_to_video`；`POST {ark}/contents/generations/tasks`（content 含 text + image_url）→ 拿 `task_id` → 轮询 `succeeded` → 下载 `video_url` 落盘 `tool-results/<task_id>.mp4` | 给图 + 一句话，回一段视频文件。 |
| G2 输入图双形态 | `image` 为本地路径 → 读字节转 `data:image/...;base64,...`；为 `http(s)://` → 直接放 `image_url.url` | 本地图自动转码，网图直接用。 |
| G3 可取消长任务 | 轮询循环每次 sleep 用 `tokio::select!` 同时等 `ctx.cancel`；取消 → 立即返回 `Err("cancelled")`，不再轮询 | 用户按停，立刻不等了。 |
| G4 墙钟封顶 | 轮询累计超 `poll_max_wait_ms`（默认 600s）→ 返回结构化超时 `Err`，**不**无限等 | 等太久就放弃，不卡死。 |
| G5 24h 下载 | `succeeded` 后立即 `GET video_url` 落盘；不依赖 `video_url` 长期有效 | 拿到链接马上下，别等它过期。 |
| G6 不可回灌 | `handle_image_to_video` 返回纯 `String`；`follow_up_parts` 恒空；`model_text` 给路径 + 元数据 | 视频只给路径，不塞进对话。 |

### 1.2 非目标

| 非目标 | 推给 | 说人话 |
|--------|------|--------|
| 文生图 / 图片编辑 | [`generate_image.md`](generate_image.md) | 图片是另一条同步链路。 |
| 视频回灌模型（让模型"看"视频帧） | 后续增强（可走 `return_last_frame` 取尾帧当 `InputImage`） | 现在协议没视频 part，先只给路径。 |
| 真正的后台任务 / detach（不阻塞 agent turn） | 后续迭代（需 `BashTaskRegistry` 式视频任务框架） | MVP 先同步阻塞轮询，可取消即可。 |
| 文生视频（纯 text，无首帧图） | 本工具聚焦图生视频；纯文生视频后续可加 `text_to_video` | 先做"给图动起来"。 |
| 多模态参考（参考视频 + 音频） | 后续（Ark 支持 video_url/audio_url，本期不接） | 先打通图生视频主路。 |
| 海外 BytePlus 线路（`dreamina-*`） | 后续（base_url + model 可配，留口子） | 先接国内火山方舟。 |
| webhook 回调（`callback_url`） | 后续（MVP 用轮询） | 先轮询，回调以后再说。 |

---

## 2. 竞品 / 选型对比

精读过 **codex / hermes-agent / openclaw / pi_agent_rust / pi-mono / QevosAgent / GenericAgent** 七仓的视频生成实现。结论先行：**仅 hermes-agent 与 openclaw 有「Agent 一等公民视频生成工具」**，且两家都用「提交任务 + 轮询」异步模式；**codex / pi_agent_rust / pi-mono / QevosAgent / GenericAgent 五仓无视频生成**（QevosAgent 仅 `load_video` 本地抽帧、GenericAgent 配了火山 Ark 但只用于 LLM chat）。Seedance 2.0 在 hermes-agent 的 FAL provider 与 openclaw 的 BytePlus provider 中均有出现，是直接对标对象。

### 2.1 视频生成工具的典型关切

```text
┌────────────────────────────────────────────────────────────────────────┐
│  本地 image_to_video 类工具通常要同时解决的五类问题                       │
├────────────────────┬─────────────────────────────────────────────────┤
│  异步任务编排       │  提交拿 task_id → 轮询状态 → 下载产物（非一锤子）    │
│  轮询/超时/取消     │  60-120s+ 长任务：退避轮询 + 墙钟上限 + 可中断       │
│  输入图形态         │  本地路径 base64 / 公网 URL；首帧 / 首尾帧           │
│  产物交付           │  video_url 易过期 → 必须下载落盘；不可回灌模型       │
│  阻塞 vs 后台       │  同步占住 turn / detach 后台 + 完成事件唤醒          │
└────────────────────┴─────────────────────────────────────────────────┘
```

### 2.2 常见实现横向对比

| 来源 / 形态 | 语言 | 工具名 | backend / API | 异步模式 | 轮询/超时/取消 | 产物交付 | 我们借鉴的点 |
|-------------|------|--------|---------------|----------|----------------|----------|--------------|
| **openclaw** | TS | `video_generate` | Runway / Sora / **BytePlus Seedance** 等多 provider（[`extensions/byteplus/video-generation-provider.ts`](../../../../openclaw/extensions/byteplus/video-generation-provider.ts)） | 提交 + 轮询（`pollProviderOperationJson`，[`extensions/runway/video-generation-provider.ts`](../../../../openclaw/extensions/runway/video-generation-provider.ts)） | poll 5s/2.5s、max 120 次、`createProviderOperationDeadline` 墙钟；任务系统可 cancel detached | `saveMediaBuffer` 落盘或 url-only；**后台任务 + 完成事件注入** | 退避轮询 + deadline 上限 + 落盘；`action=generate\|status\|list`；first_frame/last_frame |
| **hermes-agent** | Python | `video_generate` | FAL（**seedance-2.0** / veo / kling / pixverse，[`plugins/video_gen/fal/__init__.py`](../../../../hermes-agent/plugins/video_gen/fal/__init__.py)）/ xAI | `is_async=False` 同步阻塞；provider 内部轮询（xAI 显式 submit+poll 5s，[`plugins/video_gen/xai/__init__.py`](../../../../hermes-agent/plugins/video_gen/xai/__init__.py)） | xAI 240s timeout；无主动 cancel | JSON `{success, video: url/路径}`；**不回灌** | 同步阻塞 + 内部轮询模式（与 Tomcat 无后台框架契合）；`image_url` 有无决定 t2v/i2v |
| **codex** | Rust | —（无视频） | 仅图片 | — | — | — | 无视频；仅图片 extension 可参考 HTTP 封装 |
| **pi_agent_rust** | Rust | —（无） | — | — | — | — | 无视频 |
| **pi-mono** | TS | —（无） | — | — | — | — | 无视频 |
| **QevosAgent** | Python | —（无生成） | `load_video` 本地抽帧 | shell bg 任务有 `peek` 轮询（[`agent/core/async_manager.py`](../../../../QevosAgent/agent/core/async_manager.py)） | `wait_secs` 轮询 + `threading.Timer` 超时 + `job_cancel` | — | 异步任务的 submit/poll/cancel/timeout 心智模型可参考 |
| **GenericAgent** | Python | —（无） | 火山 Ark 仅配 LLM chat（`doubao-seed-code`，[`assets/configure_mykey.py`](../../../../GenericAgent/assets/configure_mykey.py)） | — | `code_run` `time.sleep(1)` 轮询 + kill | — | 证明火山 Ark base `https://ark.cn-beijing.volces.com/api/v3` 已是通用配置 |

### 2.3 Seedance 2.0 火山方舟 API 速查（官网调研）

> 来源：火山方舟 Ark 官方 + [Seedance 2.0 API 文档调研](https://apidog.com/blog/seedance-2-0-api/)、[接口AI 文档中心](https://jiekou.ai/docs/models/reference-seedance-2.0)。

| 项 | 值 |
|----|-----|
| Base URL（国内） | `https://ark.cn-beijing.volces.com/api/v3` |
| 提交任务 | `POST /contents/generations/tasks` → `{ "id": "cgt-..." }` |
| 查询任务 | `GET /contents/generations/tasks/{task_id}` → `{ status, content: { video_url }, usage }` |
| 模型 ID | `doubao-seedance-2-0-260128`（标准）/ `doubao-seedance-2-0-fast-260128`（快速，更便宜） |
| 鉴权 | `Authorization: Bearer {ARK_API_KEY}` |
| 图生视频 | `content` 数组加 `{ "type": "image_url", "image_url": { "url": "<URL 或 data:...base64>" } }`，作首帧 |
| 首尾帧 | 给两个 image_url（首帧、尾帧各一），模型推断为首尾帧模式 |
| 状态机 | `queued → running → succeeded` / `failed` / `expired` / `cancelled` |
| 关键参数 | `duration`(4-15s, 默认5)、`resolution`(480p/720p/1080p)、`ratio`(默认 adaptive)、`watermark`、`return_last_frame` |
| 产物失效 | `video_url` **24h 后 403**，必须立即下载 |
| 错误 | 429（并发限制，退避重试）、`failed`（内容/输入问题）、`expired`（排队超时） |

### 2.4 落地选型决策表（维度取舍）

**代码落点、交付物、阶段**见 **[§2.5](#25-实施点路线图)**，与 [`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) **§4.1 / §4.2** 分工一致。**`决策`** 列钉本行裁决结论。

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| **Backend / API** | 哪个视频后端、哪条线路 | **采用** 火山方舟 Ark Seedance 2.0（国内 `doubao-seedance-2-0-*`）单 backend。 | openclaw [`extensions/byteplus/video-generation-provider.ts`](../../../../openclaw/extensions/byteplus/video-generation-provider.ts)（Seedance）+ GenericAgent [`assets/configure_mykey.py`](../../../../GenericAgent/assets/configure_mykey.py)（Ark base 已验证） | 设计：reqwest 直打 Ark `contents/generations/tasks`；理由：用户已选国内线路，Seedance 2.0 是 SOTA 且支持首尾帧/参考 | × openclaw 多 provider 插件（MVP 不需要 Runway/Sora）；× 海外 BytePlus `dreamina-*`（线路与 model ID 不同，留 base_url 可配后置） | 先接火山方舟的 Seedance，一条路打通。 |
| **异步编排** | 同步阻塞 vs 后台 detach | **采用** `handle_image_to_video` 内**同步阻塞轮询**（提交→轮询→下载一气呵成）。 | hermes [`plugins/video_gen/xai/__init__.py`](../../../../hermes-agent/plugins/video_gen/xai/__init__.py)（`is_async=False` + 内部 submit/poll）+ 本仓 `tool_exec` 同步签名 | 设计：在 tool 执行内跑完三段；理由：Tomcat 无后台任务工具框架（`BashTaskRegistry` 仅 bash），同步阻塞最小改动且 hermes 同款 | × openclaw 后台任务 + 完成事件（需新建视频任务账本 + 唤醒机制，超出 MVP）；× 提交后立即返回 task_id 让模型自己轮（模型无轮询工具） | MVP 就在工具里等它跑完，别另起后台框架。 |
| **轮询 / 超时 / 取消** | 长任务不能挂死 agent | **采用** 指数退避轮询 + `tokio::select!` 监听 `ctx.cancel` + `poll_max_wait_ms` 墙钟封顶。 | openclaw [`createProviderOperationDeadline`](../../../../openclaw/extensions/runway/video-generation-provider.ts) + QevosAgent [`async_manager.py::peek`](../../../../QevosAgent/agent/core/async_manager.py) + 本仓 [`ctx.cancel`](../../../src/core/agent_loop/tool_exec/mod.rs) | 设计：10s 起退避封顶 60s、总墙钟 600s、每次 sleep `select!` 等取消；理由：60-120s+ 任务必须可中断 + 有上限，否则卡死整轮 | × 无上限死等（卡死 agent）；× 固定间隔不退避（高频打爆 429） | 退避着轮询，用户能停、太久也自动放弃。 |
| **输入图形态** | 本地路径 vs 公网 URL | **采用** 本地路径读字节转 `data:...base64`、`http(s)://` 直接用。 | Seedance API `image_url.url` 接受 URL 或 base64 + 本仓 [`ChatMessageContentPart::image_b64`](../../../src/core/llm/types.rs)（路径读字节先例） | 设计：判断 `image` 前缀分流；理由：Agent 手里多是本地图，但也要支持网图（用户已选 both_input） | × 仅 URL（本地图用不了）；× 仅 base64（网图还得先下载，多此一举） | 本地图自动转码，网图直接塞。 |
| **产物交付** | video_url 易过期怎么办 | **采用** `succeeded` 后立即下载落盘 `tool-results/<task_id>.mp4`，回执给路径 + video_url。 | openclaw `saveMediaBuffer` + hermes `save_*_video` + 本仓 [`web_fetch.md`](web_fetch.md) 二进制落盘约定 | 设计：`GET video_url` 流式写盘；理由：24h 后 403，不落盘等于白生成 | × 只返 video_url 不下载（24h 后失效，用户拿不到）；× 不落盘塞 base64 进上下文（视频几 MB 直接炸） | 拿到链接马上下到本地，别等它过期。 |
| **结果回传** | 视频能否回灌模型 | **采用** 纯 `String` 回执（`follow_up_parts` 恒空），不回灌。 | 本仓 [`llm/types.rs::ChatMessageContentPart`](../../../src/core/llm/types.rs)（无视频 variant）+ hermes 视频纯 JSON 返回 | 设计：`model_text` 给 path/video_url/元数据；理由：对话协议无 `InputVideo` part，硬塞会破坏 schema | × 模仿 [`generate_image`](generate_image.md) 回灌（协议不支持视频）；× 抽帧当图回灌（MVP 不做，列为后续 `return_last_frame` 增强） | 视频只给路径，不像图片那样塞回对话。 |
| **错误 / 限速归一化** | 429 / failed / expired 怎么处理 | **采用** 429 退避重试（在墙钟内）；`failed`/`expired`/`cancelled` → 结构化 `Err`。 | hermes/openclaw 轮询识别终态 + [`ARCHITECTURE_SPEC §10`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) | 设计：429 不算终态继续退避；终态失败给清晰 Err 文案；理由：429 是并发限制（等等就好），failed 是真失败（让模型改 prompt） | × 429 直接 Err（其实重试就行）；× failed 静默吞（模型不知情） | 限流就多等会儿，真失败就老实报错。 |

### 2.5 实施点（路线图）

**实施顺序**：**① PR-IV-A**（catalog 注册 + schema + tool_exec 占位）→ **② PR-IV-B**（`VideoGenRuntime`：提交/轮询/下载三段 + 输入图分流 + 取消/墙钟 + 配置）→ **③ PR-IV-D**（错误归一化 + 429 退避 + 终态映射打磨）。**先注册再补 backend、最后打磨错误**。

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
| --- | --- | --- | --- | --- |
| **PR-IV-A**（catalog + 占位） | **交付物**：`image_to_video` catalog 条目（`scope=Read`、`read_only=false`、`category=Exec`、`plan_only=false`）；`image_to_video_parameters()` schema（`prompt`/`image` 必填、`last_image`/`duration`/`resolution`/`ratio`）；占位 err。**落地点**：catalog / tool_exec match | [`catalog.rs`](../../../src/core/tools/contract/catalog.rs)、[`tool_exec/mod.rs`](../../../src/core/agent_loop/tool_exec/mod.rs)（match 增臂 + `ToolExecCtx` 加 `video_gen_runtime`）、新 [`branches/image_to_video.rs`](../../../src/core/agent_loop/tool_exec/branches/mod.rs) | `catalog_test::image_to_video_registered`、`submodules_test::tool_exec_image_to_video_requires_runtime_injection`（PENDING） | 先把名字 / schema / 占位接好。 |
| **PR-IV-B**（runtime 三段 + 取消 + 配置） | **交付物**：提交 `POST tasks`、退避轮询 `GET tasks/{id}`（`select!` 监听 cancel + `poll_max_wait_ms`）、下载 `video_url` 落盘；本地图 base64 / URL 直用；`ToolsVideoGenConfig`。**落地点**：`core/tools/video_gen/*`、`ToolsConfig`、`context.rs` 装配 | 新模块 `core/tools/video_gen/{mod,types}.rs`、[`infra/config/types/tools.rs`](../../../src/infra/config/types/tools.rs)、[`api/chat/context.rs`](../../../src/api/chat/context.rs)、[`accessors.rs`](../../../src/core/agent_loop/accessors.rs)；复用 [`http_client.rs`](../../../src/core/llm/http_client.rs) | `video_gen_test::{submit_returns_task_id,poll_until_succeeded,download_persists_mp4,local_image_to_base64,url_image_passthrough,cancel_stops_polling,wall_clock_timeout}`（PENDING） | 真打三段、可取消、配置齐。 |
| **PR-IV-D**（错误归一化 + 退避） | **交付物**：429 退避（墙钟内重试）、`failed`/`expired`/`cancelled` 终态 → 结构化 Err；凭证脱敏。**落地点**：`video_gen/mod.rs` 轮询分支 | [`core/tools/video_gen/mod.rs`](../../../src/core/tools/video_gen/) 轮询循环 | `video_gen_test::{http_429_backoff_retries,status_failed_returns_err,status_expired_returns_err,error_redacts_token}`（PENDING） | 限流重试、真失败报错、不漏 key。 |

下文按实施点展开**技术要点与示意图**；**交付边界与代码落点仍以表为准**。

#### 2.5.1 PR-IV-A：catalog 注册与 schema

- **交付**：[`BUILTIN_TOOL_CATALOG`](../../../src/core/tools/contract/catalog.rs) 增 `name = "image_to_video"`；`image_to_video_parameters()` 用 `object_schema(...)` 输出 schema；`tool_exec` 加 `match "image_to_video"` 占位分支。
- **catalog 元数据**：`scope = PermissionScope::Read`（出网类）、`read_only = false`（写盘 + 计费 + 长耗时）、`destructive = false`、`category = Some(ToolCategory::Exec)`、`plan_only = false`。description 须**显式提示模型**「本工具耗时较长（约 1-2 分钟），会阻塞当前轮；结果是视频文件路径，不会作为图片回灌」。

**说人话**：先把名字、参数和「这工具慢、结果是文件路径」的说明放进去。

#### 2.5.2 PR-IV-B：VideoGenRuntime（提交 / 轮询 / 下载 + 取消）

- **① 提交**：

  ```text
  POST {base}/contents/generations/tasks   (Bearer ARK_API_KEY)
  {
    "model": "doubao-seedance-2-0-260128",
    "content": [
      { "type": "text", "text": "<prompt>" },
      { "type": "image_url", "image_url": { "url": "<首帧: data URL 或 http(s)>" } }
      // last_image 存在时再加一个 image_url（尾帧）
    ],
    "duration": 5, "resolution": "720p", "ratio": "adaptive", "watermark": false
  }
  → { "id": "cgt-..." }
  ```

- **输入图分流**：`image` 以 `http://`/`https://` 开头 → 直接放 `url`；否则视为本地路径 → `std::fs::read` + `base64` → `data:image/<ext>;base64,<...>`（MIME 由扩展名/magic 推断；预检大小，超 30MB 报错——Seedance 单图上限）。
- **② 轮询（核心：可取消 + 墙钟）**：

  ```text
  let deadline = now + poll_max_wait_ms;   // 默认 600s
  let mut wait = poll_interval_ms;          // 默认 10s
  loop {
      tokio::select! {
          _ = ctx.cancel.cancelled() => return Err("cancelled"),
          _ = tokio::time::sleep(wait) => {}
      }
      if now > deadline { return Err("video task wall-clock timeout"); }
      let task = GET {base}/contents/generations/tasks/{id};
      match task.status {
          "succeeded" => break task.content.video_url,
          "failed" | "expired" | "cancelled" => return Err(status),
          "queued" | "running" => { wait = min(wait * 2, poll_interval_cap_ms /*60s*/); }
          429 => { /* 不算终态，继续退避 */ }
      }
  }
  ```

- **③ 下载（24h 窗口）**：`GET video_url` 流式写 `resolve_agent_trail_dir(cfg)?.join("tool-results").join(format!("{task_id}.mp4"))`；下载失败（含 403 过期）→ `Err`，但回执仍带 `video_url` 供用户手动取。
- **鉴权/endpoint**：`ToolsVideoGenConfig.base_url` 默认 `https://ark.cn-beijing.volces.com/api/v3`；key 读 `ARK_API_KEY`（经 [`AuthStore`](../../../src/core/llm/auth.rs) 或直接 env）。

**说人话**：拼好图和文字提交拿号；然后退避着查状态，每次 sleep 都用 `select!` 盯着取消信号，超过 10 分钟就放弃；成了就立刻把视频下到本地。

#### 2.5.3 PR-IV-D：错误归一化与 429 退避

- **429**：并发限制，**不算终态**——在墙钟内继续退避重试（对齐官方文档「429 用指数退避」）。
- **终态失败**（`failed`/`expired`/`cancelled`）：返回结构化 `Err`，文案含 `task_id` + status + 可能的 reason，让模型据此改 prompt 或重试。
- **凭证脱敏**：所有 `Err` 文案对 `Bearer xxx` → `<redacted>`。

**说人话**：限流就多等等，真失败就给清楚的错误，错误信息里不带 key。

---

## 3. 术语统一

| 术语 | 语义（大白话） | 数据载体 | 行为约束 | 说人话 |
|------|----------------|----------|----------|--------|
| **`task_id`** | Ark 异步任务句柄 | 提交响应 `id`（`cgt-...`） | 提交后立即拿到；轮询/下载/落盘文件名都用它 | 生成任务的单号。 |
| **轮询（poll）** | 反复查任务状态 | `GET tasks/{task_id}` | 指数退避（10s 起、封顶 60s）；终态退出 | 不停问"好了没"。 |
| **墙钟上限** | 轮询总时长封顶 | `ToolsVideoGenConfig.poll_max_wait_ms` | 默认 600s；超时返回 `Err`，不无限等 | 最多等 10 分钟。 |
| **`ctx.cancel`** | 用户/系统取消信号 | [`CancellationToken`](../../../src/core/agent_loop/tool_exec/mod.rs) | 每次 sleep 用 `tokio::select!` 监听；触发即 `Err("cancelled")` | 按停就立刻不等。 |
| **首帧 / 尾帧** | 图生视频的起止画面 | `content` 数组的 image_url（1 个=首帧，2 个=首尾帧） | `image` 必填作首帧；`last_image` 可选作尾帧 | 视频从哪张图开始/结束。 |
| **`image` 分流** | 本地路径转 base64 / URL 直用 | `VideoGenArgs.image: String` | `http(s)://` → 直接 url；否则读字节转 `data:...base64` | 本地图转码、网图直接用。 |
| **`video_url`** | 生成视频的临时下载地址 | 任务 `content.video_url` | **24h 后 403**；`succeeded` 后立即下载落盘 | 视频链接，限时 24 小时。 |
| **`persisted_output_path`** | 视频落盘绝对路径 | `tool-results/<task_id>.mp4` | 与 [`web_fetch`](web_fetch.md) 二进制落盘同目录 | 视频存在本地哪。 |
| **不可回灌** | 视频不作为多模态 part 回模型 | `follow_up_parts` 恒空 | 协议无 `InputVideo`；只在 `model_text` 给路径 | 视频不塞回对话，只给路径。 |

**「LLM 收到 tool 结果后」**：指 **`tool_exec` 已把 `model_text`（含路径 + video_url + 元数据）序列化为 tool 消息**、写入会话历史、即将进入下一轮模型推理之前。**本工具无紧随的 user 多模态消息**（区别于 [`generate_image`](generate_image.md)）。

---

## 4. 协议（入参 / 出参 / Schema）

**单一事实源**：

- JSON Schema（模型可见）：[`catalog.rs::image_to_video_parameters`](../../../src/core/tools/contract/catalog.rs)（PR-IV-A 添加）→ [`docs/tool-catalog.md`](../../tool-catalog.md) 自动派生。
- Rust 类型：`core/tools/video_gen/types.rs`（PR-IV-B 新增）的 `VideoGenArgs` / `VideoGenOutput`。

### 4.1 入参（工具 arguments）

| 字段 | JSON 类型 | 必填 | 默认 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|------|----------|------|--------|
| `prompt` | string | **是** | — | 全部 | 运镜 / 动作 / 内容描述 | 想让画面怎么动。 |
| `image` | string | **是** | — | 全部 | 首帧图：本地路径或 `http(s)://` URL | 从哪张图开始动。 |
| `last_image` | string \| null | 否 | null | 首尾帧 | 尾帧图；给了则进首尾帧模式 | 想指定结束画面就给。 |
| `duration` | integer | 否 | 5 | 全部 | 视频时长秒数，范围 4–15 | 多少秒。 |
| `resolution` | enum `480p`\|`720p`\|`1080p` | 否 | `720p` | 全部 | 分辨率；1080p 仅标准版 | 多清晰，越高越贵越慢。 |
| `ratio` | string \| null | 否 | `adaptive` | 全部 | 宽高比（`adaptive` 跟随首帧） | 画面比例，默认跟图。 |

**`last_image` 三态语义**：

- 缺省 / 显式 `null`：单首帧图生视频。
- 显式路径/URL：首尾帧模式（`content` 加第二个 image_url）。

### 4.2 出参（Rust：`VideoGenOutput`，序列化进 `model_text`）

| 字段 | 类型 | 说明 | 说人话 |
|------|------|------|--------|
| `persisted_output_path` | `String` | 视频落盘绝对路径（`tool-results/<task_id>.mp4`） | 视频存哪了。 |
| `video_url` | `String` | Ark 临时地址（24h 失效，仅供用户手动取/排错） | 临时链接，会过期。 |
| `task_id` | `String` | Ark 任务号 | 出问题报这个号。 |
| `duration` | `u32` | 实际时长秒 | 多少秒。 |
| `resolution` | `String` | 实际分辨率 | 多清晰。 |
| `usage_tokens` | `Option<u64>` | `usage.completion_tokens`（成本核算） | 花了多少 token。 |
| `warnings` | `Vec<String>` | 非致命提示（如下载失败但 url 可用） | 有啥小提醒。 |

> **`follow_up_parts` 恒为空**——视频不进对话多模态通道。模型据 `persisted_output_path` 知晓结果，UI/用户负责播放。

### 4.3 调用样例（jsonc）

**本地图生视频**：

```jsonc
{ "prompt": "镜头缓缓推进，人物转头微笑", "image": "/Users/me/photo.jpg", "duration": 5, "resolution": "720p" }
```

**网图首尾帧**：

```jsonc
{ "prompt": "花苞绽放成全开", "image": "https://x.com/bud.jpg", "last_image": "https://x.com/bloom.jpg", "duration": 8, "ratio": "adaptive" }
```

**典型 `model_text`（成功）**：

```jsonc
{
  "persisted_output_path": "/Users/me/.tomcat/agents/abc-123/tool-results/cgt-2026xxxx.mp4",
  "video_url": "https://ark-content.volces.com/.../output.mp4?expires=...",
  "task_id": "cgt-2026xxxxxxxx-xxxx",
  "duration": 5,
  "resolution": "720p",
  "usage_tokens": 102960,
  "warnings": []
}
```

---

## 5. One-Glance Map（文件职责总览）

```text
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/tools/contract/catalog.rs                                        │
│  • BUILTIN_TOOL_CATALOG: name = "image_to_video"                           │
│  • image_to_video_parameters(): prompt + image + last_image? + duration ...│
│  • description 提示"耗时约 1-2 分钟、结果是视频文件路径、不回灌"            │
└───────────────────────────────┬────────────────────────────────────────────┘
        │ visible_tools_for_mode_with_policy 过滤后给 LLM
        ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/agent_loop/tool_exec/  (目录模块)                                 │
│  • mod.rs::execute_tool_tuple_full → match "image_to_video"                 │
│  • mod.rs::ToolExecCtx 增 video_gen_runtime 字段 + 透传 cancel              │
│  • branches/image_to_video.rs::handle_image_to_video                        │
│      → 返回 String（follow_up_parts 恒空）                                  │
│      → 透传 ctx.cancel 给 runtime                                           │
└───────────────────────────────┬────────────────────────────────────────────┘
                                │
                                ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/tools/video_gen/                                                 │
│  ├ mod.rs  VideoGenRuntime                                                 │
│  │   ① submit()   POST contents/generations/tasks → task_id                │
│  │   ② poll()     GET tasks/{id} 退避循环 + select!(ctx.cancel) + 墙钟      │
│  │   ③ download() GET video_url → fs::write tool-results/<task_id>.mp4      │
│  │   image 分流：本地 read+base64 / http(s) 直用                            │
│  └ types.rs   VideoGenArgs / VideoGenOutput / TaskStatus                   │
└───────────────────────────────┬────────────────────────────────────────────┘
        │ 复用                     │ 复用
        ▼                         ▼
┌──────────────────────────┐  ┌──────────────────────────────────────────┐
│ src/core/llm/             │  │ src/infra/                                │
│ ├ http_client.rs          │  │ • resolve_agent_trail_dir(cfg)            │
│ │  build_http_client      │  │     /tool-results/<task_id>.mp4           │
│ └ auth.rs  AuthStore       │  │   (与 bash / web_fetch 同落盘框架)        │
│    ARK_API_KEY            │  └──────────────────────────────────────────┘
└──────────────────────────┘
        │
        ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/infra/config/types/tools.rs                                           │
│  • ToolsConfig 加 ToolsVideoGenConfig                                       │
│    { base_url, api_key_env, model, poll_interval_ms,                       │
│      poll_interval_cap_ms, poll_max_wait_ms, timeout_ms }                  │
│  src/api/chat/context.rs           • 构造 VideoGenRuntime 注入 ChatContext  │
│  src/core/agent_loop/accessors.rs  • runtime + cancel 透传进 ToolExecCtx    │
└────────────────────────────────────────────────────────────────────────────┘

  + tests:
    src/core/tools/video_gen/tests/        (submit/poll/download/取消/超时/分流 mock HTTP)
    src/core/agent_loop/tool_exec/branches/ (image_to_video 路由 + cancel 透传单测)
    tests/image_to_video_tool_tests.rs      (public roundtrip + env-gated live smoke)
```

**阅读顺序（说人话）**：模型先在 **catalog** 看到 `image_to_video` 与「慢、给路径」提示；调起后 **`tool_exec`** 路由到 **`handle_image_to_video`** 并把 `ctx.cancel` 透传下去；**`video_gen/mod`** 跑提交→轮询→下载三段，轮询时用 `select!` 同时等取消和退避 sleep；视频下到 **`tool-results/`** 后，只把路径 + 元数据 JSON 作为 `model_text` 回给模型，**没有**紧随的多模态消息。配置走 `[tools.video_gen]`。

---

## 6. 调度时序（运行时图）

```text
LLM     tool_exec   handle_image_to_video   VideoGenRuntime    Ark API        网络/对象存储
 │          │              │                      │               │               │
 │ image_to_video         │                      │               │               │
 │─────────▶│ parse args   │                      │               │               │
 │          │─────────────▶│ runtime.run(args, ctx.cancel)        │               │
 │          │              │─────────────────────▶│ image 分流(本地→base64/URL直用) │
 │          │              │                      │ ① POST tasks ─▶│               │
 │          │              │                      │◀── task_id ───│               │
 │          │              │                      │ ② loop:                       │
 │          │              │                      │   select!{ cancel | sleep }    │
 │          │              │                      │   GET tasks/{id} ─▶│           │
 │          │              │                      │   ◀── status ─────│           │
 │          │              │                      │   running → 退避继续           │
 │          │              │                      │   succeeded → break video_url  │
 │          │              │                      │ ③ GET video_url ──────────────▶│
 │          │              │                      │   ◀──── mp4 bytes ─────────────│
 │          │              │                      │   fs::write tool-results/*.mp4 │
 │          │              │◀─────────────────────│ VideoGenOutput                │
 │          │◀─────────────│ model_text (JSON, follow_up_parts 空)│               │
 │          │ tool 消息(path + video_url + 元数据) │               │               │
 │◀─────────│ 下一轮 messages：仅 tool result（无 user 多模态消息） │               │
```

**事件 / 状态迁移发布点**：

- `ctx.cancel.cancelled()` 触发（用户中断）→ 轮询 `select!` 立即落到 cancel 臂 → 返回 `Err("cancelled")`，不再发 HTTP。
- 墙钟到点（`now > deadline`）→ 返回 `Err("wall-clock timeout")`，回执提示 `task_id` 供用户后续手动查。
- `succeeded` → 立即下载（同一执行内，无独立事件）；下载完写盘后返回。
- **无** `follow_up_parts` 事件——不像 [`read`](read.md)/[`generate_image`](generate_image.md) 触发 `user_with_parts`。

---

## 7. 状态机（异步任务生命周期）

```text
┌──────────────┐ submit ok   ┌──────────────┐
│  init        │────────────▶│  submitted   │ (task_id)
└──────┬───────┘             └──────┬───────┘
       │ parse/分流 err              │
       ▼                            ▼
┌──────────────┐            ┌──────────────────────────────────────┐
│  arg_error   │            │  polling  ◀──── 退避 sleep（running） │
│  (Err)       │            └──────┬───────────────────────────────┘
└──────────────┘      ┌────────────┼───────────────┬───────────────┐
                      ▼            ▼               ▼               ▼
               ctx.cancel    超 poll_max_wait   succeeded      failed/expired/
                  │            │                 │             cancelled
                  ▼            ▼                 ▼               │
            ┌──────────┐ ┌──────────┐    ┌──────────────┐       ▼
            │cancelled │ │ timeout  │    │ downloading  │  ┌──────────┐
            │ (Err)    │ │ (Err)    │    └──────┬───────┘  │ task_err │
            └──────────┘ └──────────┘           │          │ (Err)    │
                                       ┌─────────┴───────┐  └──────────┘
                                       ▼                 ▼
                                 写盘成功            下载失败(含403)
                                 ┌──────────┐     ┌──────────────────┐
                                 │ done     │     │ done(warning:    │
                                 │ (路径+url)│     │  download_failed,│
                                 └──────────┘     │  仅给 url) (Ok)   │
                                                  └──────────────────┘
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `init` | 参数解析 + image 分流成功 | `submitted` | POST tasks 拿 task_id | 提交成功拿到单号。 |
| `init` | JSON 解析失败 / image 读失败 / 超 30MB | `arg_error` | 返回 `Err` | 参数或图有问题，直接报错。 |
| `submitted` | 进入轮询 | `polling` | — | 开始查状态。 |
| `polling` | status=`running`/`queued`（或 429） | `polling` | 退避 sleep（`select!` 等 cancel）；429 不计终态 | 还没好，等一会儿再问。 |
| `polling` | `ctx.cancel` 触发 | `cancelled` | 返回 `Err("cancelled")` | 用户按停了。 |
| `polling` | `now > deadline` | `timeout` | 返回 `Err("wall-clock timeout")`（含 task_id） | 等太久放弃。 |
| `polling` | status=`succeeded` | `downloading` | 取 `video_url` | 好了，去下载。 |
| `polling` | status=`failed`/`expired`/`cancelled` | `task_err` | 返回 `Err`（含 task_id + status） | 任务失败了。 |
| `downloading` | `GET video_url` 写盘成功 | `done(路径+url)` | 写 `tool-results/<task_id>.mp4` | 视频存好了。 |
| `downloading` | 下载失败（含 403 过期） | `done(warning)` | `warnings += "download_failed"`，仍返 `video_url`（Ok） | 没下下来但给链接，不算崩。 |

---

## 8. 配置与环境变量

**总则**：`env > config > 默认`。

| 来源 | 键 | 含义 | 默认 | 说人话 |
|------|-----|------|------|--------|
| `tomcat.config.toml` | `[tools.video_gen] base_url` | Ark API base | `https://ark.cn-beijing.volces.com/api/v3` | 火山方舟地址。 |
| `tomcat.config.toml` | `[tools.video_gen] api_key_env` | 读 key 的环境变量名 | `ARK_API_KEY` | 哪个环境变量存 key。 |
| `tomcat.config.toml` | `[tools.video_gen] model` | Seedance 模型 | `doubao-seedance-2-0-260128` | 标准版 / fast 版。 |
| `tomcat.config.toml` | `[tools.video_gen] poll_interval_ms` | 轮询起始间隔 | `10_000` | 多久查一次（起步）。 |
| `tomcat.config.toml` | `[tools.video_gen] poll_interval_cap_ms` | 退避封顶间隔 | `60_000` | 间隔最多退到多大。 |
| `tomcat.config.toml` | `[tools.video_gen] poll_max_wait_ms` | 轮询总墙钟上限 | `600_000`（10 min） | 最多等多久。 |
| `tomcat.config.toml` | `[tools.video_gen] timeout_ms` | 单次 HTTP 请求超时 | `60_000` | 单个请求多久算超时。 |
| 环境变量 | `ARK_API_KEY`（或 `api_key_env` 指定） | 火山方舟密钥 | — | 真正的密钥。 |
| 环境变量 | `TOMCAT__TOOLS__VIDEO_GEN__POLL_MAX_WAIT_MS` 等 | 上述字段运行时覆盖 | — | 容器里临时调。 |
| `tomcat.config.toml` | `[llm] proxy` | 共享出网代理 | `None` | 走企业代理在这配。 |

**用户在入参里没有可覆盖的 endpoint/key/model/poll**——模型只能动 `prompt`/`image`/`last_image`/`duration`/`resolution`/`ratio`。

---

## 9. 错误模型 / 截断 / 警告

```text
                    image_to_video 请求
                            │
        ┌───────────────────┼─────────────────────┐
        ▼                   ▼                     ▼
   参数解析失败         image 读失败/超30MB      未知 image scheme
   Err("invalid args") Err("image too large")   Err("bad image")
        │                   │                     │
        └───────────────────┴─────────────────────┘
                            │ ok
                            ▼
                    POST tasks（提交）
                            │
                ┌───────────┴───────────┐
                ▼                       ▼
           4xx 鉴权/参数            5xx / 超时
           Err(脱敏)               Err("submit failed")
                            │ 200 + task_id
                            ▼
                    轮询循环
        ┌───────────┬───────────┬───────────┬───────────┐
        ▼           ▼           ▼           ▼           ▼
   ctx.cancel   墙钟到点    429(并发)    failed/      succeeded
   Err          Err         退避重试     expired/      │
   ("cancelled")("timeout") (不计终态)   cancelled     ▼
                                        Err(status)  下载 video_url
                                                      │
                                          ┌───────────┴───────────┐
                                          ▼                       ▼
                                     写盘成功                 下载失败(403等)
                                     done(Ok)                warnings+="download_failed"
                                                             仍返 video_url (Ok)  ← 不抛 Err
```

**`tool_exec` 视角**：

- `Err(_)` → tool 消息文本为错误描述（致命类：参数 / image / 提交失败 / cancelled / timeout / task 终态失败）；**凭证脱敏**。
- `Ok(VideoGenOutput)` → JSON 回执；下载失败是**软降级**（带 `warnings` + `video_url`，不抛 `Err`，让用户/模型还能拿到链接）。

**§1 G1–G6 的「锁死它的测试」**全部位于 §10。

---

## 10. 测试矩阵（验收）

| 维度 | 用例（计划函数名） | 状态 | 说人话 |
|------|---------------------|------|--------|
| catalog 注册 | `catalog_test::image_to_video_registered`、`submodules_test::tool_exec_image_to_video_requires_runtime_injection` | PENDING | 名字注册了、未注入 runtime 给显式错误。 |
| 图生视频闭环 (G1) | `video_gen_test::{submit_returns_task_id,poll_until_succeeded,download_persists_mp4}`（mock Ark 三段） | PENDING | 提交→轮询→下载全链路。 |
| 输入图分流 (G2) | `video_gen_test::{local_image_to_base64_data_url,http_url_image_passthrough,image_over_30mb_errs}` | PENDING | 本地转码、网图直用、超大报错。 |
| 可取消 (G3) | `video_gen_test::cancel_stops_polling_immediately` | PENDING | 取消信号一来立刻停。 |
| 墙钟封顶 (G4) | `video_gen_test::wall_clock_timeout_returns_err` | PENDING | 超时返回 Err，不死等。 |
| 24h 下载 (G5) | `video_gen_test::{succeeded_triggers_download,download_403_degrades_with_url}` | PENDING | 成了就下；下载失败软降级。 |
| 不可回灌 (G6) | `image_to_video_test::follow_up_parts_always_empty` | PENDING | 返回值无多模态 part。 |
| 429 退避 | `video_gen_test::http_429_backoff_retries_within_deadline` | PENDING | 限流不算终态，退避重试。 |
| 终态失败 | `video_gen_test::{status_failed_returns_err,status_expired_returns_err,status_cancelled_returns_err}` | PENDING | 三种失败终态都报错。 |
| 凭证脱敏 | `video_gen_test::error_redacts_ark_token` | PENDING | 错误不漏 ARK_API_KEY。 |
| 首尾帧 | `video_gen_test::last_image_adds_second_image_url` | PENDING | 给尾帧时 content 加第二张图。 |
| 配置解析 | `infra/config/tests/tools_cfg_test::video_gen_fields` | PENDING | `[tools.video_gen]` 反序列化无丢字段。 |
| Public runtime / live smoke | `tests/image_to_video_tool_tests::{public_output_roundtrip,live_seedance_smoke}`（`PI_LIVE_VIDEO_GEN=1` gate） | PENDING（env-gated） | 上线前真打一次 Ark。 |
| E2E（live） | `E2E-IMAGE-TO-VIDEO-001`：真 Seedance 2.0 图生视频 + 落盘（env-gated；CI 默认跳） | PENDING（env-gated） | 整条链真跑一遍。 |

§1 观察指标 **G1–G6** 与本表逐行对应：G1↔闭环；G2↔输入图分流；G3↔可取消；G4↔墙钟；G5↔24h 下载；G6↔不可回灌。

---

## 11. 风险与应对

| 风险 | 影响 | 应对（具体动作） | 说人话 |
|------|------|------------------|--------|
| **长任务挂死 agent** | 视频 60-120s+，同步阻塞可能卡死整轮、用户停不下来 | 轮询每次 sleep 用 `tokio::select!` 监听 `ctx.cancel`（立即返回）+ `poll_max_wait_ms=600s` 墙钟封顶；超时/取消都返结构化 `Err` | 能随时停、最多等 10 分钟，不会卡死。 |
| **video_url 24h 过期** | 不及时下载等于白生成（且白计费） | `succeeded` 后**同一执行内立即下载**落盘；不把"持有 url 长期可用"作为假设；下载失败仍返 url 让用户手动救 | 拿到链接马上下，过期了至少把链接给你。 |
| **视频塞进上下文炸 token** | 几 MB 视频 base64 → 上下文爆 | **绝不**回灌视频；`model_text` 只给路径 + 元数据；`follow_up_parts` 恒空（协议也无 `InputVideo`） | 视频只给路径，绝不塞进对话。 |
| **ARK_API_KEY 泄漏到 transcript** | 凭证外泄 | 错误文案 redaction（`Bearer xxx`→`<redacted>`）；key 仅 runtime 内存读取 | 报错不抖 key。 |
| **429 并发限制** | 直接失败浪费一次提交 | 429 **不算终态**，墙钟内指数退避重试（对齐官方建议）；区别于 `failed`（真失败才 Err） | 限流就多等等再查。 |
| **failed / 内容审核拒绝** | 模型反复重试同输入 | `failed`/`expired` 归一为清晰 `Err`（含 task_id + status），让模型改 prompt/图，不自动重试 | 真失败让模型换输入，别死循环。 |
| **输入图过大 / 非法** | 提交失败或 multipart 爆 | `image` 本地路径读前 metadata 预检（Seedance 单图 30MB 上限）；非法 scheme/MIME → `Err` | 提交前先量图大小。 |
| **MVP 同步阻塞占住 turn** | 一个视频任务期间 agent 不能干别的 | 已登记为已知限制；后续可引入视频任务账本（仿 `BashTaskRegistry`）做后台 detach + 完成事件（openclaw 形态），本期不做 | 这一版生成视频时会占着，后面再做后台化。 |
| **落盘目录写满 / 不可写** | IO 失败 | `fs::write` 失败软降级（`warnings` + 返 url），不 panic；`tool-results/` 启动期已建 | 存不下就给链接，不崩。 |

---

## 12. 历史决策（已被本方案取代或待定）

- ~~做成 openclaw 式后台任务（detach + 完成事件唤醒会话）~~ → **否（本期）**：Tomcat 无视频任务框架（`BashTaskRegistry` 仅 bash），新建唤醒机制超出 MVP；改用 hermes 式**同步阻塞 + 内部轮询**，但补齐 `ctx.cancel` + 墙钟。后续迭代可再后台化。
- ~~提交后立即返回 task_id，让模型自己轮询~~ → **否**：模型没有「查视频任务」工具，且会把轮询逻辑泄漏给 LLM；改为工具内轮询到底。
- ~~把视频 base64 / 抽帧回灌给模型（仿 generate_image）~~ → **否**：[`ChatMessageContentPart`](../../../src/core/llm/types.rs) 无 `InputVideo` variant；视频太大且无意义；改为只给路径。`return_last_frame` 取尾帧当 `InputImage` 列为**后续增强**。
- ~~只返回 video_url 不下载~~ → **否**：`video_url` 24h 后 403，用户拿不到；改为 `succeeded` 后立即下载落盘。
- ~~固定间隔轮询~~ → **否**：高频打爆 429；改指数退避（10s 起、封顶 60s）。
- ~~接海外 BytePlus `dreamina-*` 线路~~ → **否（本期）**：用户已选国内火山方舟；`base_url` + `model` 配置化已留口子，后续可切。
- ~~多 provider 插件抽象（Runway/Sora/Kling）~~ → **否**：MVP 单 Seedance；多 vendor 后续迭代。

**跨文档修订**：

- 本文新增 catalog 条目 `image_to_video` 触及 [`docs/tool-catalog.md`](../../tool-catalog.md)（派生文档）——落地时运行 `UPDATE_TOOL_CATALOG=1 cargo run --bin gen-tool-catalog` 重生成，不手改。
- 与 [`generate_image.md`](generate_image.md) 共享 `tool-results/` 落盘约定与 `[tools.*]` 配置框架、`ToolExecCtx` 注入模式；但**回传方式相反**（图回灌、视频只给路径）、**同步性相反**（图同步一锤子、视频异步轮询）。
- 若未来引入「视频任务后台框架」，需新增架构文档并回链本文 §11「MVP 同步阻塞」风险行。

---

## 13. 关联文档

- 兄弟工具：[`generate_image.md`](generate_image.md)（文生图，同步可回灌）· [`web_fetch.md`](web_fetch.md)（外部 HTTP + 二进制落盘框架）· [`bash.md`](bash.md)（后台任务 / 取消传播参考）
- 取消传播：[`interrupt-and-cancellation.md`](../interrupt-and-cancellation.md)（`CancellationToken` 语义）
- 派生工具目录：[`tool-catalog.md`](../../tool-catalog.md)
- 规范：[`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)
- Seedance 2.0 API：火山方舟 Ark `https://ark.cn-beijing.volces.com/api/v3` · [API 调研](https://apidog.com/blog/seedance-2-0-api/) · [接口文档中心](https://jiekou.ai/docs/models/reference-seedance-2.0)
- 竞品源码：openclaw `extensions/byteplus/video-generation-provider.ts` · hermes-agent `plugins/video_gen/fal/__init__.py` · QevosAgent `agent/core/async_manager.py`

---

**一句话总结**：`image_to_video` 在 **`tool_exec`** 解参数并透传 `ctx.cancel`、在 **`video_gen/mod`** 跑火山方舟 Seedance 2.0 的**异步三段**（`POST tasks` 拿号 → 退避轮询 `GET tasks/{id}` 到 `succeeded` → 24h 内下载 `video_url` 落盘 `tool-results/`）；协议以 **`catalog.rs` + `video_gen/types.rs`** 为单一事实源，配置走 `[tools.video_gen]`（默认国内 Ark + `doubao-seedance-2-0-260128`）。三件事区别于 [`generate_image`](generate_image.md)：**异步轮询**（可取消 + 墙钟封顶）、**必须下载**（url 24h 失效）、**不可回灌**（协议无 `InputVideo`，只给路径）。**本工具尚未落地，全文为 PR-IV-A/B/D 目标态设计，§10 全部 PENDING；MVP 同步阻塞、后台化为后续增强。**