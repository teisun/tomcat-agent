# 内置工具描述机制跨项目调研

> 关联任务：`T2-P0-005 / T-034` — 补齐全部工具的 `description` / `usage` / `example`，产出 `docs/tool-catalog.md`。  
> 调研范围：`cc-fork-01`、`pi-mono`、`openclaw`、`hermes-agent`。  
> 调研日期：2026-05-02。

---

## 0. 如何阅读本报告

本文有两张容易混淆的表，**先看哪份取决于你当前的角色**：

| 表 | 位置 | 角度 | 你什么时候看 |
|---|---|---|---|
| **A. 推荐字段最小集** | §1（字段表） | "**写代码时**结构体里要有哪些字段" | 准备实现 `BuiltinToolCatalogEntry`、改 `build_tool_definitions()` 时 |
| **B. 跨项目设计采纳表** | §11（映射表） | "**做评审 / 解释决策**时每条设计为什么这么选、抄自谁" | 设计评审、写 ADR、回答"为什么不这么做"时 |
| C. 横向对比 + 打分 | §7 | "四个项目本身的优缺点" | 想了解参考项目原貌、对照打分时 |

阅读路径推荐：

```
快速实施     →  §1（字段最小集） →  §8.1 结构体 →  §10 实施顺序
做技术决策   →  §1.1 关键观察   →  §7 打分    →  §11 采纳表
深入了解某家 →  §3/4/5/6 对应章节
```

---

## 1. 结论摘要

四个项目对“工具描述”基本都分成了三层，只是命名不同：

| 层级 | 作用 | 典型字段 |
|---|---|---|
| LLM schema | 发给模型，决定模型是否会正确调用工具 | `name`、`description`、`parameters` / `input_schema` |
| Runtime / policy | 工具执行、权限、并发、可用性、输出截断 | `toolset`、`isReadOnly`、`checkPermissions`、`requires_env`、`maxResultSize` |
| UI / catalog | 给用户、TUI、帮助页、工具清单展示 | `label`、`displaySummary`、`promptSnippet`、`usage`、`example`、`category` |

对 `pi-rust-wasm` 来说，`T-034` 不应该只是在现有 OpenAI function JSON 里补几句中文描述。更稳的做法是建立一个**内置工具 catalog 单一事实源**，从它派生：

1. LLM function schema；
2. `docs/tool-catalog.md`；
3. TUI / `/help` / 工具选择界面的短说明；
4. 权限审计里的工具分类信息。

**对四个项目逐一交叉验证后**，再压一压字段集。结论是：**不要把 `usage`、`example` 做成运行时一等字段**，四家无一这么做（cc-fork 把示例塞进 `prompt()`、pi-mono 写进 `description` / `promptGuidelines`、OpenClaw 只在 `displaySummary` + 长 `description`、Hermes 写进 schema dict 的 `description`）。模型层只需要稳定的三件套 `name / description / parameters`，`usage` / `example` 留在 markdown 文档与测试 fixture。

**pi-rust-wasm 审批决策**：不设 `prompt_guidelines` / `promptGuidelines` 独立字段；操作规则（何时用、何时不用、替代命令等）**全部写入**各工具的 `description`。全局层面的通用习惯仍可由 [`system_prompt.rs`](../../src/core/llm/system_prompt.rs) 里现有的 `Guidelines:` 段承载，与 per-tool 规则分工。

推荐字段最小集（运行时）：

| 字段 | 必要性 | 说明 |
|---|---:|---|
| `name` | 必须 | 模型调用名，稳定 API |
| `label` | 必须 | UI 展示名，可读 |
| `category` | 必须 | `filesystem` / `exec` / `config` / `search` 等，用于分组与权限分类 |
| `description` | 必须 | **单一事实源**，发给 LLM；能力说明 + **规则**（何时用 / 何时别用 / 关键约束）写在一起 |
| `parameters` | 必须 | JSON Schema，参数级 `description` 必须完整 |
| `display_summary` | 建议 | UI / catalog 一行短文；缺省时由 `description` 取首段（OpenClaw 风格） |
| `permission_scope` | 建议 | `FS` / `Exec` / `Config` / `Network`，与 audit 枚举对齐（呼应 T-033） |
| `read_only` / `destructive` | 建议 | 权限、审计、UI 风险提示用 |
| `search_hint` | 可选 | 关键词，用于未来工具搜索 / deferred loading（cc-fork 风格） |

`usage` / `examples` 只活在 `docs/tool-catalog.md` 和测试 fixture 里，不进运行时结构。

### 1.1 调研后的关键观察（四项目交集）

四个项目的字段命名各有差异，但有几条**共识**值得直接抄：

1. **模型 schema 三件套是硬通货**：`name + description + parameters` 跨四家完全一致。Anthropic / OpenAI / Google 等 provider 对工具描述的需求收敛在这个三件套上。
2. **没有任何一家把 `usage` / `example` 做成顶层字段**。示例若进入 LLM，是嵌进 description 文本（cc-fork 用 `<example>...</example>`、Hermes 把 V4A 格式塞进参数 description）。
3. **短描述与长描述同源**。OpenClaw 用 `summarizeToolDescriptionText` 从长 `description` 提炼首段做 catalog 短文；只在需要覆盖时才提供 `displaySummary`。这避免了维护两套字符串带来的漂移。
4. **「Available tools」清单可门控**。pi-mono 只把声明了 `promptSnippet` 的工具放入系统提示工具清单；这是**显式**控制 token 预算的好钩子。
5. **禁用工具的处理**：多数项目（pi-mono / openclaw / cc-fork）靠**不把工具放进发给模型的列表**解决大半问题。Hermes **额外**在 `model_tools.py` 里按启用集**改写**各工具 `description` 文本（剔除指向未启用工具的句子）。**pi-rust-wasm 审批**：**不采纳** Hermes 式 description rewrite / `<tool:NAME>` marker（见 §11.2）；保持 catalog→function schema **直通**，暴露控制以后亦以过滤 `tool_definitions`（及权限门）为主，依赖模型在可见工具集内调度。

### 1.2 一张图理解整体方案

```
                     ┌──────────────────────────────────────┐
                     │   BuiltinToolCatalogEntry  (§8.1)    │
                     │   ── 单一事实源 (Single Source) ──   │
                     │                                      │
                     │   name / label / category            │
                     │   description ◄── 唯一长文（含规则）    │
                     │   parameters (JSON Schema)           │
                     │   permission_scope / read_only / ... │
                     └──┬──────┬──────────┬──────────┬──────┘
                        │      │          │          │
            派生 (derive)│      │          │          │
              ┌─────────▼─┐ ┌──▼─────┐ ┌──▼──────┐ ┌─▼────────┐
              │ LLM       │ │ docs/  │ │ TUI /   │ │ audit /  │
              │ function  │ │ tool-  │ │ /help / │ │ permission│
              │ schema    │ │catalog │ │ summary │ │ 分类      │
              │           │ │ .md    │ │         │ │           │
              │ §8.2      │ │ §9     │ │ §8.5    │ │ §8.6 / T-033│
              └───────────┘ └────────┘ └─────────┘ └───────────┘
                                    │
                                    ▼
                         LLM 请求中的 tools：`build_tool_definitions()`
                         派生自 catalog；`function.description` 使用条目原文，
                         不做 Hermes 式跨工具引用 rewrite（§11.2）。
```

读法：**任何工具相关的对外形态都从 catalog 派生**，不再有手写漂移。

---

## 2. 当前 `pi-rust-wasm` 的缺口

`pi-rust-wasm` 现在有两套工具描述事实源：

1. `src/core/tools/registry.rs` 的通用 `Tool` 结构体已有 `name`、`label`、`description`、`parameters`、`plugin_id`、`is_enabled`、`created_at`。
2. `src/api/chat/mod.rs::build_tool_definitions()` 里手写了 OpenAI function schema，目前多数工具只有非常短的 `description`，例如“读取文件内容”“执行 bash 命令”；`config_get` / `config_set` 的描述则已经很长。

问题是：

- 内置工具 schema 与 `ToolRegistry` 没有统一 catalog，后续很容易漂移。
- `description` 既承担“模型调用说明”，又被潜在地拿去做人类展示，粒度不一致。
- 没有 `usage` / `example`，很难产出高质量 `docs/tool-catalog.md`。
- 权限语义（例如 `Bash` 应为 `Exec`）没有和工具描述绑定，容易和 T-033 同类问题再次漂移。

---

## 3. `cc-fork-01`：Tool 类型极重，prompt 是模型说明主入口

相关文件：

- `cc-fork-01/src/Tool.ts`
- `cc-fork-01/src/utils/api.ts`
- `cc-fork-01/src/tools/FileReadTool/FileReadTool.ts`
- `cc-fork-01/src/tools/FileReadTool/prompt.ts`
- `cc-fork-01/src/tools/FileEditTool/FileEditTool.ts`
- `cc-fork-01/src/tools/FileEditTool/prompt.ts`
- `cc-fork-01/src/tools/BashTool/BashTool.tsx`
- `cc-fork-01/src/tools/BashTool/prompt.ts`

### 3.1 数据结构

`Tool` 类型很重，单个工具不只是 schema，还包含：

- `name`
- `aliases`
- `searchHint`
- `description(input, options)`：动态短描述，更多用于 UI / summary。
- `inputSchema` / `inputJSONSchema`
- `prompt(options)`：真正发给模型的长工具说明。
- `userFacingName`
- `getToolUseSummary`
- `getActivityDescription`
- `isConcurrencySafe`
- `isReadOnly`
- `isDestructive`
- `checkPermissions`
- `renderToolUseMessage`
- `renderToolResultMessage`
- `toAutoClassifierInput`

`buildTool()` 给常见字段提供 fail-closed 默认值，例如默认非并发安全、默认非只读。

### 3.2 LLM schema 生成

`src/utils/api.ts` 把工具转成 Anthropic tool schema 时，用的是：

- `name: tool.name`
- `description: await tool.prompt(...)`
- `input_schema: zodToJsonSchema(tool.inputSchema)` 或 `tool.inputJSONSchema`

也就是说，Claude Code 风格里，“模型看的 description”不是 `description()` 方法，而是 `prompt()` 方法。`description()` 更像运行期 / UI 的短描述。

### 3.3 代表性工具

`FileReadTool`：

- `description()` 返回短句：`Read a file from the local filesystem.`
- `prompt()` 返回长说明，包含用法规则：必须绝对路径、默认读取行数、offset/limit、图片/PDF/notebook、目录应走 Bash `ls` 等。
- `searchHint` 是 `read files, images, PDFs, notebooks`。
- `isReadOnly() = true`，`isConcurrencySafe() = true`。

`FileEditTool`：

- `description()` 是短句。
- `prompt()` 由 `getEditToolDescription()` 生成，包含必须先读文件、精确匹配、不要把行号前缀带入 `old_string`、优先编辑现有文件等规则。

`BashTool`：

- `description(input)` 会优先使用模型传入的 `description` 参数，否则是 `Run shell command`。
- `prompt()` 是长 shell 使用规范，包含后台任务、git、PR 等专项规则。
- `isReadOnly(input)` 通过命令语义分析判断。

### 3.4 可借鉴点

值得借鉴：

- **短描述与长 prompt 分离**：`description` 给 UI / summary，`prompt` 给 LLM。
- **工具说明可以动态生成**：根据权限、模型、特性开关、可用 agent 动态裁剪。
- **工具有 `searchHint`**：对未来工具搜索 / deferred tools 有价值。
- **工具语义和权限同居一处**：`isReadOnly`、`isDestructive`、`checkPermissions` 与工具定义绑定。

不建议直接照搬：

- `Tool` 类型过重，`pi-rust-wasm` 现在不需要把 TUI 渲染、权限、schema、执行全塞进一个 trait / struct。
- `prompt()` 作为 provider schema 的 description 很强，但也会增加 token 成本；需要控制长度。

---

## 4. `pi-mono`：TypeBox schema + AgentTool / ToolDefinition 双层

相关文件：

- `pi-mono/packages/ai/src/types.ts`
- `pi-mono/packages/agent/src/types.ts`
- `pi-mono/packages/coding-agent/src/core/extensions/types.ts`
- `pi-mono/packages/coding-agent/src/core/tools/index.ts`
- `pi-mono/packages/coding-agent/src/core/tools/read.ts`
- `pi-mono/packages/coding-agent/src/core/tools/bash.ts`
- `pi-mono/packages/coding-agent/src/core/tools/tool-definition-wrapper.ts`
- `pi-mono/packages/ai/README.md`

### 4.1 数据结构

底层 `@mariozechner/pi-ai` 的 `Tool` 非常小：

- `name`
- `description`
- `parameters`

`@mariozechner/pi-agent-core` 的 `AgentTool` 在其上增加：

- `label`
- `prepareArguments`
- `execute`

`pi-coding-agent` 的 `ToolDefinition` 再增加：

- `promptSnippet`
- `promptGuidelines`
- `renderCall`
- `renderResult`

这形成了清晰分层：

- `Tool`：模型协议层；
- `AgentTool`：可执行运行时层；
- `ToolDefinition`：面向 coding-agent 的 UI / prompt / extension 层。

### 4.2 代表性工具

`read` 工具：

- 参数用 TypeBox 定义，参数级 `description` 直接挂在 `Type.String()` / `Type.Optional()` 上。
- `description` 很具体：支持文本和图片，图片作为 attachment，文本按行数/KB 截断，建议用 offset/limit 继续读取。
- `promptSnippet` 是短句：`Read file contents`。
- `promptGuidelines` 是规则列表：例如用 read 看文件而不是 `cat` / `sed`。

`bash` 工具：

- 参数 schema 很薄：`command` + 可选 `timeout`。
- 运行期支持流式 partial update、输出截断、完整输出落临时文件。
- UI 渲染逻辑与 schema / description 分开。

工具集合：

- `codingTools = [readTool, bashTool, editTool, writeTool]`
- `readOnlyTools = [readTool, grepTool, findTool, lsTool]`
- `createCodingToolDefinitions(cwd)` / `createReadOnlyToolDefinitions(cwd)` 会按场景生成工具定义。

### 4.3 可借鉴点

值得借鉴：

- **schema 层保持极小**：`name/description/parameters` 是跨 provider 的稳定核心。
- **promptSnippet**（短句进工具清单）仍是好思路；**promptGuidelines** 在 pi-mono 里单独列出。**pi-rust-wasm 审批不设独立 guidelines 字段**，同类规则写入各工具 `description`（见 §1 审批决策）。
- **工具分组是函数化的**：coding / read-only / all 由构造函数生成，便于未来 plan mode / readonly mode。
- **TypeBox 参数 description 很完整**：参数说明不是文档附属，而是 schema 一部分。

不建议直接照搬：

- TypeBox 是 TS 生态；Rust 里更适合用 `serde_json::json!` 或自定义 builder 生成 JSON Schema。
- `renderCall` / `renderResult` 当前对 `pi-rust-wasm` 不是 T-034 必需项。

---

## 5. `openclaw`：复用 pi 工具，同时有独立 catalog / profile / plugin 层

相关文件：

- `openclaw/src/agents/tool-catalog.ts`
- `openclaw/src/agents/tool-description-presets.ts`
- `openclaw/src/agents/pi-tools.ts`
- `openclaw/src/agents/pi-tool-definition-adapter.ts`
- `openclaw/src/agents/tools/common.ts`
- `openclaw/src/plugins/tools.ts`
- `openclaw/extensions/firecrawl/src/firecrawl-search-tool.ts`

### 5.1 数据结构

OpenClaw 复用 `@mariozechner/pi-coding-agent` 的工具体系，但自己又加了两层：

1. `tool-catalog.ts`：面向工具目录 / profile 的静态 catalog。
   - `id`
   - `label`
   - `description`
   - `sectionId`
   - `profiles`
   - `includeInOpenClawGroup`
2. `AgentToolWithMeta`：在 `AgentTool` 上扩展：
   - `ownerOnly`
   - `displaySummary`

`pi-tool-definition-adapter.ts` 把 `AgentTool` 适配成 `pi-coding-agent` 的 `ToolDefinition`：

- `name`
- `label`
- `description`
- `parameters`
- `execute`

### 5.2 代表性工具

`tool-catalog.ts` 把核心工具按 section 分类：

- `fs`: `read` / `write` / `edit` / `apply_patch`
- `runtime`: `exec` / `process` / `code_execution`
- `web`: `web_search` / `web_fetch` / `x_search`
- `sessions`: `sessions_list` / `sessions_history` / `sessions_send` / `sessions_spawn`
- `messaging`、`automation`、`agents`、`media` 等

`tool-description-presets.ts` 把常用短描述抽成常量或函数，例如：

- `EXEC_TOOL_DISPLAY_SUMMARY = "Run shell commands that start now."`
- `describeSessionsSpawnTool()` 返回多句组合说明。

`extensions/firecrawl/src/firecrawl-search-tool.ts` 展示了插件工具写法：

- TypeBox 参数 schema；
- 参数级 `description`；
- 顶层 `description`；
- `execute` 内用 SDK helper 读取/校验参数。

### 5.3 可借鉴点

值得借鉴：

- **catalog 不等于 provider schema**：OpenClaw 的 `tool-catalog.ts` 更像“产品工具目录”，不承担完整 LLM 指令。
- **profile / section 很有价值**：可以自然生成 `docs/tool-catalog.md` 的章节，也能支撑不同模式启用不同工具。
- **displaySummary 与 description 同源**：`tools-effective-inventory.ts::summarizeToolDescriptionText` 把长 `description` 截到首段（或第一个空行前的 N 字符）作为短文，**仅在需要覆盖时**才提供 `displaySummary`。这避免了短/长两套字符串各自漂移。Gateway 协议 `tools.effective` 同时返回 `description`（已摘要）与 `rawDescription`（完整长文），消费端按需选用。`pi-rust-wasm` 可以照抄这个摘要算法，长 description 是唯一事实源，UI 短文由它派生。
- **插件工具也走同一字段形状**：内置工具和插件工具都能统一进 catalog。
- **Gateway 协议刻意瘦**：`tools.catalog` 不返回 `parameters`、`usage`、`example`，只携带 id/label/description/section/profiles，留给前端按需展开。这条对 `pi-rust-wasm` 未来 TUI / `/help` 接口设计也是一个朴素提示——目录 API 与执行 API 分层。

不建议直接照搬：

- OpenClaw 工具体系是多 channel / plugin / gateway 产品形态，`pi-rust-wasm` 当前无需完整 profile policy。
- `tool-catalog.ts` 的描述偏短，不能单独作为 LLM description。

---

## 6. `hermes-agent`：OpenAI function schema 自注册 + toolset 过滤

相关文件：

- `hermes-agent/tools/registry.py`
- `hermes-agent/model_tools.py`
- `hermes-agent/tools/file_tools.py`
- `hermes-agent/toolsets.py`

### 6.1 数据结构

Hermes 的中心是 `registry.register()`。每个工具模块 import 时自注册：

- `name`
- `toolset`
- `schema`
- `handler`
- `check_fn`
- `requires_env`
- `is_async`
- `description`
- `emoji`
- `max_result_size_chars`

`ToolEntry.description` 默认取 `schema["description"]`。最终 `registry.get_definitions()` 统一返回 OpenAI function 格式：

```json
{
  "type": "function",
  "function": {
    "name": "...",
    "description": "...",
    "parameters": { "...": "..." }
  }
}
```

`toolsets.py` 定义工具组，`model_tools.py::get_tool_definitions()` 按 enabled / disabled toolsets 过滤，并按实际可用工具动态改写描述，避免提到未启用工具。

### 6.2 代表性工具

`tools/file_tools.py` 里四个文件工具的 schema 很完整：

- `read_file`：说明替代 `cat/head/tail`，输出行号格式，offset/limit，大文件限制，不能读图片/二进制。
- `write_file`：说明会完整覆盖文件、自动创建父目录、 targeted edit 应用 `patch`。
- `patch`：说明 targeted find-and-replace / V4A patch，两种模式都写进 description。
- `search_files`：说明替代 `grep/rg/find/ls`，区分 content search / file search / output modes。

注册时还补 `emoji` 和 `max_result_size_chars`，用于 UI 与输出预算。

### 6.3 可借鉴点

值得借鉴：

- **自注册模式简单直接**：每个工具的 schema 和 handler 在同一个文件附近。
- **toolset 是一等概念**：可以非常直接地生成“当前启用工具清单”。
- **description 非常面向模型行为**：明确告诉模型“用这个替代 cat/head/tail”“大文件用 offset/limit”。
- **动态裁剪跨工具引用（Hermes 特有）**：`model_tools.py::get_tool_definitions()` 在拼最终 schema 前会扫一遍 `description` 文本，把指向「未启用工具」的句子删掉或重写。**pi-rust-wasm 不采纳**：与其它调研项目一致，以**过滤发给模型的工具列表**为主；不在此处复制 Hermes 的文本改写管线（理由见 §11.2）。

不建议直接照搬：

- Python import-time 自注册在 Rust 里不自然；Rust 更适合静态数组或 builder。
- OpenAI schema dict 直接散落在每个工具文件，长期可能缺少类型约束。

---

## 7. 横向对比

### 7.1 字段/能力速览

| 项目 | 模型 schema 来源 | 人类 catalog | 参数 schema | 工具分组 | UI 字段 | 权限/风险字段 |
|---|---|---|---|---|---|---|
| `cc-fork-01` | `Tool.prompt()` + `inputSchema` | 隐含在工具对象 / UI render | Zod / JSON Schema | 工具数组 + deferred/search | 很丰富 | 很丰富 |
| `pi-mono` | `Tool.description` + TypeBox `parameters` | `ToolDefinition.promptSnippet/promptGuidelines` | TypeBox | `codingTools` / `readOnlyTools` | `label` / render | 较轻 |
| `openclaw` | pi 工具 + adapter | `tool-catalog.ts` | TypeBox | profile / section / policy | `label` / `displaySummary` | `ownerOnly` / policy |
| `hermes-agent` | OpenAI function schema dict | toolsets / CLI tools config | JSON Schema dict | `toolset` | `emoji` | `check_fn` / `requires_env` |
| `pi-rust-wasm` 当前 | `build_tool_definitions()` 手写 JSON | 暂缺 | JSON Schema dict | 暂缺系统 catalog | `Tool.label` 有但未统一 | permission/audit 分散 |
| `pi-rust-wasm` 目标 | 由 `BuiltinToolCatalogEntry.description` + `parameters` 派生 | `docs/tool-catalog.md` 由 catalog 生成 | JSON Schema（参数级 description 全覆盖） | `category` + 未来 profile | `display_summary`（缺省由 description 摘要派生） | `permission_scope` 与 audit 枚举对齐 |

### 7.2 优缺点 + 可借鉴度打分

> 打分维度：**对 pi-rust-wasm 的可借鉴度**（不是项目本身好坏，而是"放进 Rust + WASM + 单代理 + chat-first"的 pi-rust-wasm 是否合身）。  
> 1 分 = 不建议照搬；3 分 = 关键想法可借鉴；5 分 = 强烈建议照抄。

| 项目 | 优点 | 缺点 / 不适合点 | 借鉴度 | 借鉴关键点（详见 §11） |
|---|---|---|---:|---|
| `cc-fork-01` (Claude Code) | • LLM 描述 (`prompt()`) 与 UI 描述 (`description()`) 严格分离<br>• `<example>` 嵌入式样例足够稳<br>• `searchHint` 为大型工具集预留 deferred loading<br>• 权限模型 (`isReadOnly`/`isDestructive`) 完整 | • `Tool` 接口超重（10+ 字段），Rust 移植成本高<br>• Zod → JSON Schema 转换链对 Rust 不直接适用<br>• 多 channel/multi-tool dispatch 不是单代理需要的 | **3** | description 内嵌 `<example>` 单例；`search_hint` 字段保留可选 |
| `pi-mono` | • 模型 schema 极薄 (`name+description+parameters`)，跨 provider 稳<br>• `promptSnippet` 显式门控 system prompt 工具清单（token 预算）<br>• `promptGuidelines` 把规则单独列（上游做法）<br>• 函数化工具集 (`codingTools` / `readOnlyTools`) 易切模式 | • TypeBox 是 TS 生态<br>• `renderCall` / `renderResult` UI 与 schema 在同一对象，Rust 拆开更自然 | **5** | **字段最小集**照抄 `name/description/parameters`；**不设**独立 `prompt_guidelines`，规则并入 `description`（§1） |
| `openclaw` | • `summarizeToolDescriptionText` 解决"长短描述漂移"<br>• Gateway `tools.catalog` 与 `tools.effective` 分层（瘦目录 vs 完整执行）<br>• `displaySummary` 仅作覆盖项<br>• profile / section 天然支持 plan / read-only 模式 | • profile policy / multi-channel 太重，T-034 用不到<br>• `tool-catalog.ts` 短描述不足以喂 LLM（需要 `description` 配合） | **4** | 摘要算法直接抄；profile 暂不引入；`display_summary` 改成可选覆盖 |
| `hermes-agent` | • description 写得最"模型导向"（明确 when / when-not）<br>• `model_tools.py` 跨工具引用动态 rewrite（仅此一家在该层做文本改写）<br>• `toolset` + `check_fn` + `requires_env` 一站搞定可用性判定<br>• schema dict 与 handler 同文件，认知近 | • Python import-time 自注册不适合 Rust 静态化思路<br>• schema dict 散落各处缺类型约束<br>• `emoji` / `max_result_size_chars` 弱关联，分量轻 | **4** | description **写法**学 Hermes；**rewrite 管线不采纳**（§11.2） |

总借鉴度：`pi-mono > openclaw ≈ hermes-agent > cc-fork-01`。这个排序**不是**对原项目品质的评价，而是**对 pi-rust-wasm 当前需求**（单代理 + Rust + chat-first + 还没到 plugin/profile 阶段）的合身度。

打分理由可视化：

```
借鉴度  pi-rust-wasm 当前阶段适配
        ┌──────────────────────────────┐
pi-mono │■■■■■  5  字段集几乎照抄      │
openclaw│■■■■   4  摘要算法 + 单一事实源│
hermes  │■■■■   4  description 写法（rewrite 不采纳）│
cc-fork │■■■    3  仅取嵌入式 example + searchHint│
        └──────────────────────────────┘
```

---

## 8. 对 T-034 的落地建议

### 8.1 建一个内置工具 catalog 单一事实源

按交集结论，运行时结构体只保留模型层与策略层；`usage` / `examples` 留在 markdown / 测试 fixture，不进结构体：

```rust
pub struct BuiltinToolCatalogEntry {
    pub name: &'static str,
    pub label: &'static str,

    /// 可选 UI / 文档分组（filesystem / exec / config / search / web / media / vc ...）。
    /// 缺省时由 `permission_scope` 自动派生（见映射表）；只有 UI 想要更细分类时才显式覆盖。
    pub category: Option<ToolCategory>,

    /// 单一事实源；同时发给 LLM。
    /// 写法见 8.3：what / when-to-use / when-not / 关键约束 + 操作规则。
    pub description: &'static str,

    /// 可选：覆盖默认摘要。缺省时由 `summarize_tool_description(description)` 派生（OpenClaw 风格）。
    pub display_summary: Option<&'static str>,

    pub parameters: serde_json::Value,

    /// 权威字段，喂权限系统与 audit；不被 UI 维度污染。
    pub permission_scope: PermissionScope,
    pub read_only: bool,
    pub destructive: bool,

    /// 可选：未来工具搜索 / deferred loading 用（cc-fork 风格）。
    pub search_hint: Option<&'static str>,
}
```

#### `category` ↔ `permission_scope` 不合并、但提供默认派生

两字段服务对象不同（**`category` = UI/文档分组**；**`permission_scope` = 权限/审计 scope，呼应 T-033**），合并会丢表达力：

- `search` 是 UI 分类，但权限是 `FS-read`；
- `web` / `media` / `vc` 等未来分类可能共用 `Network` / `Exec`；
- 一个工具可能跨多个 scope（写盘 + 联网），但 UI 仍想标一个 category。

因此**保持两字段**，并约定缺省派生：

| `permission_scope` | 派生 `category` 默认 |
|---|---|
| `FS` | `filesystem` |
| `Exec` | `exec` |
| `Config` | `config` |
| `Network` | `web` |

少数需要更细的（如 `read_file` / `list_dir` 同为 `FS-read`，但 UI 上想区分 `filesystem` 与 `search`）显式覆盖即可。`audit` / T-033 一律以 `permission_scope` 为权威。

不入结构体的：

- `usage` / `examples`：放 `docs/tool-catalog.md` 与 `tests/fixtures/tool_examples/*.json`，避免 token 成本与 prompt cache 失效，也避免维护两份。
- `renderCall` / `renderResult`：T-034 暂不需要，留给 TUI 后续。

位置建议：`src/core/tools/catalog.rs`。后续插件工具如果要进统一 catalog，留在 `core/tools` 更合适。

### 8.2 让 OpenAI function schema 从 catalog 派生

把 `src/api/chat/mod.rs::build_tool_definitions()` 改为遍历 catalog 生成：

- `function.name = entry.name`
- `function.description = entry.description`（**直通**，不做 Hermes 式跨工具引用 rewrite；见 §11.2）
- `function.parameters = entry.parameters`

这样 `docs/tool-catalog.md`、LLM schema、测试断言都能共享同一事实源。操作规则写在 `description` 内，随 `function.description` 发给模型；`display_summary` 不发给模型，仅供 UI 与 `/help` 使用。

#### description 派生流向（一图说清）

```
                    entry.description (catalog 源文，含规则与可选 <example>)
                            │
        ┌───────────────────┼─────────────────────┐
        ▼                   ▼                     ▼
  ┌──────────────┐   ┌─────────────────┐   ┌────────────────┐
  │ OpenAI       │   │ summarize_tool_ │   │ tool-catalog.md│
  │ function     │   │ description     │   │  生成器         │
  │ schema       │   │ (§8.5)          │   │  (§9)          │
  └──────┬───────┘   └────────┬────────┘   └────────┬───────┘
         │                    │                     │
         ▼                    ▼                     ▼
  function.description    display_summary       README/Catalog
   发给 LLM（与 catalog    UI / /help            人类可读文档
   条目一致）               (首段, ≤120 字)

  display_summary 取值规则：
    if entry.display_summary.is_some() → use it (覆盖)
    else                                → summarize(description)  ← OpenClaw 风格
```

### 8.3 `description` 的写法建议

参考 Hermes 和 Claude Code，`description` 应该回答四件事：

1. 这个工具做什么；
2. 什么时候应该用它；
3. 什么时候不应该用它；
4. 关键约束 / 风险是什么。

示例：

```text
Read UTF-8 text files from the local filesystem. Use this instead of shell commands such as cat/head/tail when inspecting file contents. Supports offset/limit for large files. Does not read directories; use list_dir for directories.
```

危险工具（例如 Bash / config_set / write_file）还应该写清权限与确认语义。

### 8.4 `usage` / `example` 不进运行时

四个项目无一把 `usage` 或 `example` 做成顶层 LLM 字段，理由汇总：

- **token 成本与 prompt cache**：示例每次都会进 prompt，对长会话尤其昂贵。
- **更新摩擦**：人看的示例要求格式漂亮，模型只关心规则与边界，频次不一致。
- **provider 差异**：OpenAI / Anthropic / Google 对长 description 的截断策略不同，复杂结构容易走样。

推荐分层：

- 模型可见：`description`（必要：能力说明 + 规则 + 可选 1 个嵌入式短例，cc-fork `<example>` 风格）。
- 仅文档可见：`docs/tool-catalog.md` 完整 usage / example / 反例。
- 仅测试可见：`tests/fixtures/tool_examples/*.json`，作为 schema 验证 fixture，可保证示例长期可执行。

### 8.5 摘要算法（OpenClaw 风格）

新增 helper：

```rust
/// 抽 description 首段（到首个空行/句号截断，限 N 字）。
/// 当 entry.display_summary 为 None 时使用。
pub fn summarize_tool_description(text: &str, max_chars: usize) -> String { /* ... */ }
```

UI、`/help`、catalog 索引页都用这个 helper，避免维护两套字符串。

### 8.6 加漂移测试

建议补**三类**测试：

1. 所有内置工具 `description` 非空、长度在合理区间（如 80–800 字符），`parameters` 内每个字段都有 `description`。
2. `build_tool_definitions()` 输出的工具名集合 == catalog 工具名集合 == `tests/fixtures/tool_examples/` 文件名集合（若采用 fixture 约定）。
3. `permission_scope` 与 audit scope 枚举一致，闭合 T-033 这类错配。

可选：`docs/tool-catalog.md` 由 `cargo run --bin gen-tool-catalog` 生成，CI 校验 git diff 为空，杜绝文档漂移。

---

## 9. 推荐的 `docs/tool-catalog.md` 结构

建议按 category 分组：

```md
# Tool Catalog

## Filesystem

### read_file

- Label: Read File
- Permission: FS / readonly
- Model description: ...
- Usage:
  - ...
- Parameters:
  - path: ...
  - offset: ...
- Examples:
  - Read a short file: `{ "path": "..." }`

## Execution

### execute_bash

- Permission: Exec
- Risk: may mutate filesystem or start processes
- Usage:
  - ...
```

这样能同时服务：

- 工程师 review；
- 用户理解工具能力；
- 后续 TUI `/tools` 或 `/help`；
- 权限审计对照。

---

## 10. 最小实施顺序

1. 新增 catalog 类型与内置工具静态定义，先覆盖当前 `build_tool_definitions()` 里的 7 个工具：`read_file`、`write_file`、`edit_file`、`execute_bash`、`list_dir`、`config_get`、`config_set`。
2. 改 `build_tool_definitions()` 从 catalog 派生。
3. 生成或手写 `docs/tool-catalog.md`，确保每个工具有 usage 和 example。
4. 加 catalog 漂移测试（§8.6 三类）。
5. 若 T-033 同时做，给 `execute_bash` 标 `permission_scope = Exec`，并在审计测试里断言。

---

## 11. 最终建议（跨项目设计采纳表）

> **本表与 §1 的字段表角色不同**（参见 §0）：  
> §1 = "结构体里要写哪些字段"；  
> §11 = "每个设计决策抄自哪家、为什么抄、为什么没抄别的方案"。  
> 写代码看 §1 + §8；做评审 / 写 ADR / 解释技术决策看本节。

### 11.1 采纳决策表

| # | 设计决策 | 采纳来源 | **为何采纳** | **为何不采用替代方案** | 在 §1/§8 的体现 |
|---:|---|---|---|---|---|
| 1 | 模型 schema 只保留 `name + description + parameters` | `pi-mono`、`hermes-agent` | 跨 4 家完全一致；provider 通用；token 成本低；prompt cache 友好 | cc-fork 的重 `Tool` 接口字段多但和单代理需求弱关联，Rust 移植成本不划算 | §1 字段表 + §8.1 |
| 2 | `description` 作为**单一事实源** | `openclaw` (`summarizeToolDescriptionText`) | 长短描述同源避免漂移；写一处而非两处；与"任意工具产物从 catalog 派生"原则一致 | cc-fork 的 `prompt()` + `description()` 双字段同样能解耦但需维护两份字符串，对 7 个内置工具的小规模反而是负担 | §1.1 共识 #3 + §8.5 |
| 3 | `display_summary` 改为 `Option<>`（可覆盖） | `openclaw` | 95% 情况下摘要算法够用；保留覆盖项给极少数（如 `config_set` 这种长描述）写更精炼短文 | 如果设为必填会要求每个工具都写一遍短文，且与长 description 漂移风险回归 | §8.1 + §8.5 |
| 4 | **不设** `prompt_guidelines`；规则写入 `description` | 项目审批（对比 pi-mono 可选拆条） | 字段更少；规则与工具协议同处一处，与 Hermes「全文入 description」一致；全局习惯仍由 `system_prompt` 的 Guidelines 段承担 | pi-mono 的 `promptGuidelines` 适合其 TS/prompt 拼装链；本项目 catalog 不镜像，避免多一份字符串漂移 | §1、§8.1、§8.3 |
| 5 | description 内嵌 1 个 `<example>` | `cc-fork-01` | 最稳的 few-shot 形式；不必为每个工具单开 examples 字段就能给模型一个具体调用样例 | hermes 把示例直接写在 description 里也行，但 `<example>` 标签可被生成器/测试识别，便于自动校验 | §8.3 + §8.4 |
| 6 | `usage` / `examples` **不进运行时结构** | 4 家共识 | token 成本明显（每次会话都进 prompt）；维护人看的格式与机看的边界节奏不同；provider 对长 description 截断行为不一致 | 若进结构体，必须考虑序列化、长度上限、CI 校验、跨 provider 兼容，复杂度激增 | §1.1 共识 #2 + §8.4 |
| 7 | `permission_scope` 与 audit 枚举对齐 | 自 + 呼应 T-033 | 当前 `Bash = FS` 这类错配根源就是工具描述与权限分类没绑定；放进结构体可强制一致 | 不绑定就只能靠人脑+code review，已经吃过亏 | §8.1 + §8.6 |
| 8 | `search_hint: Option<&str>` 预留 | `cc-fork-01` | 7 个工具时不需要，但插件工具一旦增多到 30+，工具搜索/deferred loading 会成为刚需，预留字段成本几乎为零 | 若不预留，将来加字段需要改所有 catalog 静态项；预留对当前实现完全无负担 | §8.1 |
| 9 | catalog → docs 由脚本生成 + CI diff 校验 | `openclaw` 思路 + Hermes toolsets | 防止"代码改了文档没改"这种最常见漂移；CI 一个 git diff 检查就闭环 | 手动维护 `docs/tool-catalog.md` 几乎一定会过时 | §8.6 + §9 |
| 10 | `category` 与 `permission_scope` **不合并**，但 `category` 可由 `permission_scope` 默认派生 | 项目审批 | 二者维度不同：`category` 服务 UI/文档分组（`search` / `web` / `media` / `vc` ...），`permission_scope` 是审计权威枚举（呼应 T-033）；合并会丢 `search`、`web` 等 UI 分类，且让权限语义被 UI 维度污染 | 完全合并：UI 失去细分类、T-033 的 audit 语义被偷换；只留 `category`：audit 失去权威字段，权限映射要绕一道 | §8.1 |

### 11.2 显式**不做**的事

| 不做 | 来源诱惑 | 不做的理由 |
|---|---|---|
| 引入 `prompt() / description() / userFacingName / isReadOnly / isDestructive / inputSchema` 全套 | `cc-fork-01` 的 `Tool` 接口 | 7 个内置工具不需要这么重的抽象；`description` + `permission_scope` + `read_only`/`destructive` 三件已覆盖 |
| 在结构体加 `usage` / `examples` 字段 | T-034 任务标题字面 | 见决策 #6；任务标题"补 usage/example"应被理解为产出文档而非运行时字段 |
| 在 catalog 增加 `prompt_guidelines` | `pi-mono` | 见决策 #4；规则并入 `description`，不设独立字段 |
| 把 `category` 与 `permission_scope` 合并成单一枚举 | 「字段越少越好」直觉 | 见决策 #10；二者服务对象不同（UI 分组 vs 审计 scope），合并丢 UI 细分类并污染权限语义 |
| Hermes 式 `rewrite_cross_refs` / `<tool:NAME>` marker | `hermes-agent` (`model_tools.py`) | **项目审批**：保持 catalog→schema 直通；与 pi-mono / openclaw 一致以**过滤 `tool_definitions`** 为主；强模型在可见工具集内调度即可，避免 marker 维护与额外管线 |
| 让 `build_tool_definitions()` 继续手写 OpenAI JSON | 现状惯性 | 漂移源头；§8.2 改为派生 |
| 引入 OpenClaw profile policy / multi-channel | OpenClaw 完整方案 | 超出 T-034 范围；当前是单代理，profile 等真要做 plan/read-only 时再说 |
| Python import-time 自注册风格 | `hermes-agent` | Rust 静态数组 + builder 更自然；编译期可校验，不用运行时反射 |

### 11.3 采纳关系图

```
                   pi-rust-wasm 的 BuiltinToolCatalogEntry
                                  │
        ┌─────────────────┬───────┴───────┬──────────────────┐
        │                 │               │                  │
   ┌────┴─────┐     ┌─────┴──────┐  ┌─────┴──────┐    ┌──────┴─────┐
   │ pi-mono  │     │  openclaw  │  │hermes-agent│    │ cc-fork-01 │
   │ (5/5)    │     │  (4/5)     │  │  (4/5)     │    │  (3/5)     │
   ├──────────┤     ├────────────┤  ├────────────┤    ├────────────┤
   │ 字段最小集│     │summarize_  │  │description │    │ <example>  │
   │ name/desc/│     │tool_desc   │  │含规则+when/│    │ 嵌入式示例 │
   │ parameters│     │display_    │  │when-not +  │    │            │
   │ (无独立   │     │summary 可选│  │规则同条    │    │ search_hint│
   │ guidelines)│    │            │  │（无 rewrite）│  │ 预留        │
   └──────────┘     └────────────┘  └────────────┘    └────────────┘

   ✘ 不采纳:  cc-fork 重 Tool 接口 / openclaw profile / hermes 自注册 / hermes description rewrite
```

### 11.4 最小可落地路径（优先级从高到低）

1. 立 `BuiltinToolCatalogEntry`（§8.1）+ `summarize_tool_description()`（§8.5）。
2. 静态填 7 个内置工具：`read_file`、`write_file`、`edit_file`、`execute_bash`、`list_dir`、`config_get`、`config_set`。
3. 改 `build_tool_definitions()` 从 catalog 派生（§8.2）；`function.description` **直通** catalog 条目。
4. 加 §8.6 的 3 类漂移测试。
5. `docs/tool-catalog.md` 用脚本生成，CI 校验 diff。
6. 顺便闭合 T-033：给 `execute_bash.permission_scope = Exec`，audit 测试断言一致。

完成后，T-034 的产物不只是"补几句文案"，而是一条可持续的工具描述基础设施，可以无痛接住后续插件工具、TUI 工具面板、plan / read-only 模式。
