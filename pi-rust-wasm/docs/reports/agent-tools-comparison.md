# 五大 Agent 工具实现深度对比与评分

> **目的**：对 Tomcat 工作区内五个代表性 Agent 代码仓——`cc-fork-01`、`hermes-agent`、`openclaw`、`pi_agent_rust`、`pi-mono`——梳理「发给 LLM 的工具」清单、实现要点、横向打分与选型建议。  
> **范围**：以各仓库 **源码与自述文档** 为准；对照 Cursor 桌面 Agent 的「能力维度」（参见 [cursor-builtin-tools-reference.md](./cursor-builtin-tools-reference.md)），**非**逐字节对齐闭源 Cursor 客户端。  
> **时效**：调研快照 **2026-05**；分支演进后以各仓库最新提交为准。**完整工具枚举（五项目「标识符 + 作用」与源码引用）**见 §10；**落地场景与行业映射**见 §11。  
> **可视化**：Cursor Canvas 须置于 IDE 托管路径：`~/.cursor/projects/Users-yankeben-workspace-Tomcat/canvases/agent-tools-comparison.canvas.tsx`（同目录下 `agent-tools-comparison.md` 为文字归档）。Canvas 内为摘要图表；**论证与引用以本文为准**。  
> **版权与合规**：`cc-fork-01` 为公开快照归档，**不等同** Anthropic 官方发行物；引用英文 description 时标注出处文件。

---

## 0. Canvas 与本文关系

| 产物 | 路径 | 说明 |
|------|------|------|
| Canvas | `~/.cursor/projects/Users-yankeben-workspace-Tomcat/canvases/agent-tools-comparison.canvas.tsx` | 七维折线对比、19×5 能力矩阵摘要、按类推荐表 |
| 长报告 | `pi-rust-wasm/docs/reports/agent-tools-comparison.md`（本文） | 完整表格、源码行引用；**§10** 为按项目「工具名 + 作用」总表 |

---

## 1. 调研方法与对照基准

1. **静态阅读**：`grep`/`glob` 定位 `registry.register`、`buildTool`、`create*Tool`；阅读各项目 `README`/`AGENTS.md` 与工具注册入口。  
2. **能力维度**：在 Cursor 文档「十大类」基础上扩展工程常见项，形成 **19 维**（见 §3），便于五项目横向对齐。  
3. **评分锚点**：七维分数 **1–10** 为**相对锚**（同维度内最高分代表该维度最强实现），避免脱离代码的主观绝对分。  
4. **边界**：未在本机运行各 CLI；动态 MCP 工具名、运行时插件列表以「机制描述 + 附录静态枚举」为主。

---

## 2. 五项目快照

### 2.1 cc-fork-01（Claude Code TS 源码镜像）

| 项 | 内容 |
|----|------|
| **定位** | npm 发布物 source map 暴露的 **TypeScript 源码快照**；教育/供应链研究语境（见 [`cc-fork-01/README.md`](../../../cc-fork-01/README.md) 开篇）。 |
| **栈** | Bun、`src/main.tsx` CLI、Ink、Zod、`src/tools.ts` 聚合工具、`utils/api.ts` 将 Zod schema 转为 API tools（`toolToAPISchema`，描述常取自各工具 `prompt()`）。 |
| **体量** | `src/tools` 约 **5 万行 / 184 文件**量级（调研口径）；根目录无完整 `package.json`，需按 `docs/CODEBASE_GUIDE.md` 理解构建边界。 |
| **工具量级** | **30+** 具名内置工具 + **MCP 动态克隆**（`mcp__<server>__<tool>`，可选无前缀覆盖内置，见 `services/mcp/client.ts` 约 **1760–1774** 行）。 |

**代表入口**：[`cc-fork-01/src/tools.ts`](../../../cc-fork-01/src/tools.ts)（`getTools` / `getAllBaseTools`）；[`cc-fork-01/src/utils/api.ts`](../../../cc-fork-01/src/utils/api.ts)（**169–176** 行附近：`prompt()` → 下发 description）。

### 2.2 hermes-agent（Python 全栈 Agent + Gateway）

| 项 | 内容 |
|----|------|
| **定位** | 交互 CLI、**多消息平台 Gateway**（Telegram/Discord/Slack/…）、**tools/registry.py 自注册**、可选 RL/SWE 批跑（`batch_runner.py`、`environments/`、`tools/rl_training_tool.py`）。 |
| **栈** | Python；[`hermes-agent/model_tools.py`](../../../hermes-agent/model_tools.py)  orchestration；[`hermes-agent/toolsets.py`](../../../hermes-agent/toolsets.py) 中 `_HERMES_CORE_TOOLS`（约 **31** 行起）列出核心工具名字符串。 |
| **工具量级** | `tools/*.py` **69** 个模块级文件；`registry.register(name=...)` 静态清点 **70+** 具名工具（含 Feishu/Yuanbao/Discord/HomeAssistant 等集成）。 |
| **测试** | `AGENTS.md` 声称完整 pytest **量级极大**（文档版本间数字不一致，以仓库 `tests/` 为准）。 |

**依赖链**（摘自 [`hermes-agent/AGENTS.md`](../../../hermes-agent/AGENTS.md) **64–74** 行）：`tools/registry.py` ← `tools/*.py`（import 时 `register`）← `model_tools.py`。

### 2.3 openclaw（TS 单体客户端 + Gateway + 插件）

| 项 | 内容 |
|----|------|
| **定位** | 个人 AI 助手产品谱系（[`openclaw/VISION.md`](../../../openclaw/VISION.md) Warelay→OpenClaw）；**pnpm workspace**，根入口 [`openclaw/openclaw.mjs`](../../../openclaw/openclaw.mjs)，源码 [`openclaw/src/agents/pi-tools.ts`](../../../openclaw/src/agents/pi-tools.ts) 装配 **Pi 编码工具**并做 openclaw 包装。 |
| **栈** | `@mariozechner/pi-coding-agent` + [`src/agents/openclaw-tools.ts`](../../../openclaw/src/agents/openclaw-tools.ts)（`createOpenClawTools`）；**extensions/\*** 声明式 `contracts.tools` + `registerTool`；MCP **bundle 物化**（[`pi-bundle-mcp-materialize.ts`](../../../openclaw/src/agents/pi-bundle-mcp-materialize.ts)）。 |
| **工具量级** | **Core catalog** 数十 ID（[`tool-catalog.ts`](../../../openclaw/src/agents/tool-catalog.ts)）+ **每插件 0～N** + **通道动态 agentTools**；典型部署「数十～上百」可见工具名。 |

### 2.4 pi_agent_rust（Rust Pi CLI）

| 项 | 内容 |
|----|------|
| **定位** | **高性能 Rust** 版 Pi Agent CLI；README 写明 **8 built-in tools**；与 pi-mono **行为对齐**（多处注释 parity）。 |
| **栈** | [`pi_agent_rust/src/tools.rs`](../../../pi_agent_rust/src/tools.rs) **单文件集中**实现内置工具（万行量级）；[`ToolRegistry::new`](../../../pi_agent_rust/src/tools.rs) 约 **1282–1311** 行注册；[`src/cli.rs`](../../../pi_agent_rust/src/cli.rs) **375–379** 行默认启用：`read,bash,edit,write,grep,find,ls,hashline_edit`。 |
| **扩展** | [`extension_tools.rs`](../../../pi_agent_rust/src/extension_tools.rs) QuickJS/WASM **ExtensionToolDef** 动态并入会话。 |

### 2.5 pi-mono（官方 TS Pi Monorepo）

| 项 | 内容 |
|----|------|
| **定位** | `@mariozechner/pi-coding-agent` 等指标包的 **上游契约仓**；[`packages/coding-agent/README.md`](../../../pi-mono/packages/coding-agent/README.md) 明确 **核心不包含 MCP**、强调扩展 `registerTool`。 |
| **内置工具** | [`packages/coding-agent/src/core/tools/index.ts`](../../../pi-mono/packages/coding-agent/src/core/tools/index.ts) **`createAllToolDefinitions`**（约 **156–165** 行）：**7** 个——`read/bash/edit/write/grep/find/ls`；**默认仅启用 4**（[`sdk.ts`](../../../pi-mono/packages/coding-agent/src/core/sdk.ts) 约 **271–277** 行会话默认 active 列表）。 |
| **与 Rust** | 本仓历史 MVP 设计文档（原 `openspec/specs/archive/001-mvp/`，已移除）约定：**语义冲突以 pi-mono 为准**。 |

### 2.6 形态分层（摘要）

- **平台型**：cc-fork-01、hermes-agent、openclaw —— 工具链 + 权限/网关/插件形态完整。  
- **内核型**：pi-mono、pi_agent_rust —— **最小读写搜壳 + bash**，其余靠扩展或宿主策略。

---

## 3. 十九维工具能力矩阵（五项目对照）

**图例**：**full** = 一等公民 LLM 工具；**partial** = 有替代路径（bash/扩展/仅 slash）；**none** = 无直接对齐。

| # | 维度 | cc-fork-01 | hermes-agent | openclaw | pi_agent_rust | pi-mono |
|---|------|------------|--------------|----------|---------------|---------|
| 1 | **语义检索 / codebase index** | none（靠 Grep+模型） | none | none（可挂扩展） | none | none |
| 2 | **Glob / 文件名** | `Glob` — [`GlobTool.ts`](../../../cc-fork-01/src/tools/GlobTool/GlobTool.ts) 封装 glob，最多约 100 文件截断 | `search_files` 等与文件工具同组 — [`file_tools.py`](../../../hermes-agent/tools/file_tools.py) **1122–1124** | `find` — Pi 透传，[`pi-tools.ts`](../../../openclaw/src/agents/pi-tools.ts) 包装 | `find` — 外部 `fd`，[`tools.rs`](../../../pi_agent_rust/src/tools.rs) FindTool | `find` — [`find.ts`](../../../pi-mono/packages/coding-agent/src/core/tools/find.ts) spawn fd |
| 3 | **Grep / 内容搜索** | `Grep` — ripgrep，禁止模型用 Bash 跑 rg（prompt 约束） | `search_files` — 同 file 工具集 | `grep` — Pi | `grep` — `rg --json` | `grep` — `ensureTool("rg")` |
| 4 | **Web 搜索** | `WebSearch` — Anthropic server-side tool 封装 | `web_search` — [`web_tools.py`](../../../hermes-agent/tools/web_tools.py) **2132+** | `web_search` — [`openclaw-tools.ts`](../../../openclaw/src/agents/openclaw-tools.ts) | none | none |
| 5 | **Web 抓取** | `WebFetch` — URL→markdown，域名策略 | `web_extract` | `web_fetch` | none | none |
| 6 | **Read** | `Read` — 多模态/PDF/笔记本分支，token 估算 | `read_file` — schema 于 file_tools | `read` — 输出可按 context window 缩放 | `read` — 行/字节上限 | `read` — 截断+图片 |
| 7 | **Write** | `Write` — 团队密钥扫描、UNC 防护 | `write_file` | `write` | `write` — 原子写 | `write` |
| 8 | **Edit / patch** | `Edit` / `NotebookEdit` | `patch` | `edit` + 条件 `apply_patch` | `edit` + **`hashline_edit`** 锚点协议 | `edit` — 多段 `edits[]` |
| 9 | **Shell** | `Bash` — `maxResultSizeChars` 30k、SandboxManager、**bashPermissions** AST | `terminal` — 多 **environments**（local/docker/ssh/…） | `exec`/`process` — 惰性 bash-tools，Docker 规格 | `bash` — 专用线程泵 stdout/stderr | `bash` — `spawn` + 超时 |
|10 | **Browser** | 无独立浏览器工具（产品靠 MCP/Web） | `browser_navigate` 等 **10+** — [`browser_tool.py`](../../../hermes-agent/tools/browser_tool.py) **2909+** | **browser** 插件 + [`sandbox/browser.ts`](../../../openclaw/src/agents/sandbox/browser.ts) | none | none |
|11 | **MCP** | 动态 `MCPTool` + List/Read McpResource | [`mcp_tool.py`](../../../hermes-agent/tools/mcp_tool.py) 大号客户端 | bundle-mcp **安全前缀**物化 | 扩展路径 | **核心不包含**（README 声明） |
|12 | **Subagent / Task** | `Agent`（别名 `Task`）+ **Swarm** `SendMessage`/`Team*` | `delegate_task` | `sessions_spawn`（`subagent`|`acp`） | none | none |
|13 | **Plan / Todo** | `EnterPlanMode`/`ExitPlanMode` + Todo v1/v2 | `todo` + kanban 工具族 | `update_plan`（实验门控） | none | none |
|14 | **图像 / 音视频 / PDF** | 读侧多模态；**ImageGen** 另工具（feature） | `image_generate`/`text_to_speech`/`vision_analyze` | **image/video/music/pdf/tts** 多条工厂 | 读图附件 | 读图 |
|15 | **Ask 用户** | `AskUserQuestion` | `clarify` | 依赖通道/UI | none | none |
|16 | **Lint / LSP** | **`LSP`** 可选工具 | none 一等工具 | none | none | none |
|17 | **Memory** | `/memory` slash + `extractMemories` 服务层 | **`memory`** 工具 + `plugins/memory/*` | memory-\* 扩展 | 扩展 | 扩展 |
|18 | **Canvas / 可视化产物** | 无 | 无 | **`canvas`** | 无 | 无 |
|19 | **Fetch Rules（Cursor 概念）** | 无同名 | 无同名 | 无同名 | 无同名 | 无同名 |

---

## 4. 重点工具深挖（八类）

### 4.1 Shell / 终端

| 项目 | 机制摘要 |
|------|----------|
| **cc-fork-01** | [`BashTool.tsx`](../../../cc-fork-01/src/tools/BashTool/BashTool.tsx) `buildTool` 约 **420+** 行；输出截断 **30_000** 字符量级；[`bashPermissions.ts`](../../../cc-fork-01/src/tools/BashTool/bashPermissions.ts) 规则 + AST 级约束（示例 **1050+** 行段）；与 **SandboxManager** 协同。 |
| **hermes-agent** | [`terminal_tool.py`](../../../hermes-agent/tools/terminal_tool.py) **`terminal`** 注册于 **2334+** 行；[`tools/environments/`](../../../hermes-agent/tools/environments/) 多后端切换（docker/ssh/modal/daytona/singularity 等）。 |
| **openclaw** | [`bash-tools.descriptions.ts`](../../../openclaw/src/agents/bash-tools.descriptions.ts) 动态 description（审批/safeBins 提示）；[`pi-tools.ts`](../../../openclaw/src/agents/pi-tools.ts) 接 Docker **exec** 规格。 |
| **pi_agent_rust** | [`run_bash_command`](../../../pi_agent_rust/src/tools.rs) 约 **1914** 行起；注释明确 **独立 OS 线程**泵管道，避免占满 blocking 池（约 **1989–1997** 行）。 |
| **pi-mono** | [`bash.ts`](../../../pi-mono/packages/coding-agent/src/core/tools/bash.ts) **`spawn`** + 超时杀进程树；哲学上无内置弹窗审批（README 段）。 |

**对比结论**：**权限与规则深度** — cc-fork-01；**执行后端多样性** — hermes；**与容器沙箱绑定** — openclaw；**进程 IO 工程化** — pi_agent_rust；**最小可审计内核** — pi-mono。

### 4.2 编辑与补丁

- **cc-fork-01 `Edit`**：字符串替换 + 大文件上限（如 **1GiB** 注释）、写后 **secrets 检查**（[`FileEditTool.ts`](../../../cc-fork-01/src/tools/FileEditTool/FileEditTool.ts) **143–147** 行附近）。  
- **pi_agent_rust `hashline_edit`**：与 `read`/`grep` 的 **`hashline`** 输出闭合，**哈希锚点**降低误替换（[`tools.rs`](../../../pi_agent_rust/src/tools.rs) **5700+** 行）。  
- **pi-mono `edit`**：多段 **`edits[]`**，旧单段参数兼容（[`edit.ts`](../../../pi-mono/packages/coding-agent/src/core/tools/edit.ts) schema 段）。  
- **hermes `patch`**：与 `read_file`/`write_file` 同 [`file_tools.py`](../../../hermes-agent/tools/file_tools.py) 注册行 **1122–1124**。

### 4.3 浏览器

- **hermes**：`browser_navigate`/`browser_snapshot`/…/`browser_console` 等全链路（[`browser_tool.py`](../../../hermes-agent/tools/browser_tool.py) **2909+**），另有 **`browser_cdp`**、**`browser_dialog`**。  
- **openclaw**：插件 **`extensions/browser`** + CI 中 **Swabble**/macOS 与浏览器镜像测试（[`sandbox/browser.ts`](../../../openclaw/src/agents/sandbox/browser.ts) Docker/CDP 端口映射）。  
- **cc-fork-01**：无一等浏览器工具；Web 侧 **`WebFetch`/`WebSearch`**。

### 4.4 子代理 / 会话

- **cc-fork-01**：[`AgentTool.tsx`](../../../cc-fork-01/src/tools/AgentTool/AgentTool.tsx) **`Agent`**（遗留名 **`Task`**），参数含 `run_in_background`、`subagent_type`、`isolation`、`cwd`；**TeamCreate/Delete**、**SendMessage**（[`tools.ts`](../../../cc-fork-01/src/tools.ts) 条件注册）。  
- **openclaw**：[`sessions-spawn-tool.ts`](../../../openclaw/src/agents/tools/sessions-spawn-tool.ts) **`SESSIONS_SPAWN_RUNTIMES`** 含 **`"subagent"` / `"acp"`**（约 **33–36** 行）。  
- **hermes**：[`delegate_tool.py`](../../../hermes-agent/tools/delegate_tool.py) **`delegate_task`**（**2513+** 行注册）。

### 4.5 MCP

- **cc-fork-01**：`MCPTool` 基类 + `buildMcpToolName`；可选 **`CLAUDE_AGENT_SDK_MCP_NO_PREFIX`**（[`client.ts`](../../../cc-fork-01/src/services/mcp/client.ts) **1760–1773**）；**`ToolSearchTool`** 延迟加载大型工具表。  
- **openclaw**：[`pi-bundle-mcp-materialize.ts`](../../../openclaw/src/agents/pi-bundle-mcp-materialize.ts) **safeServerName + 分隔符 + toolName**；profile 默认放行 bundle-mcp（见 `tool-catalog` 文档段）。  
- **hermes**：[`mcp_tool.py`](../../../hermes-agent/tools/mcp_tool.py) ~1050 行量级客户端。  
- **pi-mono**：README **明确不做 MCP**（扩展自建）。

### 4.6 Plan / Todo

- **cc-fork-01**：**`EnterPlanMode`/`ExitPlanMode`**；Todo **v2** 下 **`TaskCreate`/`TaskGet`/`TaskList`/`TaskUpdate`** 替换部分 **`TodoWrite`**（[`tools.ts`](../../../cc-fork-01/src/tools.ts) 条件分支）。  
- **openclaw**：**`update_plan`**（[`openclaw-tools.registration.ts`](../../../openclaw/src/agents/openclaw-tools.registration.ts) 实验开关）。  
- **hermes**：**`todo`** + **kanban_\*** 工具族（[`kanban_tools.py`](../../../hermes-agent/tools/kanban_tools.py)）。  
- **pi / pi-mono**：无内置 Plan/Todo 工具。

### 4.7 多媒体与 PDF

- **openclaw**：**`image`/`image_generate`/`video_generate`/`music_generate`/`pdf`/`tts`** 等工厂装配（[`openclaw-tools.ts`](../../../openclaw/src/agents/openclaw-tools.ts) **352–406** 行段）。  
- **hermes**：**`image_generate`/`text_to_speech`/`vision_analyze`** 等（各 `tools/*.py` 注册）。  
- **cc-fork-01 / Pi 线**：读侧多模态；生成类依赖功能开关与产品配置。

### 4.8 Memory

- **cc-fork-01**：持久记忆偏 **服务层 + `/memory` slash**，非单一 LLM 工具枚举。  
- **hermes**：**`memory`** 工具 + **`plugins/memory`** 多 provider（honcho/mem0/supermemory 等目录）。  
- **openclaw**：**memory-lancedb / memory-wiki** 等扩展声明 `contracts.tools`。  
- **Pi 线**：靠 **扩展** 或 bash，无一等内置 memory 工具。

---

## 5. 七维评分（1–10，相对锚）

| 维度 | cc-fork-01 | hermes-agent | openclaw | pi_agent_rust | pi-mono |
|------|------------|--------------|----------|---------------|---------|
| **工具完整度** | 9 | **10** | 9 | 4 | 3 |
| **实现深度（单工具工程细节）** | **9** | 8 | 8 | 8 | 7 |
| **安全沙箱 / 策略** | 8 | 7 | **9** | 5 | 4 |
| **可扩展性（插件/MCP/通道）** | 9 | 9 | **10** | 6 | 7 |
| **运行时性能与资源** | 7 | 6 | 7 | **10** | 7 |
| **测试与回归资产** | 4 | **10** | 8 | 9 | 8 |
| **文档与可维护自述** | 8 | 8 | 8 | **9** | 8 |

**一句话依据**：  
- **hermes**：工具数量与集成面最广，pytest 资产厚。  
- **cc-fork-01**：Claude Code 级工具编排 + bash 权限 + defer/tool-search，商用产品镜像。  
- **openclaw**：Docker 沙箱、插件目录、ACP/spawn 完整产品栈。  
- **pi_agent_rust**：`#![forbid(unsafe_code)]`、bash 泵送与限额常量、`hashline` 闭环。  
- **pi-mono**：刻意极简核心 + 扩展哲学文档完备。

---

## 6. 按工具类的「首选 / 次选」推荐

| 维度 | 首选（抄作业） | 次选 | 采纳要点 |
|------|----------------|------|----------|
| **Glob / Grep** | cc-fork-01 | pi_agent_rust | 统一 ripgrep 封装 + 明确「勿用 bash 跑 rg」提示 |
| **Read（富格式）** | cc-fork-01 | pi-mono | PDF/笔记本/图片路由与 token 估算 |
| **Write 安全** | cc-fork-01 | pi_agent_rust | 密钥扫描 + 原子写（Rust 侧重原子落盘） |
| **精准编辑** | pi_agent_rust `hashline_edit` | cc-fork-01 `Edit` | 锚点协议 vs 大块 replace |
| **Shell 治理** | cc-fork-01 | openclaw | AST 权限 vs 容器 exec 策略 |
| **Browser** | openclaw | hermes | 插件化 CDP + 沙箱镜像 vs 多云浏览器后端 |
| **MCP** | cc-fork-01 | openclaw | 动态 schema 克隆 + searchHint |
| **Subagent** | cc-fork-01 | openclaw | swarm 编排 vs ACP runtime |
| **Gateway 多平台** | hermes-agent | openclaw | Python gateway 适配器矩阵 |
| **最小内核 CLI** | pi-mono | pi_agent_rust | 7/8 内置工具 + 扩展钩子 |

---

## 7. 选型建议（落地）

1. **自研「Cursor 风格」IDE Agent**：对齐 **能力维度表**（§3）；Shell 权限抄 **cc-fork-01**，沙箱执行抄 **openclaw**，路径限额抄 **pi_agent_rust**。  
2. **只要终端 Agent、可控体积**：**pi-mono** 或 **pi_agent_rust**，通过扩展补 MCP/审批。  
3. **消息机器人 + 工具矩阵**：**hermes-agent** Gateway + toolsets。  
4. **桌面 + 网关 + 插件商店形态**：**openclaw** 架构最接近独立产品。

---

## 8. 局限与免责

1. **Cursor / Claude Code 闭源部分**不可见；对比的是 **本仓库可达源码**。  
2. **MCP 动态工具名**随用户配置变化，附录仅为静态枚举。  
3. **cc-fork-01** 的法律与伦理语境见该仓 README；引用仅限技术架构研究。  
4. **评分**为辅助选型，不代表商业成熟度或合规认证。

---

## 9. 附录：各仓工具静态枚举（代表性）

### 9.1 cc-fork-01（节选）

`Read`/`Write`/`Edit`/`NotebookEdit`/`Glob`/`Grep`/`Bash`/`WebFetch`/`WebSearch`/`Agent`(`Task`)/`TaskOutput`/`TaskStop`(`KillShell`)/`TodoWrite`/`EnterPlanMode`/`ExitPlanMode`/`AskUserQuestion`/`Skill`/`MCPTool`/`ListMcpResourcesTool`/`ReadMcpResourceTool`/`ToolSearch`/`LSP`/`SendMessage`/`TeamCreate`/`TeamDelete`/`EnterWorktree`/`ExitWorktree`/`StructuredOutput`/`Brief`/`SendUserMessage`/`Cron*` + feature-gated 工具 —— 入口 [`src/tools.ts`](../../../cc-fork-01/src/tools.ts)。

### 9.2 hermes-agent（`registry.register` 抽样）

`read_file`/`write_file`/`patch`/`search_files`/`terminal`/`process`/`web_search`/`web_extract`/`memory`/`todo`/`clarify`/`delegate_task`/`execute_code`/`browser_navigate`…/`browser_console`/`browser_cdp`/`browser_dialog`/`image_generate`/`text_to_speech`/`vision_analyze`/`session_search`/`send_message`/`skills_list`/`skill_view`/`skill_manage`/`kanban_*`/`cronjob`/`mixture_of_agents`/`rl_*`（10+）/`feishu_*`/`yb_*`/`discord*`/`ha_*` —— 见各 [`tools/*.py`](../../../hermes-agent/tools/) 文件尾部注册块。

### 9.3 openclaw（核心 + 插件契约）

- **Core ID 表**：[`src/agents/tool-catalog.ts`](../../../openclaw/src/agents/tool-catalog.ts) `CORE_TOOL_DEFINITIONS`（约 **53–312** 行）枚举 **33** 个核心 `id`（详见 §10.3）。  
- **动态装配**：[`openclaw-tools.ts`](../../../openclaw/src/agents/openclaw-tools.ts) `createOpenClawTools` 按配置追加 **`pdf`**（[`createPdfTool`](../../../openclaw/src/agents/openclaw-tools.ts)）、媒体开关项等，**不一定**全部列入 `CORE_TOOL_DEFINITIONS` 静态数组。  
- **Pi 三件套**：`grep`/`find`/`ls` + 宿主封装后的读写编辑 `exec`/`process` —— [`pi-tools.ts`](../../../openclaw/src/agents/pi-tools.ts)。  
- **插件**：[`extensions/`](../../../openclaw/extensions/) 一级子目录本仓库统计 **125** 个；各包 `openclaw.plugin.json` 声明 `contracts.tools` / providers。

### 9.4 pi_agent_rust

内置固定 **8**：`read`/`bash`/`edit`/`write`/`grep`/`find`/`ls`/`hashline_edit` —— [`src/tools.rs`](../../../pi_agent_rust/src/tools.rs) + [`src/sdk.rs`](../../../pi_agent_rust/src/sdk.rs) `BUILTIN_TOOL_NAMES`。

### 9.5 pi-mono

内置 **7**：`read`/`bash`/`edit`/`write`/`grep`/`find`/`ls` —— [`packages/coding-agent/src/core/tools/index.ts`](../../../pi-mono/packages/coding-agent/src/core/tools/index.ts)。

---

## 10. 五项目工具清单（源码枚举 + 名称与作用）

本节按仓库给出 **发给模型的工具标识符** 与 **中文作用**（综合各仓 `description`/注释）。**Pi 系两内核**在前，**三平台重栈**（OpenClaw / Hermes / cc-fork）在后，与 §3 矩阵、§9 附录互证。**动态 MCP、插件运行时增删的名称**不在此穷尽。

### 10.1 pi-mono（`@mariozechner/pi-coding-agent`，7 个内置）

| 工具名 | 作用 |
|--------|------|
| `read` | 读取文件内容；支持文本与常见图片格式，大图可缩放；文本按行数/字节上限截断，可用 `offset`/`limit` 分段读（见 [`read.ts`](../../../pi-mono/packages/coding-agent/src/core/tools/read.ts)）。 |
| `write` | 写入或覆盖文件；不存在则创建，自动创建父目录（见 [`write.ts`](../../../pi-mono/packages/coding-agent/src/core/tools/write.ts)）。 |
| `edit` | 按精确字符串替换编辑文件，支持多段 `edits[]`（见 [`edit.ts`](../../../pi-mono/packages/coding-agent/src/core/tools/edit.ts)）。 |
| `bash` | 在当前工作目录执行 shell 命令；合并 stdout/stderr，过长输出截断并可将全文落到临时文件；可选超时（秒）（见 [`bash.ts`](../../../pi-mono/packages/coding-agent/src/core/tools/bash.ts)）。 |
| `grep` | 用本机 **ripgrep** 在仓库内按模式搜索文件内容，解析 JSON 流式结果（见 [`grep.ts`](../../../pi-mono/packages/coding-agent/src/core/tools/grep.ts)）。 |
| `find` | 用 **fd**（或可注入后端）按 glob 查找文件路径（见 [`find.ts`](../../../pi-mono/packages/coding-agent/src/core/tools/find.ts)）。 |
| `ls` | 列出目录条目（见 [`ls.ts`](../../../pi-mono/packages/coding-agent/src/core/tools/ls.ts)）。 |

默认会话往往只启用 `read`/`write`/`edit`/`bash`，`grep`/`find`/`ls` 需 CLI 显式打开。

### 10.2 pi_agent_rust（8 个内置）

| 工具名 | 作用 |
|--------|------|
| `read` | 读文件；支持图片附件；文本截断规则与 pi 对齐；可选 `hashline` 输出行哈希锚点供 `hashline_edit` 使用（[`tools.rs`](../../../pi_agent_rust/src/tools.rs) ReadTool）。 |
| `write` | 创建/覆盖文件，递归建父目录，原子落盘（WriteTool）。 |
| `edit` | 精确字符串替换编辑，`oldText` 唯一匹配（EditTool）。 |
| `bash` | 执行 bash；stdout/stderr 泵送与超时、输出上限（BashTool）。 |
| `grep` | 调用外部 `rg --json` 搜索内容，尊重 `.gitignore`（GrepTool）。 |
| `find` | 调用外部 `fd` 按 glob 找文件（FindTool）。 |
| `ls` | 异步列目录，条目数上限（LsTool）。 |
| `hashline_edit` | 基于 `read`/`grep` 返回的 hashline 锚点做多段结构化编辑，降低误替换（HashlineEditTool）。 |

扩展可通过 WASM/QuickJS 再注册任意工具名（运行时决定）。

### 10.3 openclaw（Core 目录 33 + Pi 三件套 + extensions）

[`tool-catalog.ts`](../../../openclaw/src/agents/tool-catalog.ts) **`CORE_TOOL_DEFINITIONS`** 共 **33** 个 `id`（**53–312** 行）。Pi 侧 **`grep`/`find`/`ls`** 经 [`pi-tools.ts`](../../../openclaw/src/agents/pi-tools.ts) 装配。[`openclaw-tools.ts`](../../../openclaw/src/agents/openclaw-tools.ts) `createOpenClawTools` 在通过 allowlist 且具备 `agentDir` 等条件时可追加 **`pdf`**（[`pdf-tool.ts`](../../../openclaw/src/agents/tools/pdf-tool.ts)），并与 `collectPresentOpenClawTools`（约 **573** 行）一并收集。仓库 [`extensions/`](../../../openclaw/extensions/) 一级子目录 **125** 个（本机 `find … -maxdepth 1 -type d | wc`）；**模型最终可见工具数** = core + 已启用扩展 + `bundle-mcp` + 通道 `agentTools`，常数十～上百。

#### （1）Core `id` 与作用（与 catalog 一致）

| 工具名 | 作用（源码 `description`） |
|--------|----------------------------|
| `read` | Read file contents |
| `write` | Create or overwrite files |
| `edit` | Make precise edits |
| `apply_patch` | Patch files |
| `exec` | 执行 shell（摘要文案来自 `EXEC_TOOL_DISPLAY_SUMMARY`） |
| `process` | 管理长时间运行的 exec 会话（`PROCESS_TOOL_DISPLAY_SUMMARY`） |
| `code_execution` | Run sandboxed remote analysis（常为插件提供的远程沙箱执行） |
| `web_search` | Search the web |
| `web_fetch` | Fetch web content |
| `x_search` | Search X posts |
| `memory_search` | Semantic search |
| `memory_get` | Read memory files |
| `sessions_list` | 列出会话（`SESSIONS_LIST_TOOL_DISPLAY_SUMMARY`） |
| `sessions_history` | 会话历史（`SESSIONS_HISTORY_TOOL_DISPLAY_SUMMARY`） |
| `sessions_send` | 向会话发消息（`SESSIONS_SEND_TOOL_DISPLAY_SUMMARY`） |
| `sessions_spawn` | 衍生子会话 / ACP（`SESSIONS_SPAWN_TOOL_DISPLAY_SUMMARY`） |
| `sessions_yield` | End turn to receive sub-agent results |
| `subagents` | Manage sub-agents |
| `session_status` | 当前会话状态（`SESSION_STATUS_TOOL_DISPLAY_SUMMARY`） |
| `browser` | Control web browser |
| `canvas` | Control canvases |
| `message` | Send messages |
| `heartbeat_respond` | Record heartbeat outcomes |
| `cron` | 定时任务（`CRON_TOOL_DISPLAY_SUMMARY`） |
| `gateway` | Gateway control |
| `nodes` | Nodes + devices |
| `agents_list` | List agents |
| `update_plan` | 更新计划（`UPDATE_PLAN_TOOL_DISPLAY_SUMMARY`） |
| `image` | Image understanding |
| `image_generate` | Image generation |
| `music_generate` | Music generation |
| `video_generate` | Video generation |
| `tts` | Text-to-speech conversion |

#### （2）Pi 编码侧附加（[`pi-tools.ts`](../../../openclaw/src/agents/pi-tools.ts)）

| 工具名 | 作用 |
|--------|------|
| `grep` | 仓库内容正则/文本搜索（Pi 实现，经 OpenClaw 策略包装）。 |
| `find` | 按 glob 查找文件路径。 |
| `ls` | 列出目录内容。 |

#### （3）常见动态装配（非全部在 core 静态数组内）

| 名称 | 作用 |
|------|------|
| `pdf` | 条件启用时：分析 PDF 页内容（[`pdf-tool.ts`](../../../openclaw/src/agents/tools/pdf-tool.ts)）。 |
| `bundle-mcp` | 将配置的 MCP 服务器工具物化为带安全前缀的可调用名（[`pi-bundle-mcp-materialize.ts`](../../../openclaw/src/agents/pi-bundle-mcp-materialize.ts)）。 |
| *各 `extensions/*`* | 插件 manifest 声明的 `contracts.tools`（如 `browser`、`memory_*`、`wiki_*`、`code_execution` 等），名称随插件而变。 |

### 10.4 hermes-agent（`_HERMES_CORE_TOOLS` 44 + 扩展静态注册 ≈ **70+**）

核心名单单一来源 [`toolsets.py`](../../../hermes-agent/toolsets.py) **`_HERMES_CORE_TOOLS`**（**31–67** 行）；扩展由各 `tools/*.py` 尾部 `registry.register` 清点。

#### （A）核心 44：名称与作用

| 工具名 | 作用 |
|--------|------|
| `web_search` | 互联网检索（供应商由配置决定）。 |
| `web_extract` | 抓取并抽取网页正文/结构化内容。 |
| `terminal` | 在可配置后端（本机 / Docker / SSH / 云沙箱等）执行终端命令。 |
| `process` | 查询与管理后台终端会话/进程。 |
| `read_file` | 读取文件（含长文件策略）。 |
| `write_file` | 写入或创建文件。 |
| `patch` | 对文件应用补丁式修改。 |
| `search_files` | 在仓库内搜索/过滤文件路径或内容模式。 |
| `vision_analyze` | 图像理解与问答。 |
| `image_generate` | 文生图（供应商由配置决定）。 |
| `skills_list` | 列出已安装 Skill 文档。 |
| `skill_view` | 查看单个 Skill 内容。 |
| `skill_manage` | 创建/编辑/启用禁用 Skill。 |
| `browser_navigate` | 浏览器导航到 URL。 |
| `browser_snapshot` | 获取页面可访问性/结构快照。 |
| `browser_click` | 模拟点击页面元素。 |
| `browser_type` | 在控件中输入文本。 |
| `browser_scroll` | 滚动页面或元素。 |
| `browser_back` | 浏览器后退。 |
| `browser_press` | 按键/快捷键。 |
| `browser_get_images` | 获取页面图片资源信息。 |
| `browser_vision` | 将页面截屏送视觉模型分析。 |
| `browser_console` | 读取浏览器控制台日志。 |
| `browser_cdp` | 底层 Chrome DevTools Protocol 调用。 |
| `browser_dialog` | 处理浏览器原生对话框（alert/confirm 等）。 |
| `text_to_speech` | 文本转语音输出。 |
| `todo` | 会话内待办列表读写。 |
| `memory` | 读写长期记忆存储（具体后端由插件配置）。 |
| `session_search` | 在历史会话中检索。 |
| `clarify` | 向用户发起澄清问题并等待答复。 |
| `execute_code` | 在沙箱或受限环境执行代码片段。 |
| `delegate_task` | 委派子代理任务并收回结果。 |
| `cronjob` | 创建与管理定时任务。 |
| `send_message` | 通过 Gateway 向各消息通道发送消息（需 Gateway）。 |
| `ha_list_entities` | 列出 Home Assistant 实体。 |
| `ha_get_state` | 读取 HA 实体状态。 |
| `ha_list_services` | 列出 HA 可用服务。 |
| `ha_call_service` | 调用 HA 服务（开灯、脚本等）。 |
| `kanban_show` | Kanban 模式：展示看板。 |
| `kanban_complete` | 标记任务完成。 |
| `kanban_block` | 标记阻塞。 |
| `kanban_heartbeat` | 看板心跳/存活。 |
| `kanban_comment` | 在看板上评论。 |
| `kanban_create` | 创建看板项。 |
| `kanban_link` | 关联看板项链接。 |

#### （B）扩展静态注册（示例）

| 分组 | 工具标识符 | 作用 | 源码入口 |
|------|------------|------|----------|
| MoA | `mixture_of_agents` | 多代理投票/融合推理。 | [`mixture_of_agents_tool.py`](../../../hermes-agent/tools/mixture_of_agents_tool.py) |
| RL 训练（10） | `rl_list_environments`, `rl_select_environment`, `rl_get_current_config`, `rl_edit_config`, `rl_start_training`, `rl_check_status`, `rl_stop_training`, `rl_get_results`, `rl_list_runs`, `rl_test_inference` | RL 训练环境列举、配置、启停、结果与推理试用。 | [`rl_training_tool.py`](../../../hermes-agent/tools/rl_training_tool.py)（约 **1376–1394** 行） |
| 飞书（5） | `feishu_doc_read`, `feishu_drive_list_comments`, `feishu_drive_list_comment_replies`, `feishu_drive_reply_comment`, `feishu_drive_add_comment` | 飞书文档读取与云盘评论协作。 | [`feishu_doc_tool.py`](../../../hermes-agent/tools/feishu_doc_tool.py)、[`feishu_drive_tool.py`](../../../hermes-agent/tools/feishu_drive_tool.py) |
| Discord（2） | `discord`, `discord_admin` | Discord 交互与管理操作。 | [`discord_tool.py`](../../../hermes-agent/tools/discord_tool.py) |
| 元宝（5） | `yb_query_group_info`, `yb_query_group_members`, `yb_send_dm`, `yb_search_sticker`, `yb_send_sticker` | 腾讯元宝/群组相关查询与消息。 | [`yuanbao_tools.py`](../../../hermes-agent/tools/yuanbao_tools.py) |

另：**[`mcp_tool.py`](../../../hermes-agent/tools/mcp_tool.py)** 暴露 MCP 服务器上的工具，**名称随 MCP `tools/list` 动态变化**。**[`plugins/`](../../../hermes-agent/plugins/)**（如 Spotify 等）可在运行时增加模型可见工具。

### 10.5 cc-fork-01（Claude Code TS 镜像，`getAllBaseTools`）

[`src/tools.ts`](../../../cc-fork-01/src/tools.ts) **`getAllBaseTools`**（**193–250** 行）。对外字符串名由各 Tool 的 `buildTool({ name, aliases })` 与各目录下 `*_TOOL_NAME` 常量决定。

**（一）常驻主干**（不经 `feature` / 环境门控即可出现的条目；不含 `Glob`/`Grep`、`ToolSearch` 及下文「条件插入」中的类）：

| TS 类名 | 模型可见 `name`（节选） |
|---------|-------------------------|
| `AgentTool` | `Agent`（遗留别名 `Task`） |
| `TaskOutputTool` | `TaskOutput` |
| `BashTool` | `Bash` |
| `ExitPlanModeV2Tool` | `ExitPlanMode` |
| `FileReadTool` | `Read` |
| `FileEditTool` | `Edit` |
| `FileWriteTool` | `Write` |
| `NotebookEditTool` | `NotebookEdit` |
| `WebFetchTool` | `WebFetch` |
| `TodoWriteTool` | `TodoWrite` |
| `WebSearchTool` | `WebSearch` |
| `TaskStopTool` | `TaskStop`（别名 `KillShell`） |
| `AskUserQuestionTool` | `AskUserQuestion` |
| `SkillTool` | `Skill` |
| `EnterPlanModeTool` | `EnterPlanMode` |
| `BriefTool` | `SendUserMessage`（遗留别名 `Brief`，见 [`BriefTool/prompt.ts`](../../../cc-fork-01/src/tools/BriefTool/prompt.ts)） |
| `SendMessageTool` | `SendMessage`（始终 `getSendMessageTool()` 插入，与 Swarm 开关无关） |
| `ListMcpResourcesTool` | `ListMcpResourcesTool` |
| `ReadMcpResourceTool` | `ReadMcpResourceTool` |

**（二）条件插入**（与 **`getAllBaseTools`** 返回数组中的 **展开顺序** 一致，行号指 [`tools.ts`](../../../cc-fork-01/src/tools.ts) **193–250** 行）：

| 条件（`tools.ts`） | 插入的类 |
|--------------------|----------|
| `!hasEmbeddedSearchTools()`（约 **201** 行） | `GlobTool`, `GrepTool` |
| `USER_TYPE === 'ant'`（约 **214–215** 行） | `ConfigTool`, `TungstenTool` |
| `SuggestBackgroundPRTool` 非空 | `SuggestBackgroundPRTool` |
| `WebBrowserTool` 非空 | `WebBrowserTool` |
| `isTodoV2Enabled()` | `TaskCreateTool`, `TaskGetTool`, `TaskUpdateTool`, `TaskListTool` |
| `OverflowTestTool` 非空 | `OverflowTestTool` |
| `CtxInspectTool` 非空 | `CtxInspectTool` |
| `TerminalCaptureTool` 非空 | `TerminalCaptureTool` |
| `isEnvTruthy(process.env.ENABLE_LSP_TOOL)` | `LSPTool` |
| `isWorktreeModeEnabled()` | `EnterWorktreeTool`, `ExitWorktreeTool` |
| `ListPeersTool` 非空 | `ListPeersTool` |
| `isAgentSwarmsEnabled()` | `TeamCreateTool`, `TeamDeleteTool` |
| `VerifyPlanExecutionTool` 非空 | `VerifyPlanExecutionTool` |
| `USER_TYPE === 'ant' && REPLTool` | `REPLTool` |
| `WorkflowTool` 非空 | `WorkflowTool` |
| `SleepTool` 非空 | `SleepTool` |
| `feature('AGENT_TRIGGERS')` → `cronTools` | `CronCreateTool`, `CronDeleteTool`, `CronListTool` |
| `RemoteTriggerTool` 非空 | `RemoteTriggerTool` |
| `MonitorTool` 非空 | `MonitorTool` |
| `SendUserFileTool` 非空 | `SendUserFileTool` |
| `PushNotificationTool` 非空 | `PushNotificationTool` |
| `SubscribePRTool` 非空 | `SubscribePRTool` |
| `getPowerShellTool()` 非空 | `PowerShellTool` |
| `SnipTool` 非空 | `SnipTool` |
| `NODE_ENV === 'test'` | `TestingPermissionTool` |
| `isToolSearchEnabledOptimistic()` | `ToolSearchTool` |

顶部的 `feature(...)` / `process.env` 条件见该文件 **16–135** 行各 `const` 定义；**`SendMessageTool`** 在 **226** 行无条件插入（已列入上表「常驻主干」）。

**MCP**：运行时克隆为 `mcp__<server>__<tool>`（可选无前缀），见 [`services/mcp/client.ts`](../../../cc-fork-01/src/services/mcp/client.ts)。

**（三）模型可见工具：名称与作用**

下列为默认装配下常见对外名称（glob/grep 在无嵌入式搜索时可省略）。

| 工具名 | 作用 |
|--------|------|
| `Read` | 读本地文件；支持图片/PDF/笔记本等分支；可分段读控制 token（[`prompt.ts`](../../../cc-fork-01/src/tools/FileReadTool/prompt.ts) `DESCRIPTION`）。 |
| `Write` | 写入或创建文件；含写入前密钥扫描等安全检查。 |
| `Edit` | 基于唯一匹配的字符串替换编辑文件。 |
| `NotebookEdit` | 编辑 Jupyter `.ipynb` 单元。 |
| `Glob` | 按 glob 模式列举匹配文件路径（无嵌入式搜索时注册）。 |
| `Grep` | 调用 ripgrep 做内容搜索（无嵌入式搜索时注册）。 |
| `Bash` | 执行 shell；流式输出、权限规则、`bashPermissions` 约束。 |
| `WebFetch` | 拉取 URL 并转为 Markdown 等可读形式。 |
| `WebSearch` | 调用 Anthropic Web Search 能力检索互联网。 |
| `Agent`（别名 `Task`） | 启动子代理 / 后台任务 / swarm 协作（参数含 `subagent_type`、`run_in_background` 等）。 |
| `TaskOutput` | 读取后台任务聚合输出（别名含 BashOutput）。 |
| `TaskStop`（别名 `KillShell`） | 按 ID 终止后台任务。 |
| `TodoWrite` | 写入会话待办列表（Todo v2 时可能被替代）。 |
| `TaskCreate` / `TaskGet` / `TaskList` / `TaskUpdate` | Todo v2 任务模型 CRUD（启用时）。 |
| `EnterPlanMode` / `ExitPlanMode` | 进入/退出计划模式，约束可用工具集。 |
| `AskUserQuestion` | 对用户展示选择题/问答题并等待答复。 |
| `Skill` | 执行预定义 Skill 工作流（可 fork 子代理）。 |
| `EnterWorktree` / `ExitWorktree` | 进入/退出 git worktree 隔离环境（启用时）。 |
| `SendMessage` | 向 teammate / 路由目标发送消息（**常驻**：`getSendMessageTool()` 无条件装配；不限于 Swarm）。 |
| `TeamCreate` / `TeamDelete` | 创建/删除 agent 团队（Swarm 启用时）。 |
| `ListMcpResourcesTool` | 列出 MCP 资源。 |
| `ReadMcpResourceTool` | 读取 MCP 资源内容。 |
| `ToolSearch` | 在延迟加载的大型工具表中检索可用工具。 |
| `LSP`（可选） | 语言服务器：跳转定义、引用等（`ENABLE_LSP_TOOL`）。 |
| `SendUserMessage`（遗留别名 `Brief`） | 向终端用户发送可读回复（主名见 [`BriefTool/prompt.ts`](../../../cc-fork-01/src/tools/BriefTool/prompt.ts)）。 |
| `MCPTool`（动态名） | 调用 MCP 服务器工具，默认名 `mcp__<server>__<tool>`。 |

**Feature / 环境门控（启用时才出现）**：`CronCreate`/`CronDelete`/`CronList`、`RemoteTrigger`、`Sleep`、`Monitor`、`WebBrowser`、`Workflow`、`PowerShell`、`VerifyPlanExecution`、`TestingPermissionTool`（测试）、`Config`、`REPL`、`PushNotification`、`SubscribePR`、`SendUserFile`、`Snip`、`ListPeers`、`TerminalCapture`、`CtxInspect`、`OverflowTest`、`SuggestBackgroundPR`、`Tungsten` 等 —— 见 [`tools.ts`](../../../cc-fork-01/src/tools.ts) **193–250** 行条件数组。

---

## 11. 排列组合与落地场景（用户 / 行业）

### 11.1 组合维度（如何把工具当作「能力包」卖）

| 维度 | 典型工具组合 | 依赖栈 |
|------|----------------|--------|
| **纯编码** | 文件四件套 + grep/find + bash | Pi / CC / Hermes `file` + `terminal` |
| **联网研究** | web_search + web_extract（+ browser） | 三家皆有 Web；Hermes/OpenClaw 浏览器更深 |
| **委托与并行** | Agent / delegate_task / sessions_spawn(acp) | CC `AgentTool`；Hermes `delegate_task`；OpenClaw `sessions_spawn` |
| **计划与记忆** | Plan 模式 + todo/memory | CC Enter/ExitPlan；Hermes todo/memory；OpenClaw `update_plan` + memory_* |
| **消息触达** | send_message + gateway | **Hermes** Gateway；**OpenClaw** `message` + 多通道 |
| **垂直 SaaS** | feishu_* / yb_* / discord / ha_* | **Hermes** 已内置注册 |
| **训练与评测** | rl_* + execute_code | **Hermes** RL 十工具 |
| **多媒体产物** | image/video/music + tts + canvas | **OpenClaw** core + UI；Hermes 图像/TTS/视觉 |
| **扩展无限** | MCP + plugins | CC / Hermes / OpenClaw 均支持；Pi-mono 核心不含 MCP |

### 11.2 场景 × 推荐底座（简表）

| 场景 | 典型用户 | 推荐 | 一句话理由 |
|------|-----------|------|------------|
| 终端里的「Claude Code」体验研究 | 架构师 / 安全研究 | **cc-fork-01** | 与商用 CC 工具谱系最近，含 Plan/Swarm/MCP/LSP |
| 个人 AI + 手机/桌面 + 插件市场 | 极客 / 小团队 | **openclaw** | 125 扩展目录 + Gateway + `canvas`，产品化完整 |
| Telegram/Slack 里远程使唤 Agent | 运营 / DevOps | **hermes-agent** | `_HERMES_CORE_TOOLS` 一次改全平台 + `send_message` |
| 飞书文档 / 评论闭环 | 国内协同 | **hermes-agent** | `feishu_*` 一等工具 |
| 智能家居联动话术 | 家庭 / 集成 | **hermes-agent** | `ha_*` 四工具 |
| RL / SWE 代理训练 | ML / 评测 | **hermes-agent** | `rl_*` 十件套 + `environments/` |
| 合规沙箱跑命令 + 远程浏览器 | 企业安全 | **openclaw** | Docker sandbox、`browser` 插件与 exec 策略同栈 |
| 仅需最小攻击面 CLI | 嵌入式 / WASM 宿主 | **pi-mono / pi_agent_rust** | 7～8 内置，扩展后置 |

### 11.3 行业映射（非互斥）

| 行业 | 更常对齐的栈 | 说明 |
|------|----------------|------|
| **软件 / 互联网研发** | CC 镜像、OpenClaw、Hermes | 编码 + CI 脚本 +（可选）浏览器验收 |
| **金融科技（强合规）** | Openclaw 沙箱 + 最小 Pi 内核 | 缩小 bash 暴露面；审计 MCP |
| **客服 / 运营** | Hermes Gateway、Openclaw messaging | 通道多、`clarify` + `send_message` |
| **制造业 / IoT** | Hermes `ha_*` + `terminal`（慎用） | 设备状态与服务调用 |
| **教育 / 科研** | Hermes RL + MoA；Openclaw Canvas | 实验编排与可视化交付 |
| **创意 / 媒体** | Openclaw 媒体全家桶；Hermes `image_generate`/`vision_analyze` | 生成与理解并重 |

---

**文档维护**：更新时请同步修订 Canvas 内嵌数据（`~/.cursor/projects/.../canvases/agent-tools-comparison.canvas.tsx`）与本文 §5、§10–§11；**extensions 数量**以仓库 `find extensions -maxdepth 1` 为准。
