# `generate_image` 工具：文生图 / 图片编辑、base64 落盘与多模态回灌

本文档是内置 **`generate_image`** 工具的技术方案（OpenSpec **B 类**：`docs/architecture/tools/`）。与兄弟文档 [`image_to_video.md`](image_to_video.md) **拆为两份独立满额文档**——图片是同步 HTTP、可回灌模型；视频是异步轮询、不可回灌，PR 节奏与风险表互不依赖。共享的「外部生成类工具」骨架（reqwest 出网、`tool-results/` 落盘、`follow_up_parts` 多模态注入）在两篇各自完整书写，便于单篇审阅、单篇冻结。

**文首声明（口吻与 [`read.md`](read.md) 全篇闭环不同，与 [`web_fetch.md`](web_fetch.md) 路线图口吻一致）**：

- 本工具**尚未落地**；全文描述的是 **PR-IG-A/B/E 合入后的目标态行为**与代码锚点。凡与 `src/` 现状不一致处，以**本文为设计真相、以落地 PR 为最终真相**，实现期就地更新本文状态列（[ARCHITECTURE_SPEC §14 No-Stale](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)）。
- §10 测试矩阵全部为 **PENDING**（设计阶段未实现）。
- 写作约定见 [`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)（B 类：术语、调研、目标、**§4.1/§4.2**、One-Glance、测试、风险）。

> **实现锚点校准（2026-06 现状核对，落地以此为准）**：
>
> 1. **`tool_exec` 是目录模块**：中央分发在 [`tool_exec/mod.rs::execute_tool_tuple_full`](../../../src/core/agent_loop/tool_exec/mod.rs) 的 `match tc.name.as_str()`；每个工具的处理函数放 `tool_exec/branches/<tool>.rs`，并在 [`tool_exec/branches/mod.rs`](../../../src/core/agent_loop/tool_exec/branches/mod.rs) 注册。新增 `generate_image` 即：新建 `branches/generate_image.rs`（`handle_generate_image`）→ 在 `branches/mod.rs` 导出 → 在 `mod.rs` 的 match 增一臂 + 在 `ToolExecCtx` 注入 runtime。
> 2. **多模态回灌已有先例**：[`branches/read.rs::handle_read`](../../../src/core/agent_loop/tool_exec/branches/read.rs) 返回 `Result<(String, Vec<ChatMessageContentPart>), String>`，由 [`tool_dispatcher.rs`](../../../src/core/agent_loop/tool_dispatcher.rs) 把 `follow_up_parts` 拼成紧随的一条 user 消息。`generate_image` **直接复用这条管道**，是与 `web_fetch`（纯文本返回）的关键区别。
> 3. **图片 part 与 Files 上传已就绪**：[`ChatMessageContentPart::image_b64`](../../../src/core/llm/types.rs) 从磁盘路径构造 `InputImage`（含 4.5 MiB 上限）；[`openai_files.rs`](../../../src/core/llm/openai_files.rs) 的 `upload_decision_by_size` / `resolve_or_upload_path` 处理大图走 Files API。本工具复用，不重写。
> 4. **配置与落盘**：工具配置子表加在 [`infra/config/types/tools.rs`](../../../src/infra/config/types/tools.rs) 的 `ToolsConfig`；落盘目录复用 `resolve_agent_trail_dir(cfg)?.join("tool-results")`（与 [`bash`](bash.md) / [`web_fetch`](web_fetch.md) 同一注入框架）。
> 5. **HTTP / 鉴权已有底座**：`reqwest 0.12`（[`Cargo.toml`](../../../Cargo.toml) 已开 `json` / `stream` / **`multipart`** feature）；[`http_client.rs::build_http_client`](../../../src/core/llm/http_client.rs) 提供 timeout/proxy 构造；[`auth.rs::AuthStore`](../../../src/core/llm/auth.rs) 按 `{PROVIDER}_API_KEY` 读 key。

---

## 先看总图：generate_image 一条链

```text
  LLM tool_call: generate_image { prompt, action, image_path? }
        │
        ▼
┌──────────────────────────────────────────────────────────────┐
│ tool_exec/mod.rs  match "generate_image"                       │
│   → branches/generate_image.rs::handle_generate_image          │
│   （返回 (model_text, follow_up_parts) —— 与 read 同形）         │
└───────────────────────────────┬────────────────────────────────┘
                                │
                                ▼
┌──────────────────────────────────────────────────────────────┐
│ core/tools/image_gen/  ImageGenRuntime                         │
│   generate → POST {base}/images/generations  (JSON)            │
│   edit     → POST {base}/images/edits        (multipart)       │
│   解析 data[0].b64_json → decode → 落盘 tool-results/*.png      │
└───────────────────────────────┬────────────────────────────────┘
                                │
        ┌───────────────────────┴───────────────────────────┐
        ▼ model_text (JSON)                                  ▼ follow_up_parts
┌────────────────────────────┐         ┌───────────────────────────────────────┐
│ tool 消息：{ path, mime,    │         │ 复用 read 的注入逻辑：                  │
│ size, revised_prompt }      │         │  小图 → ChatMessageContentPart::image_b64│
│ + 占位句"图已存，见下条"     │         │  大图 → OpenAiFilesRuntime 上传得 file_id│
└────────────────────────────┘         └────────────────────┬──────────────────┘
                                                             ▼
                                          tool_dispatcher 追加一条
                                          ChatMessage::user_with_parts([InputImage])
                                                             │
                                                             ▼
                                          下一轮 LLM "看得见"刚生成的图
```

**看图顺序（说人话）**：模型喊一句 `generate_image`，`tool_exec` 路由到 `handle_generate_image`；runtime 按 `action` 决定打 `images/generations`（文生图，JSON）还是 `images/edits`（图片编辑，multipart 上传参考图），把返回的 base64 解码落盘到 `tool-results/`；然后**走两条出口**——一条是给模型的文本回执（路径 + 元数据），另一条是 `follow_up_parts`，复用 `read` 工具那套「小图 inline、大图传 Files API」逻辑，把刚生成的图作为紧随的一条 user 消息塞回对话，让模型下一轮真正「看见」它画出来的东西。这条**回灌**是本工具区别于 `web_fetch`（落盘只给路径）的核心设计。

---

## 1. 目标与设计原则

**一句话**：让模型一句 `prompt` 拿到一张**真实生成的图**——直连 OpenAI 兼容 Images API（`gpt-image-2`），支持文生图（`generate`）与图片编辑（`edit`）两种 action；结果 base64 解码落盘 `tool-results/`，并通过 `follow_up_parts` 把图回灌给模型（复用 [`read`](read.md) 多模态注入管道）；鉴权默认复用 LLM 那套 `api_base` + `OPENAI_API_KEY`，允许独立覆盖。

### 1.1 观察指标表（与 §10 验收一一对应）

| 目标 | 观察指标（落地后用户可感知） | 说人话 |
|------|------------------------------|--------|
| G1 文生图闭环 | catalog 注册 `generate_image`；`action=generate` + `prompt` → `POST {base}/images/generations` 200 → 解 `data[0].b64_json` → 落盘 `tool-results/genimg-<hash>.png`；`model_text` 含绝对路径 | 给句话，回一张图存到盘上。 |
| G2 图片编辑闭环 | `action=edit` + `image_path` → `POST {base}/images/edits`（multipart：原图 + prompt）→ 同样落盘 | 给张图 + 一句话，改出新图。 |
| G3 多模态回灌 | 生成成功后 `follow_up_parts` 非空：小图走 `image_b64` inline、大图走 `OpenAiFilesRuntime` 得 `file_id`；`tool_dispatcher` 追加一条 `user_with_parts` 消息；下一轮请求 messages 含该 `InputImage` | 模型下一轮能真看见自己画的图。 |
| G4 上下文不爆 | `model_text` 只含 `{path, mime, size, revised_prompt}` + 占位句，**不**把 base64 塞进 tool 文本消息；base64 仅经 `follow_up_parts` 通道（受 4.5 MiB 上限 + Files 上传分流） | 回执给路径不给一坨 base64。 |
| G5 鉴权零配置默认可用 | 不配 `[tools.image_gen]` 时，base 取 `llm.api_base`、key 取 `OPENAI_API_KEY`；配了则独立覆盖（`env > config > 默认`） | 已经配过聊天的人开箱即用。 |

### 1.2 非目标

| 非目标 | 推给 | 说人话 |
|--------|------|--------|
| 图生视频 / 视频生成 | [`image_to_video.md`](image_to_video.md) | 视频是另一条异步链路。 |
| 模型由 LLM 自选 backend / model | 用户配置（对齐 hermes / openclaw 设计） | 画图用哪个模型管理员定，省 token 也防计费意外。 |
| Provider 插件化多 backend 抽象 | 后续迭代（MVP 单 OpenAI 兼容 backend） | 先打通一条 OpenAI 路；多 vendor 以后再说。 |
| Codex 的 hosted `image_generation` 托管路径 | 不做 | 那条强制 ChatGPT/Codex 登录态，自托管用不了。 |
| 生成图自动写入用户工作区 | 不做（只落 `tool-results/`） | 不污染用户仓库；要用让模型自己 copy。 |
| 异步任务 / 轮询 | 不做（图片是同步 HTTP，一次拿到） | 画图是一锤子买卖，不需要轮询。 |

---

## 2. 竞品 / 选型对比

精读过 **codex / hermes-agent / openclaw / pi_agent_rust / pi-mono / QevosAgent / GenericAgent** 七仓的图片生成实现。结论先行：**4 仓无图片生成（pi-mono 仅库级 API、QevosAgent / GenericAgent / pi_agent_rust 核心无）**，真正有「Agent 一等公民文生图工具」的是 **codex（extension 路径）/ hermes-agent / openclaw**，三家的 backend 调用、结果回传方式各不相同，构成本方案选型的主要证据链。

### 2.1 图片生成工具的典型关切

```text
┌────────────────────────────────────────────────────────────────────────┐
│  本地 generate_image 类工具通常要同时解决的四类问题                       │
├────────────────────┬─────────────────────────────────────────────────┤
│  backend 形态       │  直连 Images API / 托管工具 / Provider 插件多 vendor │
│  鉴权与 endpoint    │  复用聊天 key / 独立 key；base_url 可配             │
│  结果回传           │  ① 纯 JSON 给 URL/路径  ② base64 回灌让模型"看见"    │
│  上下文 cost        │  base64 直塞会爆 token → 落盘 + 受控注入 / Files 上传 │
└────────────────────┴─────────────────────────────────────────────────┘
```

### 2.2 常见实现横向对比

| 来源 / 形态 | 语言 | 工具名 | backend / API | 结果回传 | 是否回灌模型 | 我们借鉴的点 |
|-------------|------|--------|---------------|----------|--------------|--------------|
| **codex**（extension） | Rust | `image_gen.imagegen` | 直连 `POST images/generations` + `images/edits`，模型 `gpt-image-2` | base64 落盘 `generated_images/`；返 turn item | **是**：`FunctionCallOutput` 带 `data:image/png;base64,...` InputImage（[`ext/image-generation/src/tool.rs`](../../../../codex/codex-rs/ext/image-generation/src/tool.rs)） | `generate`/`edit` 双 action + gpt-image-2 + data URL 回灌 |
| **hermes-agent** | Python | `image_generate` | Provider 插件（FAL 默认 / OpenAI `gpt-image-2` / xAI / Krea） | JSON `{success, image: url 或路径}`（[`tools/image_generation_tool.py`](../../../../hermes-agent/tools/image_generation_tool.py)） | **否**：纯文本 JSON，靠 agent markdown `![](url)` 展示 | 薄路由层 + 用户配 model（agent 不选）；schema 仅 `prompt`+`aspect_ratio` |
| **openclaw** | TS | `image_generate` | OpenAI 兼容 `images/generations` + `images/edits`（[`src/image-generation/openai-compatible-image-provider.ts`](../../../../openclaw/src/image-generation/openai-compatible-image-provider.ts)），多 provider | `b64_json`→Buffer→`saveMediaBuffer` 落盘 | **是**：后台任务完成后 `task_completion` 事件注入 `attachments`+`mediaUrls` | `action=generate\|edit\|list\|status`；b64_json 解码落盘；OpenAI 兼容路径 |
| **pi_agent_rust** | Rust | `generate_image`（JS 扩展） | Google Antigravity SSE（非 OpenAI Images） | base64 `ContentBlock::Image` + 可选落盘 | **是**：作为 `ToolResult` 多模态 part 回注（[`legacy_pi_mono_code/.../antigravity-image-gen.ts`](../../../../pi_agent_rust/legacy_pi_mono_code/pi-mono/packages/coding-agent/examples/extensions/antigravity-image-gen.ts)） | 「工具结果含 Image part → provider 转 base64 注入」与 Tomcat `read` 同构 |
| **pi-mono** | TS | —（无工具） | 库级 `generateImages()` 走 OpenRouter chat+modalities（[`packages/ai/src/images.ts`](../../../../pi-mono/packages/ai/src/images.ts)） | `AssistantImages.output` base64，不落盘 | **否**：明确「图像模型不参与 tool calling」 | 反面教材：库 API ≠ Agent 工具；要让 agent 用需自写工具包一层 |
| **QevosAgent** | Python | —（无） | 仅 `load_image` 读图输入 | — | — | 无生成；仅证明多模态 base64 注入是通用做法 |
| **GenericAgent** | Python | —（无） | 仅 vision 理解；Ark 仅配 LLM chat | — | — | 无生成 |

### 2.3 落地选型决策表（维度取舍）

**代码落点、交付物、阶段**见 **[§2.4](#24-实施点路线图)**，与 [`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) **§4.1 / §4.2** 分工一致。**`决策`** 列钉本行裁决结论。

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| **Backend 形态** | 直连 Images API / 托管工具 / Provider 插件多 vendor | **采用** 直连 OpenAI 兼容 Images API 单 backend；**拒绝** codex hosted 托管 + 多 vendor 插件抽象。 | codex [`ext/image-generation/src/backend.rs`](../../../../codex/codex-rs/ext/image-generation/src/backend.rs)（直连）+ 本仓 [`http_client.rs`](../../../src/core/llm/http_client.rs) | 设计：reqwest 直打 `images/generations`/`edits`；理由：与自托管目标一致、维护面最小、复用现有 LLM HTTP 底座 | × codex hosted（强制 ChatGPT 登录态，自托管不可用）；× hermes/openclaw Provider 插件（MVP 不需要多 vendor，徒增抽象） | 一条 reqwest 直打 OpenAI 画图接口；不搞托管也不搞插件全家桶。 |
| **工具表面（action）** | 只做文生图，还是含图片编辑 | **采用** `generate` + `edit` 双 action。 | codex [`ext/image-generation/src/tool.rs::ImagegenAction`](../../../../codex/codex-rs/ext/image-generation/src/tool.rs) + openclaw [`image-generate-tool.ts`](../../../../openclaw/src/agents/tools/image-generate-tool.ts) | 设计：`action` enum + `edit` 时必填 `image_path`；理由：codex/openclaw 都双 action，编辑是高频诉求（用户已选 t2i_edit） | × 仅 generate（hermes image 只做文生图，能力不全） | 既能凭空画，也能拿张图改。 |
| **结果回传** | JSON 给路径 vs base64 回灌模型 | **采用** 双出口：`model_text` 给路径元数据 + `follow_up_parts` 回灌图。 | pi_agent_rust [`antigravity-image-gen.ts`](../../../../pi_agent_rust/legacy_pi_mono_code/pi-mono/packages/coding-agent/examples/extensions/antigravity-image-gen.ts)（Image part 回注）+ 本仓 [`branches/read.rs`](../../../src/core/agent_loop/tool_exec/branches/read.rs) | 设计：复用 read 的 `(model_text, follow_up_parts)` 管道；理由：模型「看见」生成图才能迭代修改，是文生图的核心价值 | × hermes 纯 JSON（模型看不见图，只能靠 markdown 给用户）；× 把 base64 塞 tool 文本（撑爆上下文） | 既告诉模型存哪了，又让它下一轮真看见这张图。 |
| **base64 体积处理** | inline 注入 vs Files 上传 | **采用** 复用 `read` 分流：小图 `image_b64` inline、大图 `OpenAiFilesRuntime` 上传得 `file_id`。 | 本仓 [`branches/read.rs`](../../../src/core/agent_loop/tool_exec/branches/read.rs) + [`openai_files.rs::upload_decision_by_size`](../../../src/core/llm/openai_files.rs) | 设计：`upload_decision_by_size(original_size)` 决定 inline/上传；理由：4.5 MiB 上限 + Files 通道是现成的，零新代码 | × 一律 inline（大图超限失败）；× 一律落盘只给路径不回灌（丢了"看见"能力） | 小图直接塞、大图走 Files API，跟读图一个套路。 |
| **鉴权 / endpoint** | 复用聊天 key vs 独立配置 | **采用** 默认回退 `llm.api_base` + `OPENAI_API_KEY`，`[tools.image_gen]` 可独立覆盖。 | hermes [`tools/image_generation_tool.py`](../../../../hermes-agent/tools/image_generation_tool.py)（`OPENAI_API_KEY`）+ 本仓 [`config/types/llm.rs`](../../../src/infra/config/types/llm.rs) / [`auth.rs`](../../../src/core/llm/auth.rs) | 设计：config `base_url`/`api_key_env` 为 `Option`，None 时回退 llm；理由：多数人已配过聊天 key，开箱即用又留出分离口子 | × 强制独立配置（多配一份，体验差）；× 写死复用（无法让画图走官方、聊天走代理） | 不配就借用聊天那套，想分开再单独填。 |
| **落盘位置** | `tool-results/` vs 专用图片目录 | **采用** `resolve_agent_trail_dir()?/tool-results/`。 | codex `generated_images/` + 本仓 [`bash.md`](bash.md) / [`web_fetch.md`](web_fetch.md) `tool-results/` 约定 | 设计：与 bash 大输出、web_fetch 二进制同目录；理由：统一审计/清理口径，复用现成注入框架 | × codex 的 `generated_images/`（再起一个目录，与本仓现有约定割裂） | 跟 bash/web_fetch 存一块儿，不另起炉灶。 |
| **同步 vs 异步** | 是否需要任务轮询 | **采用** 同步 HTTP 阻塞拿结果，**无**轮询。 | hermes `is_async=False` + openclaw 图片 provider 多为同步 POST | 设计：一次 POST 等响应；理由：Images API 是同步返回 base64（不像视频要排队），无需 task_id | × 套用视频那套提交+轮询（图片场景多余复杂度） | 画图一次就回来，不用像视频那样等。 |

### 2.4 实施点（路线图）

**实施顺序**：**① PR-IG-A**（catalog 注册 + schema + tool_exec 占位）→ **② PR-IG-B**（`ImageGenRuntime`：generations/edits HTTP + base64 落盘 + 配置）→ **③ PR-IG-E**（`follow_up_parts` 多模态回灌，复用 read 注入逻辑）。**先注册再补 backend、最后接回灌**——避免后续 PR 反复改字面量与断言。

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
| --- | --- | --- | --- | --- |
| **PR-IG-A**（catalog + 占位） | **交付物**：`generate_image` catalog 条目（`scope=Read` 出网类、`read_only=false`、`category=Exec`）；`generate_image_parameters()` schema（`prompt` 必填、`action`/`image_path`/`size`/`quality`）；占位 err（未注入 runtime）。**落地点**：catalog / tool_exec match | [`catalog.rs`](../../../src/core/tools/contract/catalog.rs)、[`tool_exec/mod.rs`](../../../src/core/agent_loop/tool_exec/mod.rs)（match 增臂 + `ToolExecCtx` 加 `image_gen_runtime`）、新 [`tool_exec/branches/generate_image.rs`](../../../src/core/agent_loop/tool_exec/branches/mod.rs) | `catalog_test::generate_image_registered`、`submodules_test::tool_exec_generate_image_requires_runtime_injection`（PENDING） | 先把名字 / schema / 占位接好。 |
| **PR-IG-B**（runtime + HTTP + 落盘 + 配置） | **交付物**：`generate`→`POST {base}/images/generations`（JSON）、`edit`→`POST {base}/images/edits`（multipart）；`data[0].b64_json` 解码落盘 `tool-results/genimg-<hash>.png`；`ToolsImageGenConfig`（base_url/api_key_env/model/timeout，None 回退 llm）。**落地点**：`core/tools/image_gen/*`、`ToolsConfig`、`context.rs` 装配 | 新模块 `core/tools/image_gen/{mod,types}.rs`、[`infra/config/types/tools.rs`](../../../src/infra/config/types/tools.rs)、[`api/chat/context.rs`](../../../src/api/chat/context.rs)、[`accessors.rs`](../../../src/core/agent_loop/accessors.rs)；复用 [`http_client.rs`](../../../src/core/llm/http_client.rs) / [`auth.rs`](../../../src/core/llm/auth.rs) | `image_gen_test::{generate_decodes_b64_and_persists,edit_uploads_multipart,missing_key_returns_err,config_falls_back_to_llm}`（PENDING） | 真打接口、解 base64 落盘、配置回退。 |
| **PR-IG-E**（多模态回灌） | **交付物**：`handle_generate_image` 返回 `(model_text, follow_up_parts)`；小图 `image_b64` inline、大图 `OpenAiFilesRuntime` 上传；`model_text` 给路径 + 占位句。**落地点**：`branches/generate_image.rs` 复用 read 注入分支 | [`branches/generate_image.rs`](../../../src/core/agent_loop/tool_exec/branches/mod.rs)、复用 [`branches/read.rs`](../../../src/core/agent_loop/tool_exec/branches/read.rs) 的 `image_b64` / `OpenAiFilesRuntime` 路径、[`tool_dispatcher.rs`](../../../src/core/agent_loop/tool_dispatcher.rs) 已有 `user_with_parts` 拼接 | `generate_image_test::{small_image_inlined_as_part,large_image_uploaded_via_files,model_text_excludes_base64}`（PENDING） | 让模型下一轮真看见图。 |

下文按实施点展开**技术要点与示意图**；**交付边界与代码落点仍以表为准**。

#### 2.4.1 PR-IG-A：catalog 注册与 schema

- **交付**：[`BUILTIN_TOOL_CATALOG`](../../../src/core/tools/contract/catalog.rs) 增加 `name = "generate_image"`；`generate_image_parameters()` 用 `object_schema(...)`（与 `web_fetch_parameters` 同辅助函数）输出 schema；`tool_exec` 添加 `match "generate_image"` 占位分支（runtime 未注入 → friendly err）。
- **catalog 元数据**：`scope = PermissionScope::Read`（出网读取类，不写用户工作区）、`read_only = false`（确有副作用：写 `tool-results/` + 计费）、`destructive = false`、`category = Some(ToolCategory::Exec)`、`plan_only = false`（非 plan 模式可见）。
- **与后续 PR 衔接**：PR-IG-B 的 runtime 直接挂到本步占位分支；PR-IG-E 把分支返回值从 `String` 升为 `(String, Vec<ChatMessageContentPart>)` tuple。

**说人话**：先把名字、几个参数、权限元数据放进去；后面接 backend、接回灌不再动 catalog。

#### 2.4.2 PR-IG-B：ImageGenRuntime（HTTP + 落盘 + 配置）

- **HTTP 客户端**：复用 [`build_http_client`](../../../src/core/llm/http_client.rs)（timeout / proxy 与 LLM 共用一套构造），`Authorization: Bearer {key}`。
- **`generate`（文生图，JSON body）**：

  ```text
  POST {base_url}/images/generations
  { "model": "gpt-image-2", "prompt": "...", "size": "auto", "quality": "auto", "n": 1 }
  ```

- **`edit`（图片编辑，multipart）**：`reqwest::multipart::Form`，字段 `image`（读 `image_path` 字节）+ `prompt` + `model`；对齐 openclaw [`openai-compatible-image-provider.ts::appendImagesPath`](../../../../openclaw/src/image-generation/openai-compatible-image-provider.ts)（`/images/edits`）。`reqwest 0.12` 已开 `multipart` feature（[`Cargo.toml`](../../../Cargo.toml)）。
- **响应解析与落盘**：解 `data[0].b64_json`（对齐 codex [`codex-api/src/images.rs::ImageResponse`](../../../../codex/codex-rs/codex-api/src/images.rs) 与 openclaw [`image-assets.ts::generatedImageAssetFromBase64`](../../../../openclaw/src/image-generation/image-assets.ts)）→ `base64::decode` → `tokio::fs::write` 到 `resolve_agent_trail_dir(cfg)?.join("tool-results").join(format!("genimg-{hash}.png"))`，`hash = xxh32(prompt + ts)` 6 位 hex（复用 [`Cargo.toml`](../../../Cargo.toml) 已有 `xxhash-rust`）。
- **鉴权/endpoint 回退**：`ToolsImageGenConfig.base_url` / `api_key_env` 为 `Option`——None 时回退 `cfg.llm.api_base` / `OPENAI_API_KEY`（经 [`AuthStore`](../../../src/core/llm/auth.rs)）。

```text
  args { prompt, action, image_path? }
        │
        ▼
  action == "generate" ?
   ┌────┴─────┐
   yes        no(edit)
   │          │
   ▼          ▼
  JSON body  multipart(image bytes + prompt)
   │          │
   └────┬─────┘
        ▼
  POST {base}/images/{generations|edits}  (Bearer key)
        │
        ▼
  resp.data[0].b64_json → decode → fs::write(tool-results/genimg-<hash>.png)
        │
        ▼
  ImageGenOutput { path, mime, bytes, revised_prompt }
```

**说人话**：generate 走 JSON、edit 走 multipart 上传原图；返回的 base64 解码落盘；key 和地址不配就借聊天的。

#### 2.4.3 PR-IG-E：多模态回灌（复用 read 注入）

- **返回值升级**：`handle_generate_image` 从 `Result<String, String>` 升为 `Result<(String, Vec<ChatMessageContentPart>), String>`——**与 [`handle_read`](../../../src/core/agent_loop/tool_exec/branches/read.rs) 完全同形**。
- **注入分流**（逐字复用 read 的逻辑，不重写）：

  ```text
  落盘得 path + bytes
        │
        ▼
  upload_decision_by_size(bytes)  [openai_files.rs]
   ┌────┴────────────────┐
   InlinePreferred       UploadRequired / 偏好上传
   │                     │
   ▼                     ▼
  image_b64(mime,path)  OpenAiFilesRuntime.resolve_or_upload_path(..,Vision)
   │                     │  → image_file_id(meta.id)
   └────────┬────────────┘
            ▼
  follow_up_parts.push(InputImage)
  ```

- **`model_text` 内容**：`{ "path": "...", "mime": "image/png", "bytes": 1234567, "revised_prompt": "..." }` + 占位句「Generated image saved to <path>, attached as the next user message」（对齐 codex hosted 的 developer 提示语义，但本工具直接走 part 注入）。
- **`tool_dispatcher` 无需改**：[`tool_dispatcher.rs`](../../../src/core/agent_loop/tool_dispatcher.rs) 已对任何非空 `follow_up_parts` 追加 `ChatMessage::user_with_parts(...)`——`generate_image` 自动复用。

**说人话**：画完的图按大小决定 inline 还是传 Files API，塞进紧跟的一条 user 消息；这套搬运 read 工具的现成代码，dispatcher 那头一个字都不用改。

---

## 3. 术语统一

| 术语 | 语义（大白话） | 数据载体 | 行为约束 | 说人话 |
|------|----------------|----------|----------|--------|
| **`action`** | 这次是凭空画还是改图 | `GenerateImageArgs.action: enum` | `generate`（默认）→ `images/generations`；`edit` → `images/edits` 且 `image_path` 必填 | 画新的还是改老的。 |
| **`image_path`** | `edit` 模式的输入原图 | `GenerateImageArgs.image_path: Option<String>` | 仅 `edit` 必填；本地路径，读字节进 multipart；`generate` 时忽略 | 要改哪张图就给哪张的路径。 |
| **`b64_json`** | Images API 返回的图片 base64 | 响应 `data[].b64_json` | 不带 `data:` 前缀；解码后落盘，**不**进 tool 文本消息 | 接口回的一坨 base64，解码存盘。 |
| **`follow_up_parts`** | 回灌给模型的多模态片段 | `Vec<ChatMessageContentPart>` | 非空 → `tool_dispatcher` 追加一条 `user_with_parts`；图走 `InputImage`（inline 或 file_id） | 让模型下一轮看见图的那条附加消息。 |
| **`model_text`** | 给 LLM 的 tool 结果文本 | `ToolExecOutcome.model_text` | JSON `{path, mime, bytes, revised_prompt}` + 占位句；**禁含 base64** | 回执只给路径和说明，不给图本体。 |
| **base64 体积分流** | 小图 inline、大图传 Files | [`openai_files.rs::upload_decision_by_size`](../../../src/core/llm/openai_files.rs) | `InlinePreferred` → `image_b64`；`UploadRequired` → 上传得 `file_id` | 图小直接塞、图大走上传。 |
| **endpoint 回退** | 不配画图就借聊天的 base/key | `ToolsImageGenConfig.{base_url,api_key_env}: Option` | None → `llm.api_base` / `OPENAI_API_KEY`；遵循 `env > config > 默认` | 没单独配就用聊天那套。 |
| **`tool-results/`** | 生成图落盘目录 | `resolve_agent_trail_dir()?/tool-results/` | `genimg-<hash>.png`；与 bash / web_fetch 同目录 | 图都存这个统一的产物目录。 |

**「LLM 收到 tool 结果后」**：指 **`tool_exec` 已把 `model_text` 写成 tool 消息、且 `tool_dispatcher` 已把 `follow_up_parts` 拼成紧随的 user 消息**、即将进入下一轮模型推理之前。

---

## 4. 协议（入参 / 出参 / Schema）

**单一事实源**：

- JSON Schema（模型可见）：[`catalog.rs::generate_image_parameters`](../../../src/core/tools/contract/catalog.rs)（PR-IG-A 添加）→ [`docs/tool-catalog.md`](../../tool-catalog.md) 自动派生。
- Rust 类型：`core/tools/image_gen/types.rs`（PR-IG-B 新增）的 `GenerateImageArgs` / `ImageGenOutput`。

### 4.1 入参（工具 arguments）

| 字段 | JSON 类型 | 必填 | 默认 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|------|----------|------|--------|
| `prompt` | string | **是** | — | 全部 | 图像描述（文生图的内容或编辑指令） | 想画什么/想怎么改写这里。 |
| `action` | enum `generate` \| `edit` | 否 | `generate` | 全部 | `generate` 文生图；`edit` 基于 `image_path` 编辑 | 画新的还是改旧的。 |
| `image_path` | string \| null | `edit` 时**是** | null | `edit` | 待编辑原图的本地路径；`generate` 时忽略 | 改图模式下给原图路径。 |
| `size` | string \| null | 否 | `auto` | 全部 | 输出尺寸（如 `auto` / `1024x1024` / `1536x1024`） | 出图多大，一般留 auto。 |
| `quality` | string \| null | 否 | `auto` | 全部 | 质量档（透传 Images API，如 `auto`/`high`） | 画质档，默认自动。 |

**`image_path` 三态语义**：

- 缺省 / 显式 `null`：仅 `action=generate` 合法；`action=edit` 缺它 → `Err`。
- 显式路径（`action=edit`）：读字节进 multipart `image` 字段。
- 显式路径（`action=generate`）：忽略（不报错，但回执 `warnings` 提示「generate 模式忽略 image_path」）。

### 4.2 出参（Rust：`ImageGenOutput`，序列化进 `model_text`）

| 字段 | 类型 | 说明 | 说人话 |
|------|------|------|--------|
| `path` | `String` | 生成图落盘绝对路径（`tool-results/genimg-<hash>.png`） | 图存哪了。 |
| `mime` | `String` | 图片 MIME（`image/png`） | 啥格式。 |
| `bytes` | `u64` | 落盘文件字节数（用于回灌时 inline/上传分流） | 多大，决定怎么回灌。 |
| `revised_prompt` | `Option<String>` | 部分模型回写的「实际生效 prompt」 | 模型实际照着画的描述。 |
| `action` | `String` | 本次 `generate` / `edit` | 这次干了啥。 |
| `warnings` | `Vec<String>` | 非致命提示（如 generate 忽略 image_path） | 有啥小提醒。 |

> **注意**：`follow_up_parts`（回灌的 `InputImage`）**不在** `ImageGenOutput` 里——它由 `handle_generate_image` 直接产出为 tuple 第二元，经 `tool_dispatcher` 注入 user 消息，与 `model_text` 是两条正交通道。

### 4.3 调用样例（jsonc）

**文生图**：

```jsonc
{ "prompt": "a calico cat astronaut floating in a neon nebula, cinematic", "action": "generate", "size": "1024x1024" }
```

**图片编辑**：

```jsonc
{ "prompt": "make the background a sunny beach", "action": "edit", "image_path": "/Users/me/.tomcat/agents/abc/tool-results/genimg-9f3a.png" }
```

**典型 `model_text`（文生图成功）**：

```jsonc
{
  "path": "/Users/me/.tomcat/agents/abc-123/tool-results/genimg-9f3a2b.png",
  "mime": "image/png",
  "bytes": 1842317,
  "revised_prompt": "A calico cat in a spacesuit floating in a neon-lit nebula...",
  "action": "generate",
  "warnings": []
}
```

紧随其后由 `tool_dispatcher` 自动追加的 user 消息（示意，非工具直接产物）：

```jsonc
// role=user, content parts:
[ { "type": "input_image", "image_b64": "<base64>", "mime_type": "image/png" } ]
// 大图时改为：{ "type": "input_image", "file_id": "file-abc" }
```

---

## 5. One-Glance Map（文件职责总览）

```text
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/tools/contract/catalog.rs                                        │
│  • BUILTIN_TOOL_CATALOG: name = "generate_image"                           │
│  • generate_image_parameters(): prompt + action + image_path? + size? ...  │
│  • 元数据: scope=Read, read_only=false, category=Exec, plan_only=false     │
└───────────────────────────────┬────────────────────────────────────────────┘
        │ visible_tools_for_mode_with_policy 过滤后给 LLM
        ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/agent_loop/tool_exec/  (目录模块)                                 │
│  • mod.rs::execute_tool_tuple_full → match "generate_image"                 │
│  • mod.rs::ToolExecCtx 增 image_gen_runtime 字段                            │
│  • branches/generate_image.rs::handle_generate_image                        │
│      → 返回 (model_text, follow_up_parts)  ← 与 handle_read 同形            │
│      → 复用 read 的 image_b64 / OpenAiFilesRuntime 注入分支                 │
└───────────────────────────────┬────────────────────────────────────────────┘
                                │
                ┌───────────────┴──────────────────┐
                ▼                                  ▼
┌──────────────────────────────────┐  ┌──────────────────────────────────────┐
│  src/core/tools/image_gen/        │  │  src/core/agent_loop/tool_dispatcher.rs│
│  ├ mod.rs  ImageGenRuntime        │  │  • follow_up_parts 非空 →             │
│  │   generate()/edit()           │  │    push ChatMessage::user_with_parts   │
│  │   reqwest POST images/{gen,edit}│  │  • 已有逻辑，无需改                    │
│  │   b64_json→decode→fs::write    │  └──────────────────────────────────────┘
│  └ types.rs                       │
│     GenerateImageArgs / ImageGenOutput│
└───────────────────────────────┬──┘
        │ 复用                     │ 复用
        ▼                         ▼
┌──────────────────────────┐  ┌──────────────────────────────────────────┐
│ src/core/llm/             │  │ src/core/llm/openai_files.rs              │
│ ├ http_client.rs          │  │ • upload_decision_by_size(bytes)          │
│ │  build_http_client      │  │ • resolve_or_upload_path(.., Vision)      │
│ ├ auth.rs  AuthStore      │  │ src/core/llm/types.rs                     │
│ │  {PROVIDER}_API_KEY     │  │ • ChatMessageContentPart::image_b64       │
│ └ (LlmConfig.api_base)    │  │ • ::image_file_id                         │
└──────────────────────────┘  └──────────────────────────────────────────┘
        │
        ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/infra/config/types/tools.rs                                           │
│  • ToolsConfig 加 ToolsImageGenConfig                                       │
│    { base_url: Option, api_key_env: Option, model, timeout_ms }            │
│  src/api/chat/context.rs           • 构造 ImageGenRuntime 注入 ChatContext  │
│  src/core/agent_loop/accessors.rs  • runtime 透传进 ToolExecCtx             │
└────────────────────────────────────────────────────────────────────────────┘

  + tests:
    src/core/tools/image_gen/tests/        (generate/edit/落盘/配置回退 mock HTTP)
    src/core/agent_loop/tool_exec/branches/ (generate_image 注入分支单测)
    tests/generate_image_tool_tests.rs      (public roundtrip + env-gated live smoke)
```

**阅读顺序（说人话）**：模型先在 **catalog** 看到 `generate_image` 与几个参数；调起后 **`tool_exec`** 路由到 **`handle_generate_image`**；它调 **`image_gen/mod`** 的 runtime 打 OpenAI Images 接口、解 base64 落盘；拿到 `path` + `bytes` 后，**复用 `read` 工具的注入分支**（`openai_files` 判大小 + `types` 造 part）产出 `follow_up_parts`；最后 **`tool_dispatcher`**（现成逻辑）把图拼成一条 user 消息回灌。配置走 `[tools.image_gen]`，不配则回退 `[llm]`。

---

## 6. 调度时序（运行时图）

```text
LLM      tool_exec   handle_generate_image  ImageGenRuntime  network   openai_files  tool_dispatcher
 │           │              │                     │            │           │              │
 │ generate_image          │                     │            │           │              │
 │──────────▶│ parse args   │                     │            │           │              │
 │           │─────────────▶│ runtime.generate/edit            │           │              │
 │           │              │────────────────────▶│ POST images/{gen,edit} │              │
 │           │              │                     │───────────▶│           │              │
 │           │              │                     │◀── b64_json│           │              │
 │           │              │                     │ decode + fs::write tool-results/*.png  │
 │           │              │◀────────────────────│ ImageGenOutput{path,bytes,...}         │
 │           │              │ upload_decision_by_size(bytes)   │           │              │
 │           │              │─────────────────────────────────────────────▶│ inline / upload
 │           │              │◀─────────────────────────────────────────────│ InputImage part
 │           │◀─────────────│ (model_text, follow_up_parts)    │           │              │
 │           │ tool 消息(model_text) ───────────────────────────────────────────────────▶│
 │           │ follow_up_parts 非空 ────────────────────────────────────────────────────▶│ push user_with_parts
 │◀──────────│ 下一轮 messages：tool result + user(InputImage)  │           │              │
```

**事件 / 状态迁移发布点**：

- runtime HTTP 失败（4xx/5xx/超时）→ `handle_generate_image` 返回 `Err(String)`，`tool_exec` 写成 error tool 消息（不 panic、不重试风暴）。
- 落盘成功但 `follow_up_parts` 构造失败（如超 4.5 MiB 且无 Files runtime）→ 降级为纯文本回执 + `warnings`，**不**整轮失败（对齐 [`read.rs`](../../../src/core/agent_loop/tool_exec/branches/read.rs) 的 `tracing::warn! + fallback` 口径）。
- `follow_up_parts` 非空 → `tool_dispatcher` 追加 `user_with_parts`（与 read 图片注入同一事件点）。

---

## 7. 状态机（单次 generate_image 生命周期）

```text
┌──────────────┐ parse ok   ┌──────────────┐ http 200   ┌──────────────┐
│  init        │───────────▶│  requesting  │───────────▶│  persisting  │
└──────┬───────┘            └──────┬───────┘            └──────┬───────┘
       │ parse err                 │ http err / 超时           │ b64 解码/落盘成功
       ▼                           ▼                           ▼
┌──────────────┐            ┌──────────────┐            ┌──────────────┐
│  arg_error   │            │  http_error  │            │  injecting   │
│ (Err)        │            │ (Err)        │            └──────┬───────┘
└──────────────┘            └──────────────┘                   │
                                                    ┌──────────┴───────────┐
                                                    │ part 构造成功         │ part 构造失败(超限/无 Files)
                                                    ▼                       ▼
                                              ┌──────────────┐      ┌──────────────────┐
                                              │  done(图+文) │      │ done(仅文+warning)│
                                              └──────────────┘      └──────────────────┘
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `init` | 参数解析成功 | `requesting` | — | 参数对，准备发请求。 |
| `init` | `action=edit` 缺 `image_path` / JSON 解析失败 | `arg_error` | 返回 `Err`，error tool 消息 | 参数不对，直接报错。 |
| `requesting` | HTTP 200 + 含 `b64_json` | `persisting` | — | 接口回图了。 |
| `requesting` | 4xx/5xx/超时/无 `b64_json` | `http_error` | 返回 `Err`（含状态码/原因，凭证已脱敏） | 接口出错，报错但不崩。 |
| `persisting` | 解码 + `fs::write` 成功 | `injecting` | 写 `tool-results/genimg-<hash>.png` | 图存盘了。 |
| `persisting` | 解码/落盘 IO 失败 | `http_error` | 返回 `Err("persist failed: ...")` | 存盘失败也报错。 |
| `injecting` | `image_b64`/Files 上传成功 | `done(图+文)` | `follow_up_parts=[InputImage]` + `model_text` | 图回灌、回执齐全。 |
| `injecting` | 超 4.5 MiB 且无 Files runtime / 上传失败 | `done(仅文+warning)` | `follow_up_parts=[]`，`warnings += "image_inline_skipped"` | 图太大没法回灌，至少给路径。 |

---

## 8. 配置与环境变量

**总则**：`env > config > 默认`。**画图相关配置全部可缺省**——缺省即回退 `[llm]`。

| 来源 | 键 | 含义 | 默认 | 说人话 |
|------|-----|------|------|--------|
| `tomcat.config.toml` | `[tools.image_gen] base_url` | Images API base（拼 `/images/generations`） | `None` → 回退 `[llm] api_base` | 不填就用聊天那个地址。 |
| `tomcat.config.toml` | `[tools.image_gen] api_key_env` | 读 key 的环境变量名 | `None` → 回退 `OPENAI_API_KEY` | 不填就用 OPENAI_API_KEY。 |
| `tomcat.config.toml` | `[tools.image_gen] model` | 图像模型 | `gpt-image-2` | 用哪个画图模型。 |
| `tomcat.config.toml` | `[tools.image_gen] timeout_ms` | 单次请求墙钟超时 | `120_000` | 多久没回算超时。 |
| 环境变量 | `OPENAI_API_KEY`（或 `api_key_env` 指定） | API 密钥 | — | 真正的密钥。 |
| 环境变量 | `TOMCAT__TOOLS__IMAGE_GEN__MODEL` 等 | 上述字段运行时覆盖 | — | 容器里临时改。 |
| `tomcat.config.toml` | `[llm] proxy` | 共享出网代理（与 web_fetch/web_search 同链路） | `None` | 要走企业代理在这配。 |

**用户在入参里没有可覆盖的 endpoint/key/model**——模型只能动 `prompt`/`action`/`image_path`/`size`/`quality`，凭证与地址类配置不开放给模型。

---

## 9. 错误模型 / 截断 / 警告

```text
                    generate_image 请求
                            │
        ┌───────────────────┼─────────────────────┐
        ▼                   ▼                     ▼
   参数解析失败         action=edit 缺           image_path 不存在
   AppError→Err        image_path → Err          / 读失败 → Err
   ("invalid args")    ("edit requires           ("image_path not readable")
        │               image_path")              │
        └───────────────────┴─────────────────────┘
                            │ 参数 ok
                            ▼
                    HTTP POST images/{gen,edit}
                            │
        ┌───────────────────┼─────────────────────┐
        ▼                   ▼                     ▼
   4xx (鉴权/内容策略)   5xx / 超时             无 b64_json
   Err(脱敏文案)        Err("upstream 5xx")     Err("empty image data")
                            │ 200 + b64_json
                            ▼
                    decode + fs::write
                            │
                ┌───────────┴───────────┐
                ▼                       ▼
           落盘成功                  IO 失败
                │                  Err("persist failed")
                ▼
           follow_up_parts 构造
                │
        ┌───────┴────────────────┐
        ▼                        ▼
   part 成功                 超限/无 Files runtime
   done(图+文)               done(仅文 + warnings+="image_inline_skipped")  ← 不抛 Err
```

**`tool_exec` 视角**：

- `Err(_)` → tool 消息文本为错误描述（致命类：参数 / edit 缺图 / 鉴权 / upstream / 空数据 / 落盘）；**凭证一律脱敏**（`Bearer xxx` → `<redacted>`）。
- `Ok((model_text, follow_up_parts))` → `model_text` 为 JSON 回执（含可能的 `warnings`），`follow_up_parts` 可能为空（回灌降级）。

**§1 G1–G5 的「锁死它的测试」**全部位于 §10。

---

## 10. 测试矩阵（验收）

| 维度 | 用例（计划函数名） | 状态 | 说人话 |
|------|---------------------|------|--------|
| catalog 注册 | `catalog_test::generate_image_registered`、`submodules_test::tool_exec_generate_image_requires_runtime_injection` | PENDING | 名字注册了、未注入 runtime 给显式错误。 |
| 文生图 (G1) | `image_gen_test::generate_decodes_b64_and_persists`（mock HTTP 返 `b64_json` → 校验落盘 + path 字段） | PENDING | 给 prompt 回图存盘。 |
| 图片编辑 (G2) | `image_gen_test::edit_uploads_multipart`（校验 multipart 含 image + prompt 字段） | PENDING | edit 模式真上传原图。 |
| 多模态回灌 (G3) | `generate_image_test::{small_image_inlined_as_part,large_image_uploaded_via_files}` | PENDING | 小图 inline、大图走 Files。 |
| 上下文不爆 (G4) | `generate_image_test::model_text_excludes_base64` | PENDING | 回执里没有 base64。 |
| 鉴权回退 (G5) | `image_gen_test::{config_falls_back_to_llm_base_and_key,explicit_override_wins}` | PENDING | 不配回退聊天、配了用自己的。 |
| edit 缺图 | `image_gen_test::edit_without_image_path_errs` | PENDING | edit 不给图就报错。 |
| 鉴权失败脱敏 | `image_gen_test::missing_key_returns_err`、`error_redacts_bearer_token` | PENDING | 没 key 报错、错误不漏 key。 |
| upstream 错误归一 | `image_gen_test::{http_4xx_returns_err,http_5xx_returns_err,empty_data_returns_err}` | PENDING | 接口报错变成清楚的 Err。 |
| 回灌降级 | `generate_image_test::oversize_image_degrades_to_text_only` | PENDING | 图太大只给路径不崩。 |
| 配置解析 | `infra/config/tests/tools_cfg_test::image_gen_optional_fields` | PENDING | `[tools.image_gen]` 反序列化无丢字段。 |
| Public runtime / live smoke | `tests/generate_image_tool_tests::{public_output_roundtrip,live_generate_smoke}`（`PI_LIVE_IMAGE_GEN=1` gate） | PENDING（env-gated） | 上线前真打一次官方接口。 |
| E2E（live） | `E2E-IMAGE-GEN-001`：真 `gpt-image-2` 文生图 + 回灌（env-gated；CI 默认跳） | PENDING（env-gated） | 整条链真跑一遍。 |

§1 观察指标 **G1–G5** 与本表逐行对应：G1↔文生图；G2↔图片编辑；G3↔多模态回灌；G4↔上下文不爆；G5↔鉴权回退。

---

## 11. 风险与应对

| 风险 | 影响 | 应对（具体动作） | 说人话 |
|------|------|------------------|--------|
| **生成图 base64 撑爆上下文** | 1 MB 图 base64 → ~1.4 MB token 直接进 tool 文本会炸 | `model_text` **禁含 base64**，只给 path/mime/bytes；图本体只经 `follow_up_parts`，且复用 [`openai_files.rs`](../../../src/core/llm/openai_files.rs) 的 4.5 MiB 分流：超限走 Files 上传得 `file_id`（非 inline base64） | 回执给路径，图走专门通道还分大小。 |
| **复用 LLM 网关无 Images 接口** | 聊天走第三方兼容网关时 `images/generations` 404 | 默认回退仅是默认；文档明确「网关不支持时配 `[tools.image_gen] base_url` 指向官方 OpenAI」；404/400 归一为清晰 `Err` 提示用户改配置 | 网关画不了就单独配官方地址。 |
| **API key 泄漏到 transcript** | 凭证外泄 | 错误文案 redaction（`Bearer xxx`→`<redacted>`）；key 只在 runtime 内存读取，不写回执、不进 `model_text` | 报错也别把 key 抖出来。 |
| **内容策略拒绝（4xx moderation）** | 模型反复重试同 prompt | 4xx 归一为 `Err` 并把 upstream 文案（脱敏后）回给模型，让其改 prompt，**不**自动重试 | 被拒就让模型换个说法，别死循环。 |
| **edit 读到超大/非法本地图** | 撑爆 multipart / 上传失败 | `image_path` 读前做 metadata 预检（复用 read 的 25 MiB 上限语义）；非图 MIME → `Err` | 改图前先量一下原图大小和类型。 |
| **落盘目录写满 / 不可写** | IO panic | `tokio::fs::write` 失败归一为 `Err("persist failed: ...")`，不 panic；`tool-results/` 启动期已 `create_dir_all`，运行时幂等兜底 | 存不下就报错，不崩。 |
| **回灌失败让整轮塌** | part 构造异常中断对话 | 对齐 [`read.rs`](../../../src/core/agent_loop/tool_exec/branches/read.rs)：part 构造失败 `tracing::warn!` + 降级纯文本 + `warnings`，**不**抛 `Err` | 图回灌不了就只给路径，对话照常。 |

---

## 12. 历史决策（已被本方案取代或待定）

- ~~直接搬 codex 的 `ext/image-generation` extension crate 过来~~ → **否**：它依赖 `codex-api` / `codex-extension-api` / `codex-login` 整套 crate 与 codex 的 auth provider，迁移成本高于自写；**只借鉴其设计**（`generate`/`edit` 双 action、`gpt-image-2`、data URL 回灌）。
- ~~走 codex hosted `image_generation`（Responses API 托管）~~ → **否**：强制 `current_auth_uses_codex_backend()`（ChatGPT/Codex 登录态，纯 API key 不够），与 Tomcat 自托管 + OpenAI-compatible 目标冲突。
- ~~做成 Provider 插件化多 backend（hermes/openclaw 形态）~~ → **否**：MVP 只需一条 OpenAI 兼容路；多 vendor 抽象（FAL/xAI/Krea）徒增维护面，后续有需要再单开迭代。
- ~~纯 JSON 返回 URL/路径、不回灌模型（hermes 形态）~~ → **否**：文生图的核心价值是模型能「看见并迭代」生成图；改用 `follow_up_parts` 回灌（pi_agent_rust / codex extension / openclaw 均回灌）。
- ~~把 base64 直接塞进 tool 文本消息~~ → **否**：撑爆上下文；改为只在 `follow_up_parts` 通道走，且经 Files 上传分流。
- ~~生成图落到独立 `generated_images/` 目录（codex 形态）~~ → **否**：与本仓 `tool-results/` 约定割裂；统一落 `tool-results/`。

**跨文档修订**：

- 本文新增 catalog 条目 `generate_image` 触及 [`docs/tool-catalog.md`](../../tool-catalog.md)（派生文档，由工具定义自动生成）——落地时运行 `UPDATE_TOOL_CATALOG=1 cargo run --bin gen-tool-catalog` 重生成，不手改。
- 本文不修改 [`read.md`](read.md) 已冻结正文，但**复用其多模态注入管道**（`(model_text, follow_up_parts)` tuple + `image_b64`/`OpenAiFilesRuntime`）；若 read 侧注入逻辑重构，本工具需同步。
- 与 [`image_to_video.md`](image_to_video.md) 共享 `tool-results/` 落盘约定与 `[tools.*]` 配置框架，但回传方式相反（图回灌、视频只给路径）。

---

## 13. 关联文档

- 兄弟工具：[`image_to_video.md`](image_to_video.md)（图生视频，异步轮询）· [`read.md`](read.md)（多模态注入管道来源）· [`web_fetch.md`](web_fetch.md)（外部 HTTP + 落盘框架）
- 派生工具目录：[`tool-catalog.md`](../../tool-catalog.md)
- 规范：[`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)
- 竞品源码：codex `ext/image-generation/` · hermes-agent `tools/image_generation_tool.py` · openclaw `src/agents/tools/image-generate-tool.ts` · pi_agent_rust `antigravity-image-gen.ts` · pi-mono `packages/ai/src/images.ts`

---

**一句话总结**：`generate_image` 在 **`tool_exec`** 解参数、在 **`image_gen/mod`** 用 reqwest 直打 OpenAI 兼容 `images/generations`（文生图）/ `images/edits`（图片编辑）、解 `b64_json` 落盘 `tool-results/`；协议以 **`catalog.rs` + `image_gen/types.rs`** 为单一事实源，鉴权默认回退 `[llm]` 的 base/key 可独立覆盖；最关键的是**复用 [`read`](read.md) 的 `(model_text, follow_up_parts)` 管道把生成图回灌给模型**，让模型下一轮真正「看见」自己画的图——这是它区别于 `web_fetch`（只给路径）和 hermes（只给 URL）的核心。**本工具尚未落地，全文为 PR-IG-A/B/E 目标态设计，§10 全部 PENDING。**
