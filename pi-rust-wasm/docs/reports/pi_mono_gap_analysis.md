# pi-rust-wasm 与 pi-mono 差距分析报告

**日期**：2026-03-22  
**基准**：pi-mono（TypeScript Monorepo，上游参考实现） vs pi-rust-wasm（Rust + WasmEdge 重写）  
**方法**：对照 pi-mono 文档快照（`pi-mono_docs/`）、pi-rust-wasm 源码与 `TASK_BOARD.md` 当前状态，按 12 个维度逐项分析。

---

## 目录

1. [LLM / Provider 系统](#1-llm--provider-系统)
2. [CLI 命令体系](#2-cli-命令体系)
3. [扩展 / 插件 API 对齐](#3-扩展--插件-api-对齐)
4. [会话管理](#4-会话管理)
5. [UI / TUI 系统](#5-ui--tui-系统)
6. [工具系统](#6-工具系统)
7. [Agent Loop / 事件流](#7-agent-loop--事件流)
8. [技能 / 主题 / Prompt 模板](#8-技能--主题--prompt-模板)
9. [认证 / 鉴权](#9-认证--鉴权)
10. [安全 / 审计](#10-安全--审计)
11. [包管理 / 扩展分发](#11-包管理--扩展分发)
12. [多 Agent / 子代理 / RPC 模式](#12-多-agent--子代理--rpc-模式)
13. [差距汇总表](#13-差距汇总表)
14. [与 TASK_BOARD 映射](#14-与-task_board-映射)
15. [建议实施路线图](#15-建议实施路线图)

---

## 1. LLM / Provider 系统

**差距评级：大**

### pi-mono 能力

- **9 个内置 Provider**：`anthropic-messages`、`openai-completions`、`openai-responses`、`azure-openai-responses`、`openai-codex-responses`、`google-generative-ai`、`google-gemini-cli`、`google-vertex`、`bedrock-converse-stream`
- **四套调用 API**：`stream`、`complete`、`streamSimple`、`completeSimple`
- **AssistantMessageEvent 事件协议**：`text_start/delta/end`、`thinking_start/delta/end`、`toolcall_start/delta/end`、`done`、`error`
- **TypeBox 参数校验**：Tool parameters 使用 TypeBox schema，流式 partial JSON 防御
- **跨 Provider 会话交接**：Context 可序列化，中途切换模型继续对话
- **thinking / reasoning 预算**：`SimpleStreamOptions.reasoning`、`thinkingBudgets`
- **扩展注册自定义 Provider**：`registerProvider` API

### pi-rust-wasm 现状

- **仅 `OpenAiProvider`**：OpenAI 兼容 HTTP（通过 `base_url` 可对接其他兼容服务）
- **流式收齐返回**：dispatcher 中 `llm.createChatCompletionStream` 内部聚合 delta 为单条 content，非真正流式透出
- **Stub API**：`setModel`、`getModel`、`setThinkingLevel` 均为空返回

### 关键缺口

| 缺口 | 影响 | 建议优先级 |
|------|------|-----------|
| 多 Provider 支持（至少 Anthropic + Google） | 用户被锁定在 OpenAI 兼容生态 | P1 |
| 真正流式事件透传（`AssistantMessageEvent` 协议） | TUI 流式渲染、thinking 展示无法实现 | P1 |
| thinking / reasoning 支持 | 无法利用 Claude thinking、o1 reasoning | P2 |
| 跨 Provider 交接 | 无法中途切换模型 | P3 |
| `registerProvider` 扩展 API | 社区无法贡献自定义 Provider | P2 |

---

## 2. CLI 命令体系

**差距评级：中大**

### pi-mono 能力

- **包管理命令**：`pi install`、`pi remove`/`pi uninstall`、`pi update`、`pi list`
- **20+ 交互 slash 命令**：`/login`、`/logout`、`/model`、`/scoped-models`、`/settings`、`/resume`、`/new`、`/name`、`/session`、`/tree`、`/fork`、`/compact`、`/copy`、`/export`、`/share`、`/reload`、`/hotkeys`、`/changelog`、`/quit`、`/exit`；以及 `/skill:name`、`/templatename`
- **多输出模式**：`--print`（非交互）、`--mode json`（结构化）、`--mode rpc`（机器间通信）
- **工具选择**：`--tools read,bash,edit,write`、`--no-tools`
- **资源加载标志**：`-e/--extension`、`--skill`、`--prompt-template`、`--theme` 及对应 `--no-*`
- **会话恢复**：`-c/--continue`、`-r/--resume`、`--session`、`--fork`、`--no-session`

### pi-rust-wasm 现状

- **已实现**：`init`、`doctor`、`config`（get/set/edit/export/import）、`session`（list/new/switch/delete/archive/search）、`plugin`（list/load/unload/enable/disable/info）、`audit`（list/show/export）、`chat`（`--resume`）
- **占位 / 部分**：`session switch`（固定 key 提示）、`config import`（校验但未写入）
- **缺失**：包管理命令、slash 命令框架、print/json/rpc 模式、`--tools` 选择、资源加载标志

### 关键缺口

| 缺口 | 影响 | 建议优先级 |
|------|------|-----------|
| slash 命令框架（交互模式内 `/xxx`） | 用户无法在对话中切换模型/会话/设置 | P1 |
| `--tools` 工具选择 | 无法限制 Agent 可用工具 | P1 |
| `--print` 非交互模式 | 无法用于脚本/CI 管道 | P2 |
| 包管理（install/remove/update） | 无法远程安装扩展 | P2 |
| `--mode json/rpc` | 无法作为 API 后端或被其他工具集成 | P3 |

---

## 3. 扩展 / 插件 API 对齐

**差距评级：小（已大部分完成）**

### 已完成

TASK-05a~05e 完成 15/15 pi-mono 社区插件端到端验收。`globalThis.pi` 上已有：

| API 类别 | 已实现 |
|----------|--------|
| 事件 | `on`、`off`、`once`、`emit` |
| 执行 | `exec`、`readFile`、`writeFile`、`editFile` |
| 工具 | `registerTool`、`unregisterTool`、`getActiveTools`、`setActiveTools`、`getAllTools` |
| 命令/Flag/快捷键 | `registerCommand`、`registerShortcut`（Stub）、`registerFlag`/`getFlag`（Stub） |
| LLM | `createChatCompletion`、`complete`、`setModel`（Stub）、`getModel`（Stub）、`setThinkingLevel`（Stub） |
| 会话 | `session.getCurrent`、`session.getMessages`、`session.sendMessage`、`sendMessage`、`sendUserMessage`、`getSessionName`（Stub）、`setSessionName`（Stub）、`appendEntry`（Stub） |
| ctx 上下文 | `cwd`、`hasUI`、`model`、`isIdle`（Stub）、`abort`（Stub）、`sessionManager.*`、`modelRegistry.*`、`ui.*` |

### 仍缺

| 缺口 | 说明 | 建议优先级 |
|------|------|-----------|
| `registerProvider` | 扩展注册自定义 LLM Provider | P2（依赖多 Provider 架构） |
| `registerMessageRenderer` | 自定义消息渲染器 | P3（依赖 TUI） |
| 独立 `events` 句柄 | 部分 pi-mono 扩展使用 `pi.events.on(...)` 而非 `pi.on(...)` | P3 |
| `setActiveTools` 真实实现 | 当前宿主不更新状态 | P2 |
| Stub → 真实实现 | `registerFlag`/`getFlag`/`registerShortcut`/`getSessionName`/`setSessionName`/`setThinkingLevel` 等 | P2 |

---

## 4. 会话管理

**差距评级：中**

### pi-mono 能力

- **SessionManager**：持久化目录、当前 session 名、分支栈；`buildSessionContext()` 恢复 messages + model + thinkingLevel
- **分支 / Fork**：会话分支、`/fork` 命令、`/tree` 查看分支树
- **Compact**：自动/手动消息压缩（`auto_compaction_start/end` 事件、`/compact` 命令）
- **多会话切换**：`/session`、`/resume`、`-c`/`-r`
- **会话导出/分享**：`/export`、`/share`、`/copy`

### pi-rust-wasm 现状

- **SessionManager 基础 CRUD**：创建、列表、删除、归档、transcript 追加/只读、分支/叶子查询 API 均有
- **当前会话 key**：固定 `DEFAULT_SESSION_KEY`（MVP）
- **CLI 侧**：`session switch` 为占位提示

### 关键缺口

| 缺口 | 影响 | 建议优先级 |
|------|------|-----------|
| 多会话动态切换（真实 `session switch`） | 用户无法在多个对话间切换 | P1 |
| Compact（消息压缩） | 长对话超出上下文窗口后无法继续 | P1 |
| Fork（会话分支） | 无法从某个点分叉探索不同方向 | P2 |
| Tree 视图 | 无法可视化会话分支结构 | P3 |
| 导出 / 分享 | 无法将对话导出为文件或分享 | P3 |

---

## 5. UI / TUI 系统

**差距评级：大**

### pi-mono 能力

- **pi-tui**：完整终端 UI 框架
  - 差分渲染（仅更新变更区域）
  - CSI 2026 同步输出（防闪烁）
  - Overlay 浮层系统（模态、非模态、堆叠、焦点管理）
  - 组件化：`Container`、`SelectList`、`Text`、`DynamicBorder`、`BorderedLoader`、`CustomEditor`
  - 主题系统（配色方案切换）
  - Markdown 渲染、代码高亮
- **pi-web-ui**：Web 聊天组件、IndexedDB 存储、Artifacts

### pi-rust-wasm 现状

- **ctx.ui.***：大部分为 Stub
  - `notify`：仅 tracing 日志输出
  - `select`/`confirm`/`input`：返回默认值
  - `custom`：TUI shim 提供组件构造但无真实渲染
  - `setStatus`/`setWidget`/`setHeader`/`setFooter`：仅日志
- **chat 模式**：基本的终端输入/输出，流式文本打印
- **无 Overlay、无 Markdown 渲染、无代码高亮**

### 关键缺口

| 缺口 | 影响 | 建议优先级 |
|------|------|-----------|
| 流式 Markdown 渲染与代码高亮 | 输出可读性差 | P1 |
| Diff 预览确认（edit/write 前展示变更） | 用户无法审查 Agent 修改 | P1 |
| 组件化 TUI 渲染引擎 | 插件 `ctx.ui.custom` 无法真正工作 | P2 |
| Overlay 系统 | 模态交互（选择列表、设置面板等）无法实现 | P2 |
| Web UI | 无浏览器端界面 | P3 |

---

## 6. 工具系统

**差距评级：小**

### pi-mono 能力

- **默认工具**：`read`、`write`、`edit`、`bash`
- **只读工具**：`grep`、`find`、`ls`
- **可选**：`glob`、`rg`（Pod 测试中出现）
- **`--tools` 选择**：CLI 可指定启用哪些工具

### pi-rust-wasm 现状

- **4 原语完整**：`readFile`、`writeFile`、`editFile`、`executeBash`（`DefaultPrimitiveExecutor`）
- **ToolRegistry**：注册/注销/列表/调用完整
- **插件 `registerTool`**：完整（含 TypeBox schema 包装）

### 关键缺口

| 缺口 | 影响 | 建议优先级 |
|------|------|-----------|
| `grep`/`find`/`ls` 内置工具 | Agent 需通过 bash 间接实现，效率低 | P2 |
| `--tools` 工具选择 | 无法限制 Agent 可用工具集 | P1（归入 CLI） |

---

## 7. Agent Loop / 事件流

**差距评级：小**

### pi-mono 能力

- **AgentEvent 流**：`agent_start/end`、`turn_start/end`、`message_start/update/end`、`tool_execution_start/update/end`、`auto_compaction_start/end`、`auto_retry_start/end`
- **三层循环**：对话管理 → 容错重试 → 思考-行动（Steering / FollowUp / Abort）
- **Subscribe 模式**：上层通过 `session.subscribe()` 获取事件流

### pi-rust-wasm 现状

- **TASK-14 已完成**：三层 AgentLoop（Steering/FollowUp/Abort）、AgentEvent
- **`infra/events.rs`**：已枚举大部分 wire 事件常量（含 agent_*、turn_*、message_*、tool_execution_*、auto_compaction_*、auto_retry_*）
- **DefaultEventBus**：on/once/off/emit_sync 完整

### 关键缺口

| 缺口 | 影响 | 建议优先级 |
|------|------|-----------|
| `auto_compaction_*` 实际触发 | 依赖 compact 能力实现 | P2（与会话联动） |
| streaming 事件向 TUI 透传 | 流式渲染依赖事件链路 | P1（与 LLM/TUI 联动） |

---

## 8. 技能 / 主题 / Prompt 模板

**差距评级：大**

### pi-mono 能力

- **Skills 系统**：`loadSkills()` 从 `~/.pi/agent/skills/` 加载；`/skill:name` slash 命令调用；技能可扩展 Agent 行为
- **Themes**：终端配色方案；`--theme` 参数；`ctx.ui.theme.*` API
- **Prompt Templates**：`--prompt-template` 参数；可自定义系统提示模板

### pi-rust-wasm 现状

- **system_prompt.rs**：存在但仅为硬编码拼接，无模板/技能/主题加载
- **ctx.ui.theme**：Stub（`getAllThemes`/`getTheme`/`setTheme` 返回空）

### 关键缺口

| 缺口 | 影响 | 建议优先级 |
|------|------|-----------|
| Skills 加载与管理 | 无法通过技能扩展 Agent 行为 | P2 |
| Prompt Templates | 无法自定义系统提示 | P2 |
| Themes | 无终端配色自定义（依赖 TUI） | P3 |

---

## 9. 认证 / 鉴权

**差距评级：中**

### pi-mono 能力

- **AuthStorage**：`load()`、`resolveApiKey(provider)`、持久化鉴权信息
- **订阅型鉴权**：Claude Max、ChatGPT Plus/Pro、Copilot、Gemini CLI、Antigravity 等
- **`/login` `/logout`**：交互式鉴权流程
- **`models.json`**：`~/.pi/agent/models.json` 自定义模型配置
- **`ModelRegistry`**：`getAll`、`getAvailable`、`find`、`getApiKeyForProvider`

### pi-rust-wasm 现状

- **config 管理**：`InfraConfig` 中存储 `api_key`、`base_url` 等
- **AuthStorage shim**：`pi_coding_agent_shim.js` 中提供空实现
- **ModelRegistry stub**：`ctx.modelRegistry` 有方法签名但返回空

### 关键缺口

| 缺口 | 影响 | 建议优先级 |
|------|------|-----------|
| 多 Provider API key 管理 | 切换 Provider 需手动改配置 | P1（与多 Provider 联动） |
| `models.json` 自定义模型注册 | 无法使用自定义端点模型 | P2 |
| 订阅型鉴权 | 无法使用 Claude Max 等订阅 | P3 |
| `/login` `/logout` 流程 | 无交互式鉴权入口 | P2 |

---

## 10. 安全 / 审计

**差距评级：小**

### pi-mono 能力

- **设计哲学**：核心不提供权限弹窗；安全由扩展实现（路径保护、Git 检查点等）
- **Pi packages 具备完整系统权限**，用户需审源码

### pi-rust-wasm 现状

- **TASK-04 审计系统**：已完整落地（`infra/audit.rs`）；高危操作可追溯、可查询、可导出
- **ConfirmationStrategy**：AllowAll / DenyAll 等策略已有
- **CLI `audit` 命令**：list / show / export 完整

### 关键缺口

| 缺口 | 影响 | 建议优先级 |
|------|------|-----------|
| 插件安全扫描（TASK-09） | 加载前无静态风险检测 | P2（已在 TASK_BOARD） |
| 交互式权限确认 UI | 用户无法逐次审批高危操作 | P2（依赖 TUI） |

---

## 11. 包管理 / 扩展分发

**差距评级：大**

### pi-mono 能力

- **npm 生态**：`pi install <package>`、`pi remove`、`pi update`、`pi list`
- **`package.json` 的 `pi` 字段**：声明扩展入口
- **npm registry 分发**：社区可发布扩展到 npm
- **依赖解析**：扩展可声明 npm 依赖，加载时自动解析

### pi-rust-wasm 现状

- **仅本地路径加载**：`plugin load <path>` 从本地目录加载
- **无远程包管理**

### 关键缺口

| 缺口 | 影响 | 建议优先级 |
|------|------|-----------|
| `plugin install/remove/update` 远程命令 | 无法从注册表安装扩展 | P2 |
| 扩展注册表（npm 或自建） | 无扩展分发渠道 | P3 |
| npm 依赖解析 | 带外部依赖的扩展需手动处理 | P2 |

---

## 12. 多 Agent / 子代理 / RPC 模式

**差距评级：中大**

### pi-mono 能力

- **`--mode json`**：结构化 JSON 输出（机器可读）
- **`--mode rpc`**：RPC 协议模式（可被其他工具集成）
- **子代理**：核心不内建；推荐扩展模式（tmux、多进程、`registerTool` 派发）
- **SDK**：`createAgentSession` 可编程式创建 Agent 会话

### pi-rust-wasm 现状

- **Architecture.md 设计**：`multi-agent.md` 描述了 AgentRegistry + dispatch_agent
- **代码中尚未实现**
- **无 json/rpc 输出模式**

### 关键缺口

| 缺口 | 影响 | 建议优先级 |
|------|------|-----------|
| `--print` 非交互模式 | 无法用于脚本/CI | P2 |
| `--mode json` 结构化输出 | 无法被程序解析 | P2 |
| `--mode rpc` | 无法作为 API 后端 | P3 |
| 多 Agent 调度实现 | 无法并行多 Agent 任务 | P3 |

---

## 13. 差距汇总表

| # | 维度 | 差距评级 | P0 缺口 | P1 缺口 | P2 缺口 | P3 缺口 |
|---|------|---------|---------|---------|---------|---------|
| 1 | LLM / Provider | 大 | — | 多 Provider、流式事件 | thinking、registerProvider | 跨 Provider 交接 |
| 2 | CLI 命令 | 中大 | — | slash 命令、--tools | 包管理、--print | --mode json/rpc |
| 3 | 扩展 API | 小 | — | — | registerProvider、Stub→真实 | registerMessageRenderer |
| 4 | 会话管理 | 中 | — | 多会话切换、compact | fork | tree、导出 |
| 5 | UI / TUI | 大 | — | Markdown 渲染、diff 确认 | 组件化 TUI、Overlay | Web UI |
| 6 | 工具系统 | 小 | — | — | grep/find/ls 内置 | — |
| 7 | Agent Loop | 小 | — | streaming 透传 | auto_compaction | — |
| 8 | 技能/主题/模板 | 大 | — | — | Skills、Prompt Templates | Themes |
| 9 | 认证/鉴权 | 中 | — | 多 Provider key 管理 | models.json、/login | 订阅型鉴权 |
| 10 | 安全/审计 | 小 | — | — | 安全扫描、权限确认 UI | — |
| 11 | 包管理 | 大 | — | — | install/remove/update | 注册表 |
| 12 | 多 Agent/RPC | 中大 | — | — | --print、--mode json | --mode rpc、多 Agent |

**P1 缺口总计**：8 项（决定产品核心可用性）  
**P2 缺口总计**：17 项（决定产品竞争力）  
**P3 缺口总计**：10 项（锦上添花）

---

## 14. 与 TASK_BOARD 映射

| TASK_BOARD 任务 | 状态 | 对应差距维度 |
|----------------|------|-------------|
| TASK-06 核心模块单元测试全覆盖 | TODO | 跨维度（质量保障） |
| TASK-07 全平台兼容性测试 | TODO | 跨维度（质量保障） |
| TASK-08 CLI 交互体验优化 | TODO | §2 CLI、§5 TUI |
| TASK-09 插件安全扫描 | TODO | §10 安全 |
| TASK-10 项目文档编写 | TODO | 跨维度（文档） |
| TASK-11 示例插件开发 | TODO | §3 扩展 API |

### 尚未进入看板的差距（建议新增任务）

| 建议任务 | 对应差距 | 建议优先级 |
|----------|---------|-----------|
| 多 Provider LLM 支持 | §1 | P1 |
| 流式事件协议与 TUI 流式渲染 | §1 + §5 | P1 |
| Slash 命令框架 | §2 | P1 |
| 会话 compact（消息压缩） | §4 | P1 |
| 多会话动态切换 | §4 | P1 |
| Markdown 渲染与代码高亮 | §5 | P1 |
| Diff 预览确认 | §5 | P1 |
| `--tools` 工具选择 | §2 + §6 | P1 |
| Skills / Prompt Templates 加载 | §8 | P2 |
| 多 Provider API key 管理 | §9 | P1 |
| 包管理命令（install/remove/update） | §11 | P2 |
| `--print` 非交互模式 | §12 | P2 |
| grep/find/ls 内置工具 | §6 | P2 |
| 组件化 TUI 渲染引擎 | §5 | P2 |
| `--mode json/rpc` | §12 | P3 |

---

## 15. 建议实施路线图

### 短期（1-2 迭代）— 核心可用性

聚焦 P1 缺口，使 pi-rust-wasm 在核心交互体验上接近 pi-mono。

1. **多 Provider LLM + 流式事件**
   - 实现 `AnthropicProvider`（Claude）、`GoogleProvider`（Gemini），保留 OpenAI 兼容
   - 将 dispatcher 中 `createChatCompletionStream` 改为真正流式，按 `AssistantMessageEvent` 协议透传
   - 多 Provider API key 管理（扩展 `InfraConfig` 或引入 `AuthStorage`）

2. **CLI 增强**
   - Slash 命令框架（解析 `/xxx` → handler 分发）
   - `--tools` 工具选择（在 `ToolRegistry` 上实现 active set 过滤）
   - 真实 `session switch`（替换 `DEFAULT_SESSION_KEY` 固定逻辑）

3. **TUI 基础**
   - 流式 Markdown 渲染（可基于 `termimad` 或 `bat` 的 syntect）
   - Diff 预览确认（write/edit 前展示变更 diff，用户确认后执行）

4. **会话 compact**
   - 实现消息压缩策略（截断/摘要），触发 `auto_compaction_*` 事件
   - 在长对话超出 context window 时自动激活

### 中期（3-4 迭代）— 产品竞争力

聚焦 P2 缺口。

5. **工具扩展**：`grep`/`find`/`ls` 内置工具；`--print` 非交互模式
6. **Skills / Prompt Templates**：从 `~/.pi_wasm/skills/` 加载技能文件；`--prompt-template` 参数
7. **包管理基础**：`plugin install <url/path>`、`plugin remove`；简单的本地注册表
8. **组件化 TUI**：参考 `ratatui` 实现组件渲染；Overlay 浮层
9. **扩展 API 补全**：Stub → 真实实现（registerFlag/getFlag、setActiveTools 等）；`registerProvider`
10. **安全扫描**：TASK-09 落地

### 长期（5+ 迭代）— 生态完善

聚焦 P3 缺口。

11. **`--mode json/rpc`**、多 Agent 调度
12. **跨 Provider 交接**、thinking/reasoning 预算
13. **Web UI**（可基于 pi-web-ui 架构）
14. **npm 兼容扩展注册表**
15. **Themes、订阅型鉴权**
16. **会话 fork/tree/export/share**

---

## 附录：数据来源

| 来源 | 路径 |
|------|------|
| pi-mono 文档快照 | `pi-mono_docs/00~05-*.md`、`pi-coding-agent-README.md` |
| pi-rust-wasm 源码 | `src/`、`assets/js/`、`tests/` |
| 已有差距分析 | `docs/reports/extension_api_gap_analysis.md` |
| 兼容矩阵 | `docs/reports/extension_compat_matrix.md` |
| 任务看板 | `agents/TASK_BOARD.md` |
| 架构文档 | `openspec/specs/Architecture.md` |
| 兼容策略 | `openspec/specs/architecture/plugin-system/pi-mono-compat-strategy.md` |

---

**维护**：每次迭代完成后更新本报告中的差距评级与缺口状态。
