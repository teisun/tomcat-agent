# Cursor 内置工具与能力参考（可抄作业版）

> **目的**：整理 Cursor **官方文档**中关于 Agent 工具、搜索、浏览器、子代理、Canvas 与工具调用原理的说明，方便在自研 Agent / 权限门 / 工具协议设计时对照。  
> **范围**：面向 **Cursor Desktop Agent**（侧栏 `Cmd+I`）；**不包含** MCP 服务器的工具清单（MCP 由各服务器自行定义）。  
> **时效**：正文以 Cursor 文档页面为准（摘录时间见文末）；产品迭代后请以 [cursor.com/docs](https://cursor.com/docs/agent/overview.md) 为准核对。  
> **版权声明**：下文「文档原文」类英文段落摘自 Cursor 官方文档；内部实现名称为工程推断与公开线索，见 §9。

---

## 1. Agent 的三块拼图

文档指出，Agent 由三部分组成：

1. **Instructions**：系统提示与 [Rules](https://cursor.com/docs/rules.md)。
2. **Tools**：编辑文件、检索代码库、执行终端命令等。
3. **Model**：你为任务选择的模型。

Cursor 声称会对不同前沿模型分别调整指令与工具配置（见 [Agent 概览](https://cursor.com/docs/agent/overview.md)）。

---

## 2. 工具调用（Tool Calling）机制

来源：[Tool calling fundamentals](https://cursor.com/learn/tool-calling.md)。

### 2.1 流程（文档叙述）

1. 模型判断需要额外能力；
2. 以 JSON 等形式指定 **工具名 + 参数**；
3. 应用执行工具并将结果写回上下文；
4. 模型继续对话。

### 2.2 单个工具的组成（文档）

每个工具通常包含：

1. **名称**（文档示例：`read_file`、`search_web`）  
2. **描述**：告知模型何时、如何使用  
3. **参数**：工具输入

文档中的 **示例** JSON（教学用，**不一定**等于当前 Cursor 桌面端注册的全部真实工具名）：

```json
{
  "name": "read_file",
  "description": "Read the contents of a file from the codebase",
  "parameters": {
    "filepath": "The path to the file to read"
  }
}
```

### 2.3 成本与上下文

- 工具 **定义** 会占用输入上下文（文档称通常每个工具数百 token 量级）。  
- **工具结果** 进入后续上下文，用量随返回内容变化。  
- 工具调用多时，上下文消耗更快；文档说明在 Cursor 内会看到更多 **cached input** 类用量（因需把上下文再次发给模型）。

### 2.4 「内置工具」以外：MCP

文档将 [MCP](https://cursor.com/marketplace) 描述为跨应用接入工具的通用方式；Cursor Agent 可在内置能力之上再挂载 MCP（与本报告「仅内置」范围不同）。

---

## 3. Agent 概览：官方列出的十大类工具（文档小节名称 + 原文描述）

来源：[Cursor Agent › Tools](https://cursor.com/docs/agent/overview.md)。

文档说明：**单次任务中工具调用次数没有上限**。下列为文档中 **小节标题 + 英文描述原文**（便于与你方工具 schema 对齐措辞）。

| # | 文档中的名称 | 官方描述（English, verbatim from docs） |
|---|----------------|------------------------------------------|
| 1 | **Semantic search** | Perform semantic searches within your indexed codebase. Finds code by meaning, not just exact matches. |
| 2 | **Search files and folders** | Search for files by name, read directory structures, and find exact keywords or patterns within files. |
| 3 | **Web** | Generate search queries and perform web searches. |
| 4 | **Fetch Rules** | Retrieve specific rules based on type and description. |
| 5 | **Read files** | Intelligently read the content of a file. Also supports image files (.png, .jpg, .gif, .webp, .svg) and includes them in the conversation context for analysis by vision-capable models. |
| 6 | **Edit files** | Suggest edits to files and apply them automatically. |
| 7 | **Run shell commands** | Execute terminal commands and monitor output. By default, Cursor uses the first terminal profile available. |
| 8 | **Browser** | Control a browser to take screenshots, test applications, and verify visual changes. Agent can navigate pages, interact with elements, and capture the current state for analysis. See the Browser documentation for details. |
| 9 | **Image generation** | Generate images from text descriptions or reference images. Useful for creating UI mockups, product assets, and visualizing architecture diagrams. Images are saved to your project's `assets/` folder by default and shown inline in chat. |
| 10 | **Ask questions** | Ask clarifying questions during a task. While waiting for your response, the agent continues reading files, making edits, or running commands. Your answer is incorporated as soon as it arrives. |

### 3.1 中文释义（便于抄作业到内部规格）

| 名称 | 简要释义 |
|------|-----------|
| Semantic search | 在 **已索引** 的代码库上做语义检索，按含义而非仅字面匹配找代码。 |
| Search files and folders | 按文件名搜索、浏览目录结构、在文件内做关键词/模式匹配（与「即时 grep」策略相关，见 §4）。 |
| Web | 生成检索词并执行 **互联网搜索**。 |
| Fetch Rules | 按类型与描述拉取 **Rules**（与 `.cursor/rules` 等规则体系配合）。 |
| Read files | 读取文件内容；支持多种图片格式，可被具备视觉能力的模型当作对话上下文分析。 |
| Edit files | 提议修改并 **自动应用** 到文件。 |
| Run shell commands | 执行终端命令并查看输出；默认使用第一个终端 Profile（可在命令面板设置 **Terminal: Select Default Profile**）。 |
| Browser | 控制浏览器做截图、测试、视觉验证；详见 §5。 |
| Image generation | 文生图或参考图；默认保存到项目 `assets/`，并在聊天内联展示。 |
| Ask questions | 任务中发起澄清提问；等待回复期间 Agent 仍可继续读文件、编辑、跑命令，回复到达后并入上下文。 |

---

## 4. 语义检索与「代理式搜索」（Semantic & agentic search）

来源：[Semantic & agentic search](https://cursor.com/docs/agent/tools/search.md)。

### 4.1 Instant Grep（即时 Grep）

- 文档：最快方式是 **精确匹配**（符号名、变量、错误串、正则）。引用特定符号时 Agent 会自动使用 grep。  
- Cursor 内置 **Instant Grep**，声称在大仓库上相对 `ripgrep` 有性能优势；**自动运行，无需配置**。  
- 支持完整正则与词边界，示例模式：`import.*PaymentService`、`PaymentFailedError`。

### 4.2 Semantic search（语义搜索）

- 适用于「不知道确切名字」的场景；依赖 Cursor 将代码库 **索引为向量**（自定义 embedding 模型）。  
- 文档引用研究：语义搜索与 grep 结合回答代码库问题的准确率更高；大仓库（1000+ 文件）增益更明显。

### 4.3 索引机制（文档要点）

- 代码被切成有意义块（函数、类等），嵌入向量库存储。  
- 打开工作区后开始索引；**约 80% 完成度** 时语义搜索可用。  
- 默认约 **每 5 分钟** 同步变更（仅处理改动文件）。  
- 遵守 [.gitignore / .cursorignore](https://cursor.com/docs/reference/ignore-file.md)；可在 **Settings › Indexing** 查看状态或触发重建。

### 4.4 Agent 如何组合工具（文档表格）

| 提示风格 | 使用的工具倾向 | 示例 |
|----------|----------------|------|
| 具体符号或字符串 | Grep | 「找出所有 import `PaymentService` 的文件」 |
| 概念或行为 | 语义搜索再 grep 补细节 | 「我们如何处理支付失败？」 |
| 复杂探索 | 多次搜索、读文件、跟踪引用 | 「从结账到确认邮件的数据流」 |

文档强调：**用户不必手动选工具**，描述目标即可；复杂任务会链式调用。

### 4.5 Explore subagent（探索子代理）

- Agent 可生成 **Explore** 子代理：独立上下文、更快模型、**并行多次搜索**，只把摘要返回主对话，避免撑爆主上下文。  
- 可直接要求：「用子代理找出所有校验用户输入的位置」。  
- 详见 [Subagents](https://cursor.com/docs/subagents.md)（§7）。

---

## 5. Browser（浏览器）能力摘要

来源：[Browser](https://cursor.com/docs/agent/tools/browser.md)。

### 5.1 文档声明的能力块（Browser capabilities）

| 能力 | 文档描述要点 |
|------|----------------|
| **Navigate** | 访问 URL、跟随链接、前进/后退、刷新。 |
| **Click** | 点击、双击、右键、悬停等。 |
| **Type** | 在输入框、表单中输入文本。 |
| **Scroll** | 滚动页面、定位元素、浏览长文档。 |
| **Screenshot** | 截图以理解布局、验证视觉结果。 |
| **Console Output** | 读取控制台日志与错误。 |
| **Network Traffic** | 监控 HTTP 请求与响应（文档注明部分布局下能力范围可能变化）。 |

### 5.2 与其它工具的配合（文档）

- 浏览器日志写入文件，Agent 可用 **grep / 选择性读取**，避免每次动作后全文摘要浪费 token。  
- 截图与 **读文件工具** 集成，模型以 **图像** 形式「看见」页面而非仅靠文字描述。  
- Agent 会收到日志行数、预览片段等额外提示；并被提示识别已运行的开发服务器端口，避免重复启动或猜端口。

### 5.3 企业管控（摘录）

- 企业客户可通过 MCP 允许/拒绝列表等管控浏览器能力。  
- **Origin allowlist**：可限制 Agent 自动导航的站点；文档明确提到 Agent 仅能使用 **`browser_navigate`** 访问列表内来源（具体策略以组织配置为准）。

### 5.4 安全与审批（文档）

- 浏览器在安全 WebView 中运行，通过扩展侧 MCP 控制；文档提及第三方安全审计。  
- 默认需用户审批浏览器动作；可在 Agent Settings 配置 **Manual approval / Allow-listed actions / Auto-run**（文档警告 Auto-run 风险）。  
- 可与 [security guardrails](https://cursor.com/docs/agent/security.md) 的允许/阻止列表联动。

---

## 6. Canvases（画布）

来源：[Canvases](https://cursor.com/docs/agent/tools/canvas.md)。

**文档原文要点**：Canvases 让 Cursor 创建 **交互式产物**，在聊天旁渲染；相比长 Markdown 表或代码块，提供独立视图（分区、统计、表格），可重新打开、编辑、迭代。

**流程概述**：

1. Cursor 判断任务适合可视化，或你明确要求；  
2. 构建 Canvas 并在回复中插入引用；  
3. 你可查看渲染结果、切到源码微调，或让 Cursor 修改；  
4. Canvas 保存在工作区的 canvas 列表中便于复访。

可与 [Skills](https://cursor.com/docs/skills.md) 打包重复性 Canvas 工作流。

---

## 7. Subagents（子代理）与 Task 工具

来源：[Subagents](https://cursor.com/docs/subagents.md)。

### 7.1 内置三个子代理（文档表格）

| Subagent | 用途 | 为何独立成子代理 |
|----------|------|------------------|
| **Explore** | 搜索与分析代码库 | 探索产生大量中间结果，需更快模型并行搜索 |
| **Bash** | 执行一系列 shell 命令 | 命令输出冗长，隔离后父代理专注决策 |
| **Browser** | 通过 MCP 控制浏览器 | DOM 与截图噪声大，子代理过滤后返回摘要 |

文档说明：**无需手动配置**，Agent 在合适时自动使用。

### 7.2 Task 工具与并行

- 文档原文：*Agent sends multiple **Task tool** calls in a single message*，子代理可 **并行** 执行。  
- 子代理 **独立上下文**，父代理需在提示中自带必要背景（子代理看不到完整历史）。  
- 运行模式：**Foreground**（阻塞至完成）与 **Background**（立即返回）。

### 7.3 自定义子代理

- 路径示例：项目 `.cursor/agents/`、用户 `~/.cursor/agents/`（及文档列出的 `.claude/`、`.codex/` 兼容路径）。  
- Markdown + YAML frontmatter：`name`、`description`、`model`、`readonly`、`is_background` 等。  
- `description` 影响 Agent **何时委托**给该子代理。

---

## 8. 与 Agent 协同的产品能力（非单独「工具名」，但常一起出现）

仍摘自 [Agent 概览](https://cursor.com/docs/agent/overview.md)：

### 8.1 Checkpoints

- 在 Agent 会话中保存代码库快照；重大修改前自动创建。  
- 可从时间线预览或 **Restore Checkpoint** 回滚。  
- **本地存储**，与 Git 无关；长期版本控制仍应用 Git。

### 8.2 Queued messages（排队消息）

- Agent 工作时可将后续指令排队，顺序执行。  
- `Enter` 入队；`Cmd+Enter`（文档写法）可立即插入并处理当前请求。

---

## 9. 附录：桌面端内部工具枚举与 UI 标签（工程线索）

> **注意**：以下为从 Cursor 桌面安装包 **客户端脚本** 中可见的 **枚举 → 短标签** 映射线索，用于对照日志/UI。**不等于**官方公开的「API 工具名」完整表，也**不等于**发给模型的长 `description` 字段（后者多在服务端或请求链路中组装）。

公开源码片段中常见映射逻辑（含义归纳）：

| 内部枚举（节选） | UI / 日志短名称 |
|------------------|-----------------|
| `READ_FILE` / `READ_FILE_V2` / `READ_MCP_RESOURCE` | Read |
| `RIPGREP_SEARCH` / `RIPGREP_RAW_SEARCH` | Grep |
| `GLOB_FILE_SEARCH` / `FILE_SEARCH` | Glob |
| `RUN_TERMINAL_COMMAND_V2` | Shell |
| `EDIT_FILE` / `EDIT_FILE_V2` | Edit |
| `LIST_DIR` / `LIST_DIR_V2` | LS |
| `SEMANTIC_SEARCH_FULL` / `READ_SEMSEARCH_FILES` | SemanticSearch |
| `DELETE_FILE` | Delete |
| `WEB_SEARCH` | WebSearch |
| `MCP` / `CALL_MCP_TOOL` | MCP |
| `TASK_V2` | Task |
| `CREATE_PLAN` | CreatePlan |
| `READ_LINTS` | ReadLints |
| `LIST_MCP_RESOURCES` | ListMCPResources |
| `TODO_WRITE` | TodoWrite |
| `ASK_QUESTION` | AskQuestion |

文档 **Agent 概览** 中的「十大类」是 **产品层归纳**；实现层可能存在更多细分工具（如 Lint、Glob、Delete、Plan、Todo 等），与 §3 表格 **不是简单一一对应**，设计自家 Agent 时建议以 **能力维度** 对齐而非强行合并为 10 条。

---

## 10. 局限与实施建议（给抄作业方）

1. **官方未在单页给出**「蛇形 API 名 ↔ 描述 ↔ 参数」的完整机器可读清单；教学页中的 `read_file` / `search_web` 仅为 **示例**。  
2. **MCP 工具** 由各服务器在 JSON schema 中自带 `description`；与内置能力并列扩展 Agent，但不属于本报告的「内置」枚举。  
3. 自建 Agent 若要 **像素级对齐 Cursor**，需在合规前提下分析自身客户端/网关的工具注册表；依赖外部逆向不具有可维护性。  
4. 推荐抄法：以 §3 **十大类** 作产品规格，以 §4–§7 作 **搜索/浏览器/子代理** 的补充说明，以 §9 作 **工程对照**，参数级 schema 自行定义。

---

## 11. 参考链接（官方）

| 主题 | URL |
|------|-----|
| Agent 概览 | https://cursor.com/docs/agent/overview.md |
| Tool calling fundamentals | https://cursor.com/learn/tool-calling.md |
| Semantic & agentic search | https://cursor.com/docs/agent/tools/search.md |
| Browser | https://cursor.com/docs/agent/tools/browser.md |
| Canvases | https://cursor.com/docs/agent/tools/canvas.md |
| Subagents | https://cursor.com/docs/subagents.md |
| Rules | https://cursor.com/docs/rules.md |

---

**文档维护**：若你更新本文件，请在段首「时效」或本脚注补充日期与变更摘要。
