# 模型管理与 Add Models：自定义下拉 · 编辑器区设置中心 · 内置预置 · Anthropic Provider

> 适用范围：给 Tomcat Agent Box 增加「模型管理」端到端能力——把 Composer 的原生 `<select>` 换成含分隔符与「Add Models」的**自定义下拉**；点击后在**编辑器区全屏 Webview 标签页**打开「设置中心」（左导航模块入口 / 右模块内容，本期只 Models 可点）；在 Models 模块页里增删改模型、填写 API Key；配套新增 `tomcat model` CLI 子命令，把常用模型（OpenAI / MiMo / DeepSeek / GLM / Kimi / Claude Opus）作为**官方内置预置**开箱即用（用户只填 Key），并实现新的 **Anthropic Messages provider**。
> 上位规范：[`ARCHITECTURE_SPEC.md`](../../../tomcat/docs/openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)。本方案参考 [`tomcat-vscode-extension-phase2.md`](tomcat-vscode-extension-phase2.md) 的组织方式，按规范 §1–§10 拆为「总览（本文）+ 5 篇子文档」，文首「方案导图集」置于子文档之前、不占用 § 编号。
> 单一事实源：模型目录仍以内嵌 `tomcat/src/core/llm/builtin_models.toml` + `catalog.rs` 解析结果为准；serve 协议以 `tomcat/src/api/serve/types.rs` 为准；模型增删改 / Key 写入的落地逻辑统一收敛到拟新增的 `core/llm/admin.rs`，CLI 与 serve 都调用它，不各写一份。

**一句话定位**：本方案把「模型管理」收敛成一条清晰链路：**唯一写盘中枢** `core/llm/admin.rs`（改 `models.toml`/`.env`，含锁、原子写、校验、Key 脱敏与只入不出）对上暴露 `tomcat model` **CLI** 与 `tomcat serve` **模型管理命令**两个等价门面；前端在侧栏用**自定义下拉**（可用模型 + 分隔符 + Add Models）触发**编辑器区设置中心**（左导航 / 右 Models 页，密文填 Key 即用）；后端把常用模型统一收敛到内嵌 `builtin_models.toml` **单源**（OpenAI 5.4/5.5/5.6、MiMo、DeepSeek、GLM、Kimi、Opus×3），由 `catalog.rs` 解析成运行时 builtin catalog，用 **path-aware endpoint** 修好 GLM 非 `/v1` 地址，并新增 `anthropic-messages` **provider** 接入 Claude；「一个模型能不能用」由「Key 变量是否非空」动态二分，内置预置填个 Key 就地转正——全程 Key 密文、不回吐、`.env` 0600。

---

## 子文档索引

本方案按 ARCHITECTURE_SPEC §1–§10 拆分；下表给出「子文档 ↔ 规范 §」对应关系。建议先读本文「文首导图集」建立心智模型，再按需下钻。

| 子文档 | 覆盖规范 § | 内容 | 何时读 |
|--------|------------|------|--------|
| [`01-scope-and-research.md`](model-management-add-models/01-scope-and-research.md) | §1 术语 · §2 竞品调研 | 模型管理新增术语（`ModelView` / `key_present` / 设置中心 / model-admin capability 等）；设置页形态、下拉形态、GLM/Kimi/Anthropic 接入的调研证据链 | 想先搞清「为什么这样拆、为什么写盘要走 Rust、为什么 Anthropic 要新 provider」先读它。 |
| [`02-decisions-and-delivery.md`](model-management-add-models/02-decisions-and-delivery.md) | §3 落地选型与实施 | §3.1 八行决策表（设置页承载 / 写入路径 / 下拉形态 / 单表 builtin / path-aware / Anthropic / 热重载 / Key 保护）+ §3.2 实施点（IP0–IP4） | 开发前先对齐最终方案与阶段拆分时读它。 |
| [`03-protocol-and-runtime.md`](model-management-add-models/03-protocol-and-runtime.md) | §4 协议 · §5 One-Glance | 新 serve 命令 / `ModelView` 字段表 / webview intent / jsonc 样例；完整文件职责总览 | 落协议、改字段、梳理文件改动面时查它。 |
| [`04-operations-validation-history.md`](model-management-add-models/04-operations-validation-history.md) | §6 配置 · §7 错误 · §8 测试 · §9 风险 · §10 历史 | provider→env 映射、错误归一化、测试矩阵、风险与历史决策 | 验收、回归、排错、补文档时查它。 |
| [`05-ui-ascii-baseline.md`](model-management-add-models/05-ui-ascii-baseline.md) | 附录 A（UI 基线） | 完整保留总分 ASCII UI 设计图集（总图 / 下拉 / 设置中心 / Models 页 / provider 分派 / CLI） | 想看界面排版与交互基线时直接读它。 |

---

## 文首导读：方案导图集

### 阅读顺序建议（说人话）

1. **A.1 抽象总图**：先看职责与事实源——「谁负责画 UI、谁负责写盘、单一事实源在哪、缺 Key 的模型怎么变成可用」。
2. **A.2 具体总图**：再把同一条链路落到真实进程 / 文件 / wire 帧（Composer 下拉 ↔ 设置中心 webview ↔ 扩展宿主 ↔ `tomcat serve` 新命令 ↔ `core::llm::admin` ↔ `models.toml` / `.env`）。
3. **B 状态机**：最后看一个「模型目录条目」的生命周期：`内置预置(缺Key) → 填Key → 可用`、`自定义(upsert) → 可用`、以及删除/覆盖。
4. **附录 A（UI 图集）**：想看每一块 UI 长什么样，直接翻 [`05-ui-ascii-baseline.md`](model-management-add-models/05-ui-ascii-baseline.md)。
5. **下钻子文档**：想看「为什么这么选」跳 [`01-scope-and-research.md`](model-management-add-models/01-scope-and-research.md) / [`02-decisions-and-delivery.md`](model-management-add-models/02-decisions-and-delivery.md)；看协议跳 [`03-protocol-and-runtime.md`](model-management-add-models/03-protocol-and-runtime.md)；看改了哪些文件、验收与风险跳 [`03-protocol-and-runtime.md`](model-management-add-models/03-protocol-and-runtime.md) / [`04-operations-validation-history.md`](model-management-add-models/04-operations-validation-history.md)。

> 说人话：本特性的认知锚点是「**写盘逻辑只有一份（`admin.rs`），CLI 和 GUI 只是它的两个门面**」。UI 再花哨，最终都落到「改 `models.toml`」或「改 `.env`」两件事；而「一个模型能不能用」= 「它在目录里」且「它的 API Key 变量非空」。

### A.1 抽象 ASCII 总图（职责 / 事实源 / 分叉 / 终局）

```text
输入 / 触发点
  · Composer 下拉点「Add Models …」
  · 设置中心 Models 页：新增模型 / 填 Key / 删除
  · 终端：tomcat model list|add|remove|key ...
        │
        │  意图：读目录 / 写模型 / 写 Key
        ▼
┌─ 门面层（两个入口，等价语义）──────────────────────────────┐
│  GUI 入口：tomcat serve 模型管理命令（NDJSON）              │
│  CLI 入口：tomcat model <sub>                              │
│  职责：只做参数校验 + 转调，不各自实现写盘                  │
└───────────────────────────┬──────────────────────────────┘
                            │
                            ▼  单一事实来源
┌─ core::llm::admin（写盘 / 读视图的唯一实现）──────────────┐
│  list_model_views  ── 读：内置+用户合并，标注 source/key   │
│  upsert_user_model ── 写 → models.toml（锁+原子+校验）     │
│  remove_user_model ── 写 → models.toml（内置不可删）        │
│  set_provider_key  ── 写 → assets/.env（0600，只入不出）    │
└───────────────────────────┬──────────────────────────────┘
                            │ 写后 reload
                            ▼
┌─ ModelCatalog（可重载句柄；单一事实源）───────────────────┐
│  builtin_models.toml（内嵌）── 官方内置预置（开机即在）    │
│  ⊕ 用户 models.toml（同 id 覆盖 / 新 id 追加）             │
│  registry(api) → provider 分派                            │
└───────────────────────────┬──────────────────────────────┘
        关键分叉：一个条目「能不能用」                        │
        ├─ api_key_env 对应变量非空 ─► Ready（进下拉、可 set_model）
        └─ 变量为空（多为内置预置）─► Needs API key（仅设置页可见，待填 Key）
                            │
                            ▼  provider 分派（按 api）
        openai / openai-responses（复用）│ anthropic-messages（新增）
                            │
                            ▼  终局
        选中模型 → set_model → AgentLoop 正常对话
```

> 导读：这张图先回答「谁写盘、事实源在哪、缺 Key 怎么办」。**最该记住两点**：(1) 写盘逻辑收敛在 `admin.rs` 一处，CLI/GUI 只是门面——这直接满足「CLI 与 GUI 共用同一套逻辑」的硬约束；(2) 「可用 vs 需填 Key」不是两张表，而是**同一目录条目按「Key 变量是否非空」二分**，所以内置预置只要补个 Key 就地转正、无需重新登记模型。

### A.2 具体 ASCII 总图（落到真实进程 / 文件 / wire 帧）

```text
 VS Code 扩展进程 (Node/TS)                              子进程 (Rust)
┌───────────────────────────────────────┐     ┌────────────────────────────────────┐
│ Secondary Side Bar                     │     │ $ tomcat serve --stdio              │
│  gui/src/components/Composer.tsx       │     │                                     │
│   自定义 Model 下拉 ◀NEW               │     │ src/api/serve/types.rs              │
│   （分隔符 + Add Models 页脚）          │     │  ServeCommand ◀NEW 变体:            │
│        │ postIntent("openModelSettings")│stdin│   UpsertModel/RemoveModel/          │
│        ▼                               │NDJSON│   SetProviderKey/ListProviderKeys   │
│  ui/webview/provider.ts                │────▶│                                     │
│   handleIntent → SettingsPanel.reveal  │     │ src/api/serve/commands.rs           │
│                                        │     │  handle_command ◀NEW 分支 → 调:     │
│ Editor Area                            │     │   core::llm::admin::*               │
│  ui/settings/SettingsPanel.ts ◀NEW     │     │  ListModels 出参 ◀加 source/key     │
│   createWebviewPanel(retainContext)    │     │                                     │
│        │ postMessage(intent/state)      │◀────│ src/api/serve/control.rs            │
│        ▼                               │stdout│  capabilities ◀NEW:                 │
│  gui/src/settings/*.tsx ◀NEW           │NDJSON│   upsert_model/remove_model/        │
│   设置中心壳(左导航/右内容)+Models 页  │     │   set_provider_key                  │
│        │ upsertModel / setProviderKey   │     │                                     │
│        ▼                               │     │ src/core/llm/admin.rs ◀NEW ★        │
│  serveClient/TomcatMessenger.ts        │     │  list_model_views/upsert/remove/    │
│   +sendUpsertModel/+sendSetProviderKey │     │  set_provider_key/list_provider_keys│
│   +sendRemoveModel/+sendListProviderKey│     │        │ 写                          │
│  serveClient/initialize.ts             │     │        ├─▶ core/llm/catalog.rs      │
│   +SERVE_CAPABILITY_* 门控             │     │        │    builtin_models.toml      │
│  serveClient/wire.d.ts / protocol.ts   │     │        │    /Opus…；可重载句柄       │
│   +新命令/响应类型                     │     │        ├─▶ core/llm/registry.rs     │
└───────────────────────────────────────┘     │        │    +("anthropic-messages",..)│
       terminal 入口（等价语义）                │        │    core/llm/anthropic/* ◀NEW │
       $ tomcat model list|add|remove|key      │        └─▶ ~/.tomcat/models.toml     │
        └─ src/api/cli/model_cmd.rs ◀NEW ──────┘             ~/.tomcat/assets/.env    │
           复用同一 core::llm::admin ★         └────────────────────────────────────┘
   构建期：tomcat serve --print-schema → 刷新 serveClient/wire.d.ts（新命令自动入类型）
```

> 导读：这张图把抽象落到真实对象。**看清三件事**：(1) 带 ★ 的 `core/llm/admin.rs` 是新的写盘中枢，`model_cmd.rs`（CLI）与 `commands.rs`（serve）都指向它；(2) 设置中心是**第二个 webview**（`SettingsPanel.ts` + `gui/src/settings/*`，编辑器区），与侧栏 Agent Box 独立，仅通过 `openModelSettings` intent 联动；(3) 除 Anthropic 需要新增 `core/llm/anthropic/*` wire 外，GLM/Kimi 等预置都复用既有 `openai` / `openai-responses`，registry 只加一行。

### B. 状态机：一个「模型目录条目」的生命周期

```text
                       upsert_user_model（含合法 api/provider/base_url）
        ┌───────────────────────────────────────────────────────────┐
        │                                                             ▼
┌───────────────┐  内置于 builtin_models.toml       ┌───────────────────────────┐
│  (不存在)     │ ─────────────────────────────────▶│  NeedsKey                 │
└───────────────┘                                    │  {source, key_present=F}  │
        ▲                                             └───────┬───────────────────┘
        │ remove_user_model                                   │ set_provider_key(env,value)
        │（仅 User 源可删；Builtin 拒绝）                       ▼   写 .env → key_present=T
        │                                             ┌───────────────────────────┐
        └─────────────────────────────────────────── │  Ready                    │
                                                      │  {进下拉 / 可 set_model}   │
                                                      └───────┬───────────────────┘
                                                              │ 清空 / 删除对应 .env 键
                                                              ▼
                                                        回到 NeedsKey
```

| 当前状态      | 事件                            | 目标状态           | 副作用（`admin.rs`）                                   | 说人话                    |
| --------- | ----------------------------- | -------------- | ------------------------------------------------- | ---------------------- |
| (不存在)     | 命中内嵌 `builtin_models.toml` | NeedsKey       | 无（开机即在，`key_present` 依 `.env` 判定）                 | 官方内置模型开机就列出来，只是还没 Key。 |
| (不存在)     | `upsert_user_model`           | Ready/NeedsKey | 写 `models.toml` + reload；校验 api/provider/base_url | 用户手动加一个自定义模型。          |
| NeedsKey  | `set_provider_key{env,value}` | Ready          | 写 `.env`(0600) + 重载 Key；仅回 `key_present=true`     | 填上 Key 就地转正，进入下拉。      |
| Ready     | 对应 `.env` 键被清空                | NeedsKey       | 下次 `list_model_views` 判定 `key_present=false`      | Key 没了就退回「待填」。         |
| User 源任意态 | `remove_user_model`           | (不存在)          | 从 `models.toml` 删除 + reload                       | 只能删自己加的；内置删不掉。         |
| Builtin 源 | `remove_user_model`           | 不变（Err）        | 拒绝：内置不可删（可被同 id user 条目覆盖）                        | 想改内置就用同 id 覆盖，而不是删。    |

> 导读：状态机的关键是**「可用」由 Key 变量是否非空动态判定**，不是模型登记时的静态字段。所以内置预置（NeedsKey）和填了 Key 的模型（Ready）是同一条目的两态；删除只对 User 源生效，内置只能覆盖不能删。

---

## 一句话总结

本方案把「模型管理」收敛成一条清晰链路：**唯一写盘中枢** `core/llm/admin.rs`（改 `models.toml`/`.env`，含锁、原子写、校验、Key 脱敏与只入不出）对上暴露 `tomcat model` **CLI** 与 `tomcat serve` **模型管理命令**两个等价门面；前端在侧栏用**自定义下拉**（可用模型 + 分隔符 + Add Models）触发**编辑器区设置中心**（左导航 / 右 Models 页，密文填 Key 即用）；后端把常用模型统一收敛到内嵌 `builtin_models.toml` **单源**（OpenAI 5.4/5.5/5.6、MiMo、DeepSeek、GLM、Kimi、Opus×3），由 `catalog.rs` 解析成运行时 builtin catalog，用 **path-aware endpoint** 修好 GLM 非 `/v1` 地址，并新增 `anthropic-messages` **provider** 接入 Claude；「一个模型能不能用」由「Key 变量是否非空」动态二分，内置预置填个 Key 就地转正——全程 Key 密文、不回吐、`.env` 0600。UI 总分设计详见附录 A。

---

## 附录 A：总分 UI 设计图集（索引）

附录 A 已下沉到子文档 [`05-ui-ascii-baseline.md`](model-management-add-models/05-ui-ascii-baseline.md)，内容不变。
