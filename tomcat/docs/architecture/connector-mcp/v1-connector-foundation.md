# 连接器(Connector)模块：把 MCP / CLI / A2A 等外部能力接入 Agent 工具面（本期实现 MCP 连接器）

> 适用范围：给 Tomcat 增加通用连接器模块。连接器类型规划为 MCP / CLI / A2A；本期已实现 MCP 的 stdio 与 Streamable HTTP 传输，HTTP OAuth/PKCE 由标准 metadata discovery、loopback callback 与安全 token store 驱动。CLI/A2A 仍只保留扩展边界。
> 上位规范：[`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)（本文 `## 1`–`## 10` 对应规范 §1–§10）。相邻方案：[`plugin-system-overview.md`](../plugin-system-overview.md)（动态工具注册的现成范式）、[`skill-system.md`](../skill-system.md)（内置资产物化范式）、[`tools/read.md`](../tools/read.md)（图片回流的单一事实源）。
> 单一事实源：连接器抽象（`Connector` trait / `ConnectorType {Mcp, Cli, A2a}` / `ConnectorRegistry`）在 `core/connector/mod.rs`；**MCP 连接器**的运行时事实源在 `core/connector/mcp/manager.rs::McpManager`；工具暴露给模型经 `core/llm/system_prompt.rs::ToolSurface`；图片回流由 `core/agent_loop/tool_dispatcher.rs::extract_tool_result_media` 使用 `ChatMessageContentPart` 与 `openai_files` 共享原语完成。`core/connector/` 已在当前工作树落地；CLI/A2A 只保留枚举值，尚未创建实现目录。
> **项目 MCP 当前规则（2026-09）**：workspace MCP 配置通过 `workspace.project_resource_dir` 解析，默认 `<project>/.agents/mcp.json`。旧图里写死的项目 `.tomcat/mcp.json` 不能被前端或后端猜测、创建或打开。


**一句话定位**：连接器模块 = 把外部能力暴露的工具安全地接入 Tomcat。当前 MCP 支持 stdio 与 Streamable HTTP；HTTP 连接器可使用无认证、Bearer/custom headers 或标准 OAuth/PKCE。MCP 目录仍由 `McpManager` 维护并通过 v2 渐进式披露提供。

---

> **文档已拆分（总-分结构）**：本文是 **v1（连接器基座 / Option B）** 的完整设计。**总纲/导航**见 [../mcp-client.md](../mcp-client.md)；**v2（渐进式披露，最新且权威）**见 [v2-progressive-disclosure.md](./v2-progressive-disclosure.md)。v1 的 §3.1 **R4「MCP 工具进 `ToolRegistry`/进前缀」与 R9「Ready 即注册」已被 v2 修订**（该做法导致缓存前缀失稳 + token 爆炸）；本文其余决策（R2 传输 / R5 图片回流 / R7 信任 / R10 配置形状）**v1 仍是权威**。凡与 v2 冲突处以 v2 为准。

---

## 文首导读：方案导图集

### 阅读顺序建议

1. **A.1 抽象总图**：先看职责与事实源——谁负责拉起 server、谁持有工具目录、工具与图片如何回到模型、trust 在哪一环把关。
2. **A.2 具体总图**：再把同一条链路落到真实对象——`ConnectorRegistry` / `ToolRegistry` / `CompositeToolExecutor` / `McpManager` / `tool_dispatcher` 出口转换器 / `ChatMessageContentPart`。
3. **B 状态机**：最后看一个 MCP server 连接的生命周期（信任检查[默认放行] → 连接中 → 就绪 → 失败/重连 → 关闭；仅项目来源/命令变化需一次确认）。

### A.1 抽象 ASCII 总图（职责 / 事实源 / 分叉 / 终局）

> 专业：MCP 客户端把「配置里声明的外部 server」经信任门拉起，`tools/list` 得到的工具注册进 `ToolRegistry`、再经既有 `list_tools` 自动并入 `ToolSurface` 让模型可见；模型发起 `mcp__{server}__{tool}` 调用后，`tools/call` 的结果按内容类型分流：文本进工具消息，图片走 `follow_up_parts` 回流成模型可见 `InputImage`。
> 说人话：连接器模块是「把别人写的能力借来当自己工具」的统一框架；本期只实现 **MCP 连接器**。下面用 MCP 连接器 + `@playwright/mcp`（一个能开浏览器、点页面、截图的外部程序）走一遍，五步；关键分叉有两个——**信任门**（没批准就根本不启动这个外部程序）与**结果分流**（文字直接给模型看，图片走 Tomcat 已有的「截图变成模型输入」那条现成道）。

```text
连接器注册表 ConnectorRegistry（core/connector/mod.rs）
  ├─ MCP 连接器  ← 本期实现（下面走一遍）
  ├─ CLI 连接器  ← 预留（未来类型）
  └─ A2A 连接器  ← 预留（未来类型）
每种连接器都做同一件事：发现工具 → 并入工具面 → 调用 → 结果回流；差别只在「怎么连、怎么调」。
下面是 MCP 连接器 + @playwright/mcp 的一次完整调用：

起点：mcp.json 的 mcpServers 里声明了一个 server —— playwright（用 npx 启动，能开浏览器/截图）

  第①步  信任检查：这个外部程序，允许启动吗？（默认放行，Cursor 对齐）
  ────────────────────────────────────────────────────────────
    · 你自己全局 mcp.json 里的 server / 内置 curated / 命令没变 → 直接启动（无感，零步骤）
    · 只在两种情况需确认一次（非打断式弹框；本期在 /connector list 里 /connector trust 批一次）：
      项目仓库带来的 mcp.json【首次出现】、或已知 server 的【启动命令被改】
      （信任绑「启动命令」而非 server 名，专防共享仓库同名换命令 = MCPoison；细节见 §3.1 R7）
    · 用户拒绝确认 → 不启动
        │  批准通过
        ▼
  第②步  启动它，并问：「你有哪些工具？」
  ────────────────────────────────────────────────────────────
    启动子进程 → 握手 → 要来工具清单：
        browser_navigate / browser_click / browser_take_screenshot / ...
    （McpManager 把这份清单记在内存里 = 运行时唯一事实源）
        │  每个工具名字加前缀：browser_take_screenshot → mcp__playwright__browser_take_screenshot
        ▼
  第③步  把这些工具加进「模型能看见的工具菜单」
  ────────────────────────────────────────────────────────────
    模型菜单 = Tomcat 自带工具 ＋ 插件工具 ＋ MCP 工具
    （只是让模型"看见"，不改动 Tomcat 自带工具目录）
        │  模型决定：调用 mcp__playwright__browser_take_screenshot
        ▼
  第④步  真正去调用它，并把结果按类型分流  ← 关键分叉
  ────────────────────────────────────────────────────────────
    向 playwright 发起调用，拿回结果：
        文字（页面标题/元素信息） → 当作「工具结果文本」给模型读
        图片（截图 base64）       → 转成模型真正能"看见"的图片
             └─ 复用 Tomcat 已有的「截图喂给模型」管道；只补 JSON→parts 转换器，不新建图片管道
        ▼
  终局  下一轮，模型同时收到：工具结果文本 ＋（作为图片的）那张截图
        模型不支持看图 → 自动换成一段占位文字，不报错
```

### A.2 具体 ASCII 总图（真实对象 / 模块 / 运行时约束）

> 专业：把上面同一条链路落到 Tomcat 真实代码。**采用 Option B（§3.1 R4）：MCP 工具与插件一样进 `ToolRegistry`**——`ConnectorRegistry` 连上 server 后把工具 `register_tool` 进注册表（`plugin_id="mcp:{server}"`）；`CompositeToolExecutor` 按 `plugin_id` 把调用路由到 `McpManager`；图片回流靠 `tool_dispatcher` 出口的**媒体转换器**（对纯文本插件 no-op）。
> 说人话：下面**左边是人话「这一步在干嘛」，右边括号里是对应的代码位置**。MCP 不再单开一条前缀专线，而是和插件走同一条注册表路。

```text
开机：连上 MCP、把它的工具登记进「工具注册表」
   人话：ConnectorRegistry 连 server、要来工具清单，逐个登记进注册表（像插件那样）
   代码：context.rs 构造 ConnectorRegistry 并注入 GlobalServices；
         run_loop 每轮入口懒触发 spawn_connect_all()        (api/chat/context.rs / run_loop/mod.rs)
         生命周期协调器收到 Ready 才 register_tool(name=mcp__{server}__{tool},
         plugin_id="mcp:{server}", params=inputSchema)       (core/connector/mod.rs)
         执行器换成 CompositeToolExecutor{plugin, mcp}       (core/connector/mod.rs；DefaultToolRegistry::new 注入)
        │
        ▼
每轮向模型提问前：工具菜单【自动】含 MCP
   人话：observe_tool_surface 本就 list_tools 整个注册表，MCP 已在里面——不用为 MCP 单独合并
   代码：observe_tool_surface() → tool_registry.list_tools()；同文件新增一行
         spawn_connector_startup_if_needed() 懒触发后台预连  (api/chat/run_loop/mod.rs)
        │  模型点了 mcp__playwright__browser_take_screenshot
        ▼
总机 → 注册表按 plugin_id 路由
   人话：注册表查到该工具（已登记）→ 调 CompositeToolExecutor → 按 plugin_id 分流
   代码：run_tool_calls_with_usage → registry.call_tool(name)  (core/agent_loop/tool_dispatcher.rs)
         CompositeToolExecutor: plugin_id="mcp:playwright" → McpToolExecutor → McpManager::call_tool
                                其余 plugin_id             → PluginToolExecutor(JS VM)
                                                    (core/connector/mcp/executor.rs · 新增)
         └─ 返回 JSON（含 text/image 块）
        │
        ▼
出口转换器：把 JSON 结果里的图片翻成模型能看的图  ← 关键、且向后兼容
   人话：dispatcher 收到注册表返回的 JSON，扫出 image 块转成 InputImage；纯文本插件没 image 块→原样
   代码：extract_tool_result_media(result, files_runtime)   (tool_dispatcher.rs · 统一解释注册表 JSON 结果)
           text 块  → ToolExecOutcome.model_text
           image 块 → follow_up_parts（复用图片大小决策、Files API 与内容类型共享原语）
                      (core/llm/types.rs::ChatMessageContentPart, core/llm/openai_files.rs)
        │  ★ 对今天纯文本插件：无 image 块 → follow_up_parts 为空 → 行为与今天完全一致（no-op）
        ▼
收尾：把文字和图片交回对话
   人话：文字放「工具结果」，截图放「下一条用户消息」一起发；模型看不了图就换占位文字
   代码：tool_dispatcher 推 tool(text) + user_with_parts(图片)
         degrade_unsupported_multimodal(...)          (core/llm/multimodal.rs · 非 vision 降级)
```

> 阅读顺序（说人话）：从上往下就是一次完整调用——开机连 MCP 并把工具登记进注册表 → 工具菜单经 `list_tools` 自动含 MCP → 模型点了某个 MCP 工具 → 注册表按 `plugin_id` 路由到 `McpManager` → 出口转换器把结果里的图片翻成 `InputImage`（纯文本插件 no-op）→ 文字给模型看、图片走现成回流。**核心逻辑改两处**：`tool_dispatcher.rs`（出口转换器）、`context.rs`（建 `CompositeToolExecutor` + 注册 MCP 工具）；`run_loop` 另有一行只负责懒触发后台连接；无需改 `system_prompt`/`tool_exec`（`list_tools` 自动上工具面）。完整清单见 §5。

### B. 状态机：单个 MCP server 连接生命周期

> 专业：磁盘只有配置，无持久连接状态；下列状态由 `McpManager` 内存中的 `ServerStatus` 推导。信任检查默认放行（config 即信任，R7），只有项目来源首见/命令变化需一次确认。
> 说人话：你全局配的 server 配好直接连（无感）；只有「项目仓库带来的配置第一次用」或「命令被改」才需确认一次（本期 `/connector trust` 批一次，非启动时打断式弹框）。启动期失败会有限退避；运行中断线会立刻撤工具，需 `/connector test` 或 `/connector reload` 显式重连。

```text
PreSpawn 信任检查（spawn 前唯一关卡，默认无感放行）
  ├─ 默认放行：全局 ~/.tomcat/mcp.json 的 server / 已确认过的项目 server / 命令未变（curated 同此）
  │      └──▶ Connecting ──(initialize + tools/list)──▶ Ready
  │                │ 启动期握手超时/进程退出 → Failed（有限退避）
  │                Ready ──传输断裂──▶ Disconnected ──/connector test 或 reload──▶ Ready
  │                （Failed/断开：从工具面撤下其工具；in-flight 调用不 replay）
  │
  └─ 需一次确认：项目 <workspace>/.tomcat/mcp.json【首次出现】或【已知 server 命令被改】(← MCPoison 攻击面)
         · 不自动连（后台预连跳过、不阻塞会话）；在 /connector list 显示为「待确认」+命令 diff
         ├─ /connector trust <server>（未来 GUI 确认弹窗）→ 记入 connector-trust.json（绑 command 指纹）→ Connecting
         └─ /connector deny 或不批 → Blocked（不 spawn，工具面不含其工具）

关机 / McpManager Drop（任意状态）→ 释放 rmcp 的 RunningService；子进程收尾由 rmcp 管理
```

| 当前状态 | 事件 / 条件 | 目标状态 | 副作用 | 说人话 |
|----------|-------------|----------|--------|--------|
| PreSpawn | 全局 `mcp.json` 声明 / curated / 已确认的项目 server / 命令未变 | Connecting | 直接 stdio spawn 子进程（**默认无感**） | 你自己配的直接连，跟 Cursor 一样。 |
| PreSpawn | 项目 `mcp.json` **首次出现** 或 **已知 server 命令变化**（MCPoison 面） | NeedsConfirm | **不自动连**（后台预连跳过它，不阻塞会话）；在 `/connector list` 显示为「待确认」+ 命令 diff；浮动版本额外**告警建议钉死**（不拦） | 别人仓库带来的、或命令被改了，先晾着不连、列出来等你批。 |
| NeedsConfirm | 用户批准：`/connector trust <server>`（未来 GUI 为确认弹窗） | Connecting | 把绑定 `command` 指纹的身份写入 `connector-trust.json`，spawn | 你 `/connector trust` 批一次，之后就记住了。 |
| NeedsConfirm | 用户 `/connector deny`（或忽略） | Blocked | 不 spawn，工具面不含其工具 | 不批就不启动，晾着也不连。 |
| Connecting | initialize + tools/list 成功 | Ready | 缓存工具目录；生命周期协调器把该 server 的工具注册进 `ToolRegistry` | 握手完拿到工具清单才算就绪，工具才会出现在下一轮菜单。 |
| Connecting | 握手超时 / 进程立即退出 | Failed | 启动期后台任务有限退避；该 server 缺席工具面 | 起不来就记错误，不卡住 agent。 |
| Ready | 传输断裂（EOF/管道断） | Disconnected | 先撤销 `plugin_id="mcp:{server}"` 的已注册工具，再标记失效；**不 replay**在途调用 | 断了先把失效工具从菜单撤下，别让模型点一个必失败的工具。 |
| Disconnected | 用户 `/connector test` 或 `/connector reload` | Ready | 重新 `tools/list`，再重新注册工具 | 显式重连时重新取工具，工具可能更新。 |
| any | 会话关闭 / `McpManager` Drop | (terminated) | 释放 rmcp `RunningService`；Tomcat 未额外配置进程组或行长上限 | 生命周期交给协议库，Tomcat 不重复造一套进程管理。 |

---

## 1. 术语统一

| 术语 | 语义 | 数据载体 | 行为约束 | 说人话 |
|------|------|----------|----------|--------|
| 连接器(Connector) | 「接入一类外部能力并把其工具暴露给 Agent」的统一抽象；MCP/CLI/A2A 各是一种实现。 | `core/connector/mod.rs::Connector`(trait) | 每个连接器提供：发现工具 / 并入工具面 / 调用 / 结果回流。本期只实现 MCP。 | 把某类外部能力接进来当工具的统一插座。 |
| ConnectorType | 连接器类型枚举 `{Mcp, Cli, A2a}`。 | `core/connector/mod.rs::ConnectorType` | 本期只有 `Mcp` 有实现；`Cli`/`A2a` 仅枚举预留，尚未定义目录、前缀或连接方式。 | 这个连接器是哪一类。 |
| ConnectorRegistry | 所有连接器的注册/发现/生命周期入口，注入 `GlobalServices`。 | `core/connector/mod.rs::ConnectorRegistry` | 启动期 `spawn_connect_all()`；生命周期协调器收到 Ready/NotReady 事件才 `register_tool`/`unregister_plugin_tools("mcp:{server}")`。它只持有 `Weak<dyn ToolRegistry>`，`McpManager` 不反向持有 registry，避免异步任务形成强引用环；执行经 `CompositeToolExecutor` 按 `plugin_id` 路由。本期只注册 MCP。 | 管所有连接器的总台：后台连上才把工具挂出来，断了就撤下。 |
| MCP server | 一个对外暴露工具/资源的外部进程，讲 JSON-RPC（MCP 协议）；是 MCP 连接器连接的目标。 | `core/connector/mcp/config.rs::McpServerConfig` | 本期仅 stdio 子进程；由 MCP 连接器作为**客户端**连它。 | 别人写的「工具服务器」，MCP 连接器去连它。 |
| stdio 传输 | 用子进程的 stdin/stdout 传 JSON-RPC 帧。 | `core/connector/mcp/transport.rs`（经 `rmcp` crate） | Tomcat 明确执行 `env_clear` 后只注入 config `env` + `PATH`/`HOME`；子进程 Drop 语义、协议帧限制交给 `rmcp`，本仓未另设 8MiB 单行上限。 | 通过管道跟子进程一问一答，环境清干净但留下能找到 node 的必需项。 |
| McpManager | MCP 连接与工具目录的运行时唯一事实源。 | `core/connector/mcp/manager.rs::McpManager`，注入 `GlobalServices` | 持有每 server 的连接状态与工具缓存；`list_tools()` / `call_tool()`。 | 管所有 MCP 连接的总管。 |
| 工具名映射 | 模型可见名 `mcp__{server}__{tool}` ↔ 协议原始 `(server, raw_tool)`。 | `core/connector/mcp/naming.rs` | 双下划线分隔；**常态原样保留完整可读名**，仅当超 provider 名长上限（OpenAI 64）时才消毒 + 截断可读头 + 短哈希后缀；**原始名单独存对照表供 `tools/call`**（哈希不可逆）。 | 常态给模型完整可读名；太长才截断加小尾巴，调用时查表翻回真名。 |
| ToolSurface | 发给模型的工具定义合集（builtin + plugin + MCP）。 | `core/llm/system_prompt.rs::ToolSurface` | MCP 工具**和插件一样注册进 `ToolRegistry`**（Option B，§3.1 R4），经 `observe_tool_surface` 的 `list_tools` **自动并入** surface（不需为 MCP 单独合并）；只读子 agent（code reviewer / verifier）走 `plan_runtime::review.rs::resolve_internal_tools` 按 **builtin 目录**白名单过滤，MCP/插件都不在 builtin 目录里，**天然不泄漏给子 agent**。 | 模型「看得见哪些工具」的那张清单；MCP 像插件一样自动进来，子 agent 看不到。 |
| follow_up_parts | 工具执行后追加到「下一条 user 消息」的多模态内容。 | `core/agent_loop/tool_exec/mod.rs::ToolExecOutcome.follow_up_parts` | 承载 `ChatMessageContentPart::InputImage`；由 `tool_dispatcher` 注入。 | 工具产出的图片，塞进下一条消息给模型看。 |
| InputImage 回流 | 把图片（base64 内联或 Files API 上传）变成模型可见 vision 内容。 | `core/llm/types.rs::ChatMessageContentPart` + `core/llm/openai_files.rs` | MCP 单独完成 base64 解码与结果分流，复用 `upload_decision_by_size` / `OpenAiFilesRuntime` / `ChatMessageContentPart` 这些共享原语；不是调用 `read.rs`。 | 「让模型真正看见图」的现成零件直接复用，不复制上传策略。 |
| Trust 身份 | 一个 server 的信任判据，**默认 = 启动命令指纹** `hash(command+args+env+cwd)`（绑命令而非 server 名，防 MCPoison）；**可选强档**再加本地入口内容身份（SHA256/SHA512）。 | `core/connector/mcp/trust.rs::TrustStore`（`~/.tomcat/connector-trust.json`） | 记录只保存用于展示的安全启动快照（command/args/cwd；环境变量永不保存；常见敏感参数值脱敏），哈希仍是唯一判据。全局/curated 默认信任；项目首见 / 指纹变化需确认；浮动版本只告警不拦。 | 记住「我放行过的启动方式」；命令被换了才再问一次，密钥不会出现在列表或信任文件里。 |
| curated 条目 | Tomcat 自带、默认信任的 MCP server 声明（如**钉死版本**的 `@playwright/mcp@x.y.z`）。 | init 物化的 `~/.tomcat/mcp.json` 的 `mcpServers` 默认段 | 内置且钉死版本，走全局 allowlist **默认信任、无感直连**；用户自己加的全局 server 同样开箱即用（config 即信任）。 | 我们预置那条开箱即用；你自己全局加的也一样直接用。 |
| 在途调用不 replay | 传输在 `tools/call` 途中断裂时，只为**后续**调用重连，不重放当前调用。 | `McpManager::call_tool` 错误分支 | 借 pi_agent_rust 的 `MCP_DELIVERY_INDETERMINATE` 纪律。 | 一个调用一半断了，宁可报错也不重来（可能已执行）。 |

> 时间点钉死：「工具面构造」发生在 `run_loop` **每轮构造 `ChatRequest` 之前**（`observe_tool_surface()` → `list_tools()` 已含 MCP）；「图片回流」发生在 `tool_dispatcher` 出口 `extract_tool_result_media()` 产出 `follow_up_parts` 后、推 `ChatMessage::user_with_parts` **那一刻**，早于下一次 `ChatRequest`。

---

## 2. 竞品 / 选型对比（调研）

调研了 11 个仓库的 MCP 客户端实现，核心问题：**一个 Rust/多进程 agent 如何连外部 MCP server、把工具接进模型、并把结果（尤其图片）喂回去**。两条最同构的 Rust 前例是 Codex（用 `rmcp` crate）与 pi_agent_rust（手写客户端）；图片处理上 continue 与 pi_agent_rust 是反例。

| 竞品 | 形态 | 关键设计（含 file:symbol） | 我们借鉴的点 | 说人话 |
|------|------|----------------------------|---------------|--------|
| **Codex**（Rust） | `rmcp` crate + 三层 | `rmcp-client/src/rmcp_client.rs::RmcpClient`（stdio+HTTP、initialize、list/call）；`protocol/src/models.rs::convert_mcp_content_to_items`（image→`input_image` data URL）；配置 `config/src/mcp_types.rs` `[mcp_servers.{name}]`；命名 `mcp__{server}__{tool}`；30s/300s 超时 | 用官方 `rmcp` 而非手写传输；image→data URL→vision；`mcp__` 命名；per-server/tool 审批模式 | 直接用官方 Rust MCP 库，图片转 data URL 喂给模型。 |
| **pi_agent_rust**（Rust） | 手写客户端（无 rmcp） | `src/mcp/transport.rs::StdioTransport`（NDJSON、`env_clear`+白名单）；`src/mcp/trust.rs::TrustStore`（**spawn 前指纹信任 + 可执行 SHA256**）；`mcp__{server}__{tool}`；无 image→vision（图片入 `details`，**反例**）；`MCP_DELIVERY_INDETERMINATE` 不 replay | **trust-before-spawn 指纹门**；在途不 replay 纪律；工具挂成原生 `Tool`；避免其「图片不回流」缺陷 | 起进程前先按指纹验授权；断了不重放；但别学它把图片丢掉。 |
| **pi**（TS） | **无 MCP，刻意** | `packages/coding-agent/README.md`「No MCP … build an extension」；仅通用 `normalizeToolResultImages` | 「核心保持最小、MCP 作为可选层」的哲学取舍参考 | 有人干脆不做 MCP，把它当可选扩展。 |
| **cline**（TS） | `@modelcontextprotocol/sdk` | `apps/vscode/src/services/mcp/McpHub.ts`；`sdk/.../ai-sdk-format.ts::toToolResultImagePart`（image→AI SDK file part）；`autoApprove: string[]`；`server__tool` | image→provider part；per-tool auto-approve 清单 | 官方 SDK，图片转成 provider 的文件块，白名单免确认。 |
| **continue**（TS） | `MCPManagerSingleton` | `core/tools/callTool.ts`：非 text/resource 一律报错——**image 被丢弃**（反例）；`server_tool` + `mcp://` URI | 反例：视觉验收场景绝不能丢图 | 它直接把图片当不支持类型丢了，正是我们要避免的。 |
| **opencode**（TS） | Effect `MCP.Service` | `packages/opencode/src/session/tools.ts`：image→session 附件 data URL；`server_tool` | image→data URL 附件再由 provider 适配 | 图片先变附件再按模型能力转。 |
| **openclaw**（TS） | 自定义传输 | `src/agents/mcp-content.ts::mcpContentBlockToAgentContent`（image→`{type:image}`）；Codex 式审批模式；tool filter include/exclude | 干净的 content→agent 映射边界；工具过滤 | 有个专门函数把 MCP 内容翻成 agent 内容。 |
| **vscode**（TS） | 原生客户端 | `contrib/mcp/common/mcpRegistry.ts::_checkTrust`（**nonce 指纹信任 + workspace trust**）；`mcp_{server}_{tool}`；image→`IToolResult` data 部件；`mcp.json`（user/workspace/.vscode） | nonce/指纹信任模型；readOnlyHint 决定是否预确认 | 用启动配置的哈希做信任，改了就重新问。 |
| **hermes-agent**（Py） | 官方 Python SDK | `tools/mcp_tool.py`：`trust: untrusted`+approval gate；`_cache_mcp_image_block`→`MEDIA:{path}`；`mcp__{server}__{tool}` | 信任分级 + 未信任写操作需批准 | 分「可信/不可信」，不可信又写操作才拦。 |
| **langchain** | 仓内**无客户端** | 客户端在外部 `langchain-mcp-adapters`（`tools.py::_convert_mcp_content_to_lc_block` image→`create_image_block`） | image→标准 content block 的转换函数形态 | 它把客户端拆成独立包，转换函数可参考。 |

**为什么选「rmcp + 复用 follow_up_parts 图片回流 + config 即信任 + 命令指纹防 MCPoison」而不是别的（3–5 条）**：

1. **不手写传输**：Codex 证明 `rmcp` 在 Rust core 里可用；pi_agent_rust 手写传输是额外维护负担（NDJSON 编解码、握手、超时全要自己维护）。除非 `rmcp` 被证明过重/不稳，否则不重复造轮子（推翻条件见 §3.1 R1）。
2. **图片必须回流**：本子系统存在的唯一动机是 Phase 2 的**视觉验收**；continue 丢图、pi_agent_rust 图片入 details 都会让「截图→模型看见」断链。Tomcat 的 `follow_up_parts`+`InputImage` 已经提供回流数据结构；MCP 复用大小决策、Files API 和内容类型共享原语，不复制上传策略。
3. **config 即信任（Cursor 对齐），命令指纹防 MCPoison**：Cursor 的实践是「配置里有就自动启动」，用户配好零额外步骤——本方案对齐（全局 `mcp.json`/curated 无感直连）。但 Cursor 早年栽在「信任只认 server 名、不认命令」（CVE-2025-54136 MCPoison，1.3 修复），故唯一保留的门是把信任**绑到启动命令指纹**：项目仓库带来的配置首见 / 命令被改时 spawn 前确认一次。内容哈希（integrity/SHA256）降为**可选强档**；浮动版本对用户 server 只告警不拦（Cursor 亦不拦）。Tomcat 现有 `PermissionGate` 是路径/bash 语义、tool-call 审批是另一层（正交），故 spawn 侧用轻量的命令指纹信任即可（R7）。
4. **统一进 registry + 出口转换器（Option B）**：MCP 工具像插件一样注册进 `ToolRegistry`（`plugin_id="mcp:{server}"`，`CompositeToolExecutor` 路由到 `McpManager`），`list_tools` 自动上工具面；`ToolRegistry` 契约是 JSON-only（无原生图片 parts 通道），故在 `tool_dispatcher` 出口用 `extract_tool_result_media()` 把 `image` 块转为 `follow_up_parts`（复用共享大小/上传/内容类型原语），**对纯文本插件 no-op、向后兼容**。这比给 MCP 单开 `mcp__` 前缀专线更优雅：消除可见/执行不对称与前缀抢注隐患，还顺带解锁「能返图的插件」（见 §3.1 R4）。
5. **传输可插拔、本期落 stdio**：MCP 规范（2026-07-28）里 **stdio 是本地 server 的一等且最常见传输**（不是"简化版"；"MCP 都是 HTTP" 是常见误解——HTTP 是给远程/托管 server 的）。`@playwright/mcp` 默认就是 npx stdio，视觉验收用它零网络零 OAuth。连接器框架把传输做成可插拔，**Streamable HTTP（远程）与 OAuth 已写进设计、列为紧跟子阶段**（`rmcp` 已支持 streamable http，增量小），见 §3.1 R2 与「传输」章。

---

## 3. 落地选型与实施（已定稿）

### 3.1 落地选型决策表

> 每行一个可辩驳分叉；`取自` 至少一条本仓证据 + 一条外部 agent 证据。

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
|------|------|------|------|----------|---------------|--------|
| **R-C0 连接器抽象** | 直接做 MCP 客户端，还是抽象成「连接器」框架？ | **采用** `Connector` trait + `ConnectorType {Mcp,Cli,A2a}` + `ConnectorRegistry`；**本期只实现 MCP 连接器**，CLI/A2A 预留为未来类型 | Tomcat：本仓 `skill`/`plugin` 子系统的「注册表 + 多实现」模式（`core/skill`、`ext/plugin`）；外部：`codex codex-mcp`（MCP 连接分层）、`hermes-agent tools/mcp_tool.py`（连接器式工具注册） | 设计：`ConnectorRegistry` 按已知配置源构造连接器（本期 `mcp.json`→MCP 连接器；未来 CLI/A2A 各有配置源，`type` 在代码侧而非强推进 MCP 配置，见 R10），工具注册进 `ToolRegistry` 后由 `CompositeToolExecutor` 按 `plugin_id`（`mcp:`/`cli:`/`a2a:`）路由（Option B，R4）；MCP 是首个实现。理由：MCP/CLI/A2A 都是「接入外部能力→暴露工具→回流结果」的同构问题，统一抽象避免将来各写一套；本期只填 MCP，抽象成本极低。 | 未入选：只做 `McpManager` 不抽象。拒因：CLI/A2A 迟早要接，届时重构工具面/命令面成本更高；现在留好接缝更省。 | 把「接外部能力」做成统一插座，本期只插上 MCP，CLI/A2A 以后即插即用。 |
| **R1 传输库** | 手写 stdio JSON-RPC 还是用官方 `rmcp` crate？ | **采用** `rmcp` crate 做 stdio 传输与 JSON-RPC；**拒绝** 手写传输 | Tomcat：新增 `core/connector/mcp/transport.rs` 薄封装；外部：`codex/codex-rs/rmcp-client/src/rmcp_client.rs::RmcpClient` 用 `rmcp` | 设计：`rmcp` 提供 `Transport<RoleClient>`、initialize 握手、分页 `tools/list`；封装成 `McpTransport`。理由：Codex 生产验证；握手/框架/超时无需自维护，代价是 +1 依赖树。 | 未入选：pi_agent_rust `src/mcp/transport.rs` **手写 NDJSON 传输**。拒因：JSON-RPC 编解码/握手/取消全自维护，纯负债，除非 rmcp 不可用。 | 用官方 Rust MCP 库，别自己手搓协议。 |
| **R2 MCP 传输范围** | MCP 连接器本期实现哪些传输？ | **框架传输可插拔（代码 `McpTransport` trait）；本期只实现 stdio**；**Streamable HTTP（2026-07-28 单端点）+ OAuth 写进设计、列为紧跟子阶段**（P8）；不做已废弃的 legacy HTTP+SSE | Tomcat：`McpTransport` trait（`StdioTransport` 本期 / `HttpTransport` 预留）——`mcp.json` **无 `transport` 字段**：有 `command` 即 stdio、P8 以 `url` 键区分 HTTP（生态惯例，同 Cursor/Claude）；外部：`codex rmcp-client`（`rmcp` 同一 crate 支持 stdio + streamable http）、`@playwright/mcp`（默认 stdio，`--port` 启 HTTP）、MCP 规范 2026-07-28（stdio + Streamable HTTP 两种一等传输，HTTP+SSE 已废弃） | 设计：`McpTransport` trait 后面挂 `StdioTransport`（本期）与 `HttpTransport`（P8）；`@playwright/mcp` 默认 stdio，视觉验收零网络零 OAuth。理由：**纠正"MCP=HTTP"误解**——stdio 是本地 server 的一等且最常见传输；Streamable HTTP 是给远程/托管 server 的，`rmcp` 已支持故增量小，但 OAuth（认证远程）最重，单列不 front-load。 | 未入选：(a) 本期就做 HTTP/SSE/OAuth 全套——拒因：驱动本期的 playwright 只需 stdio，OAuth 拖成大工程；(b) 只做 stdio 且不给 HTTP 留接缝——拒因：远程 MCP 是真实未来需求，`McpTransport` trait 留好接缝几乎零成本。 | 本地连接用 stdio（本期），远程 HTTP/OAuth 设计好、紧跟做；别被"MCP 都是 HTTP"带偏。 |
| **R3 工具命名** | 模型看到的工具名怎么起？ | **采用** `mcp__{server}__{tool}`（双下划线）；**仅当超 provider 名长上限时**才「消毒 + 截断保留可读头 + 短哈希后缀」（非整名变哈希）；原始 `(server,tool)` 另存对照表 | Tomcat：新增 `core/connector/mcp/naming.rs`；外部：`codex/codex-rs/codex-mcp/src/tools.rs:226,265-285`（`MAX=128`，`truncate_name+append_hash_suffix`）、`cline/.../mcp/name-transform.ts:4-34`（`MAX=64`，`${baseName}_${hash}`）、`pi_agent_rust::mounted_name`（64 cap+SHA256）、`hermes-agent` `mcp__` | 设计：双下划线定界最不易与模型名/服务名冲突；**常态原样保留完整可读名**（`@playwright/mcp` 的 `browser_*` 都 <64，永不触发兜底）；仅超 64/含非法字符时截断可读头 + 短哈希去重；`tools/call` 一律查 `McpToolDef` 对照表取原始名（哈希单向不可反解，故必须另存）。理由：OpenAI 硬限 function name 64 字符是真实约束，4 家 agent 都这么兜底，可读性由「保留可读头」保住。 | 未入选：(a) cline/continue/opencode 的单下划线 `server_tool`——拒因：名字内含下划线时定界歧义；(b) 超长时整名变纯哈希（初稿举的坏例子）——拒因：可读性差，改为保留可读头。 | 常态就是完整可读名 `mcp__playwright__browser_click`；只有名字长到超 API 上限才截断+加小尾巴，且尽量留可读头；调用时靠对照表翻回真名。 |
| **R4 工具面接入 + 执行路径** | MCP 工具独立走 `mcp__` 前缀分支，还是统一进 `ToolRegistry`？ | **采用 Option B**：MCP 工具**注册进 `ToolRegistry`**（`plugin_id="mcp:{server}"`，由 `CompositeToolExecutor` 按 `plugin_id` 路由到 `McpManager`）；`list_tools` 自动并入工具面；`tool_dispatcher` 的 registry 分支用向后兼容的**工具结果媒体转换器**把 `content[].type=="image"` 变成 `follow_up_parts`（纯文本插件 no-op）。**拒绝**独立 `mcp__` 前缀分支。 | Tomcat：`core/tools/contract/registry.rs::{ToolExecutor,ToolRegistry}`（`DefaultToolRegistry` 持单个 `executor: Arc<dyn ToolExecutor>`、`call_tool` 的边界是 JSON）、`tool_dispatcher.rs::extract_tool_result_media`、`run_loop/mod.rs::observe_tool_surface`、`ext/plugin_tool_executor.rs`；外部：`codex convert_mcp_content_to_items`（JSON image→input_image 转换器范式）、`pi_agent_rust` 把 MCP 挂成原生 `Tool`。 | 注册表/插件契约是 JSON-only（`execute -> serde_json::Value`，因插件是 rquickjs JS-VM），原来没有承载原生图片 parts 的通道（是「无通道」非「清空」）。Option B 的三件事：(1) `McpToolExecutor` 包 `McpManager`，`CompositeToolExecutor` 按 `tool.plugin_id` 路由；(2) `ConnectorRegistry` 在 Ready 后把工具登记进 `DefaultToolRegistry`；(3) dispatcher 用 `extract_tool_result_media()` 将 text 写进 `model_text`、将 image 块转 `follow_up_parts`。它复用上传/内容类型的共享原语，不调用 `read.rs` 的流程；对纯文本插件 no-op。 | 未入选：(a) surface-only + `mcp__` 前缀分支——可见/执行两条路不对称、靠字符串前缀路由有抢注隐患；(b) 注册进 registry 但不加转换器——图片仍回流不了。 | 让 MCP 和插件走同一条注册表路；出口加一个「JSON 图片→图片」的小转换器，MCP 截图就能回来，纯文本插件不受影响。 |
| **R5 图片回流** | MCP 返回的 image 块怎么进模型？ | **采用** `follow_up_parts` + `InputImage`（小图内联、大图 Files API）；**拒绝**新建一套上传策略。 | Tomcat：`tool_dispatcher.rs::mcp_image_part`、`core/llm/types.rs::ChatMessageContentPart::{image_base64_data,image_file_id}`、`openai_files.rs::upload_decision_by_size`；外部：`codex/.../models.rs::convert_mcp_content_to_items`（image→input_image）、`cline::toToolResultImagePart`。 | MCP 路径单独解 base64 和组装结果；base64 块小图调 `image_base64_data(mime,b64)`，大图落临时文件后调 `OpenAiFilesRuntime::resolve_or_upload_path`，文本块进 `model_text`，非 vision 由 `degrade_unsupported_multimodal` 降级。共享的是大小策略、Files API 和消息类型，不是假称调用 `read.rs`。 | 未入选：continue `callTool.ts` 把 image 当不支持类型报错**丢弃**；pi_agent_rust 图片入 `details` 不回流。拒因：视觉验收场景丢图 = 功能失效。 | 图片会进模型能看见的 InputImage；上传规则共用，流程不复制。 |
| **R6 配置位置与作用域** | MCP server 声明放哪、怎么分层？ | **采用** `~/.tomcat/mcp.json`（全局）+ 项目级 `<workspace>/.tomcat/mcp.json`（覆盖同名 server）+ 主配置 `[connector] enabled` 总开关；init 物化 curated 默认；**形状见 R10** | Tomcat：`infra/config/types/skills.rs::SkillsConfig` 分层、`skill/builtin.rs::materialize_builtin_skills` 物化；外部：**Cursor `~/.cursor/mcp.json`(user) + 项目 `.cursor/mcp.json`**、`vscode` `mcp.json`(user/workspace)、Claude Desktop `claude_desktop_config.json` | 设计：user + workspace 两层，workspace 覆盖同名 server；`~/.tomcat/mcp.json` 缺失时物化含 `@playwright/mcp` 的默认；总开关在主配置 `[connector] enabled`。理由：与 Cursor/VS Code/Claude 的 user+workspace 分层完全一致，用户心智零迁移；server 列表独立于主配置便于整文件分享/复制粘贴。 | 未入选：全塞进主 `tomcat.config.toml` 的 `[mcp.servers]`（codex 式）。拒因：server 列表频繁增删、且要能整段从生态复制粘贴，独立文件更合适。 | server 清单放独立的 `mcp.json`，全局一份、项目可覆盖，自带一条 playwright。 |
 | **R10 配置形状与最小字段面（对齐生态标准 `mcpServers`）** | 用什么配置形状、必填几个字段？ | **采用** 生态标准 `mcpServers` JSON 形状：`{"mcpServers":{"<name>":{"command","args","env?","cwd?"}}}`。**每条 server 必填 `command`+`args`**（`name`=键），`env`/`cwd` 可选；`trusted`/`integrity`/`startupTimeoutMs`/`callTimeoutMs`/`toolFilter` 为**可选高级字段**；**MCP 配置无 `type` 字段**（文件即声明 MCP，连接器类型在代码侧）。curated `playwright` 预置。**推翻条件**：当 CLI/A2A 真正落地、需同一文件声明多类型连接器时，再评估引入带 `type` 的统一配置（届时 `mcp.json` 可作为 MCP 专属子配置保留）。 | 外部：**Cursor `~/.cursor/mcp.json`（用户截图：`{mcpServers:{playwright:{command:"npx",args:["-y","@executeautomation/playwright-mcp-server"]}}}`）**、Claude Desktop `claude_desktop_config.json`（同 `mcpServers`）、`vscode`（`servers`）、`codex` `[mcp_servers]`（TOML 变体，字段同构）；Tomcat：`core/connector/mcp/config.rs::McpServerConfig`（serde `#[serde(default)]` 实现可选字段） | **第一性原理**：Cursor「配置简单」的根因**不是字段少，而是用了生态既成事实的 `mcpServers` 形状——从任意 MCP 文档/注册表/Cursor 复制一段即用、零翻译**，这是最大的 onboarding 收益。必填收敛到 `command`+`args`（与 Cursor 完全一致）降低认知负担；把 `type` 留在代码而非强推进配置，避免在 CLI/A2A 尚未落地时就摊派「本不该存在」的字段（YAGNI）。安全所需的 `trusted`/`integrity` 做**可选**；裸 Cursor 片段写入全局配置会直接连接，项目配置则按 R7 的首见确认规则处理。 | 未入选：(a) 自研多类型配置文件 + 每条带 `type`（本文初稿 R6）——拒因：与生态 `mcpServers` 片段不可直接复制、要手工翻成 TOML；只有 MCP 时 `type` 是冗余字段。(b) 必填 `type`/`transport`/`trusted`——拒因：Cursor 仅需 `command`/`args` 即连上，多一个必填都是摩擦。 | 用大家都在用的 `mcp.json`（`mcpServers`）格式，网上或 Cursor 抄一段就能用；只有 command 和 args 必填，环境变量、目录想填才填。 |
| **R7 信任模型（Cursor 对齐：config 即信任 + MCPoison 防护）** | 用户配好一个 MCP 要不要额外一步才能用？怎么防被掉包/同名换命令？ | **默认 config 即信任、零额外步骤（Cursor 对齐）**：出现在**用户全局 `~/.tomcat/mcp.json`** 的 server（含内置 curated）**直接自动连接、开箱即用，无 `/connector trust` 步骤**。**唯一保留的门是 Cursor 踩坑后补的那一个**（CVE-2025-54136 MCPoison）：信任**绑定到实际启动命令**（`command+args+env+cwd` 指纹）而非仅 server 名——**项目级 `<workspace>/.tomcat/mcp.json`**（可能随不可信仓库进来）**首次出现**、或**已知 server 的命令被改**时，spawn 前**一次性确认**。浮动版本（`@latest`）对用户 server **只告警不拦**（curated 内置仍钉死）；内容哈希（integrity/SHA256）为**可选强档**，非必需。 | 外部：**CVE-2025-54136「MCPoison」（Check Point）——Cursor 早期把信任钉在 config 的 server 名上、不绑实际命令，攻击者在共享仓库 `.cursor/mcp.json` 同名换命令即静默执行；Cursor 1.3（2025-07）改为绑定实际命令**；Cursor 文档：全局/项目 `mcp.json` 合并、项目优先、**配置即自动启动**（无 spawn 审批，tool-call 才审批）；`vscode::mcpRegistry._checkTrust`（nonce=启动配置哈希 + **workspace trust**）；Tomcat：新增 `core/connector/mcp/trust.rs`（`~/.tomcat/connector-trust.json`）、可选强档参照 `pi_agent_rust::StoredExecutionIdentity`（二进制 SHA256） | 第一性原理——**「人把 server 写进自己的配置文件」本身就是同意**（Cursor 同理），全局配置=你的机器，零摩擦。真正的 agent 风险是「**项目仓库带来的 `.tomcat/mcp.json` 被同名换了命令**」——这正是 MCPoison；对策不是给所有 server 加确认，而是**只在项目来源首见 + 命令变化时确认**，且把信任**绑到命令而非名字**。这样 UX 与 Cursor 一致（全局即用），却避开了 Cursor 曾经的坑。version/integrity 做成「curated 钉死 + 用户 server 告警 + 可选强档」，不给普通用户添堵。 | 未入选：(a) 每个 server spawn 前都要 `/connector trust`（本文初稿）——拒因：与「跟 Cursor 一样开箱即用」冲突，普通场景纯摩擦；(b) 只按 server 名信任（Cursor 早期）——拒因：正是 MCPoison，同名换命令静默执行；(c) 硬拦所有 `@latest`——拒因：Cursor 都不拦，用户 server 告警即可，别挡路；(d) 强依赖 `PermissionGate` 或 per-call 审批——拒因：Gate 是路径/bash 语义、tool-call 审批是另一层（本就有），与 spawn 信任正交。 | 你自己全局配的 MCP，配好直接能用，跟 Cursor 一模一样，不用额外点信任。只有「项目仓库里带来的 MCP 配置」第一次用、或某个 MCP 的启动命令被人偷偷改了，才需确认一次（本期在 `/connector list` 里 `/connector trust` 批一次，不是启动时打断你的弹框）——因为 Cursor 早年就栽在「只认名字不认命令」上（MCPoison），我们把这一个坑堵上。 |
| **R8 在途失败语义** | `tools/call` 途中传输断裂怎么办？ | **采用** 当前调用不 replay；标记 `Disconnected` 并撤工具；用户显式 `/connector test` 或 `/connector reload` 才重连。启动期失败仍有限退避。 | Tomcat：`McpManager::call_tool` 错误分支、`ConnectorRegistry::connect_with_backoff`（仅启动期）；外部：`pi_agent_rust` `MCP_DELIVERY_INDETERMINATE`（不 replay）、`codex` `reinitialize_after_session_expiry`（仅 HTTP session）。 | stdio 传输断裂时当前 `call_tool` 返回明确错误并标记 `Disconnected`。自动重放会重复副作用；运行中是否自动重连还需要有不与 deny/reload 竞态的生命周期设计，本期不假称已实现。 | 未入选：自动重放在途调用。拒因：MCP 工具可能有副作用，重放不安全。 | 断在半路的调用宁可报错，也不重来；需要恢复时用 test/reload 明确重连。 |
| **R9 连接时机与工具生命周期** | 配置好的 MCP server 何时连接？同步卡住首轮、后台预连，还是模型首次调用时才连？ | **采用** Chat/serve 会话初始化后，对每个**默认放行的 server**（全局/curated/已确认项目 server，R7）**后台并行预连** `ConnectorRegistry::spawn_connect_all()`（需确认的项目来源 server 不自动连、留待用户确认）；不 await、不阻塞首轮/serve handshake。`Ready` 事件才注册 `plugin_id="mcp:{server}"` 的工具；断裂/失败先 `unregister_plugin_tools`，用户 test/reload 重连成功后 Ready 再登记；协调器只持 `Weak<dyn ToolRegistry>`，避免强引用环。 | Tomcat：`core/skill/discovery.rs::spawn_discovery_task`、`api/chat/context.rs::spawn_skill_discovery_if_needed` 的启动期后台任务先例；`api/chat/run_loop/mod.rs::observe_tool_surface` 每轮 `list_tools(None)`，故工具登记后天然在下一轮出现。外部：Codex `codex-mcp/src/runtime.rs::McpStartupPolicy::Eager` + `connection_manager.rs::join_set.spawn`（主 agent 预连）；Continue `core/context/mcp/MCPManagerSingleton.ts::setConnections`（`void refreshConnections()`）；VS Code `mcpService.ts::autostart` / `chatServiceImpl.ts` 是可选、会等待的反例。 | **第一性原理**：模型只能调用已在本轮 ToolSurface 中的工具；若懒连到首次调用，工具尚未可见，模型没有触发连接的入口；若同步等待，慢 server/`npx` 下载把可用的首轮对话拖住。故选“后台预连 + Ready 才登记 + 每轮自动刷新”。断线必须撤销已登记工具，否则工具仍可见却必失败。Codex 的 root agent Eager、Continue IDE 异步预连支持此选择；VS Code 的可选 autostart 可在未来作为 `initial_tool_grace_ms` 增强，**本期默认 0、不延迟首轮**。 | 未入选：(a) 同步等待所有 server 后才允许首轮——拒因：任一慢/坏 server 把主交互卡住；(b) 首次工具调用才连接——拒因：未连接工具不在 ToolSurface，模型无从调用，且首个调用等待不可预测；(c) Ready 后不断线撤销——拒因：向模型暴露必失败的陈旧工具。 | 开机时悄悄并行去连；连好才让模型看见工具，断了就撤下。聊天和服务先可用，绝不等慢 MCP。 |
| **R-C1 「添加连接器」命令（两模式对等）** | 用户/GUI 怎么新增一个连接器？ | **采用** Chat 斜杠 `/connector add` + serve `add_connector`，两模式**对等**、都落到同一份配置写盘逻辑。 | Tomcat：Chat 仿 [cmd_model.rs](../../../src/api/chat/commands/cmd_model.rs)（模型 add 的现成范式）+ [commands/parse.rs](../../../src/api/chat/commands/parse.rs)；serve 仿 [control.rs](../../../src/api/serve/control.rs) capabilities 登记 + [commands.rs](../../../src/api/serve/commands.rs) 处理 + [types.rs](../../../src/api/serve/types.rs)::`ServeCommand` 变体；外部：`hermes-agent hermes_cli/mcp_config.py`（`mcp add` CLI）、`vscode` mcp.json add 流。 | 写盘统一由 `config.rs::upsert_global_server()` 完成，再 `ConnectorRegistry::reload()` 读取并重连；Chat 与 serve 只是两个门面。理由：Tomcat 既有 model-admin 已是「CLI + serve 双门面 + 单写盘」的成熟范式。 | 未入选：只做 serve 命令、Chat 不管。拒因：Chat（CLI）用户也要能加连接器，双模式对等是产品要求。 | 加连接器：命令行敲 `/connector add`、GUI 发 `add_connector`，落到同一份 `mcp.json`。 |
| **R-C2 「查看/管理连接器」命令（两模式对等）** | 怎么看/信任/删/测连接器与其工具？ | **采用** Chat `/connector list\|trust\|deny\|remove\|test\|reload\|tools` + serve `list_connectors`/`list_connector_tools`/`set_connector_trust`/`remove_connector`/`test_connector`/`reload_connector`/`set_connector_tool_filter`，两模式对等。 | Tomcat：对齐既有 `list_models`/`set_provider_key`/`remove_model` 与 [cmd_model.rs](../../../src/api/chat/commands/cmd_model.rs)；trust 落 `~/.tomcat/connector-trust.json`（R7）；外部：`hermes-agent`（`mcp list/test`）、`cline` McpHub 管理面。 | `list_connectors` 只返回轻量状态摘要 + 信任态，避免每次列表带上几百个工具；展开单个 server 时才调 `list_connector_tools(name)`。Chat `/connector tools <name>` 与 serve 都复用 `McpManager::tool_defs`，因此看到同一份经 `toolFilter` 裁剪的事实。 | 未入选：把管理揉进 add 命令，或把完整工具塞进每次 list。拒因：list/trust/remove/test 是不同动作；全量 tools 会让摘要列表随 server 数量膨胀。 | 看/信任/删/测连接器先看摘要；想看某个 server 的工具再单独展开，列表不会越来越重。 |
| **R12 工具目录缓存策略** | 每次启动都现取 `tools/list`，还是跨重启持久缓存工具列表？ | **本期不做跨重启持久缓存**：每次连接**现取** `tools/list`，仅**会话内内存**缓存（`McpManager` 持有、Ready 后注册进 `ToolRegistry`）；不落盘、不引 TTL 复杂化。**持久缓存 + `tools/list_changed` 实时刷新列为 fast-follow**，届时**复用 R7 的命令指纹当缓存 nonce**（命令变即失效）。 | 外部：**Codex `codex-mcp/src/tool_catalog_cache.rs`（30min 进程内 TTL、不落盘；连接仍现取 tools/list，缓存只服务懒启动/宽限）**、**Continue `MCPConnection.ts:139-143,328-332`（连接即清空重取、无缓存）**、**Cline `McpHub`（会话内 + SDK 5s TTL、debounced list_changed 刷新、不落盘）**、**VS Code `mcpServer.ts::McpServerMetadataCache`（唯一落盘：`storageKey='mcpToolCache'`，按启动配置 nonce 失效——为"启动前显示工具"）**；Tomcat：`McpManager` 会话内工具目录（R9 Ready 后注册） | 第一性原理——缓存的两大动机（① 启动延迟/懒启动、② 未连即显示工具）在我们的 R9 下都**不成立**：R9 是**后台预连、非阻塞**，工具几秒内 Ready 且经 `list_tools` 自动上工具面，无用户可感延迟；又**未处理 `tools/list_changed`**（§9 非目标），落盘缓存只放大陈旧风险。3/4 家（Codex/Continue/Cline）都不落盘、连接现取，是业界常态；**现取 = 永远新鲜、零失效逻辑**。将来若做 lazy-start（Codex 式）或"未连即显示工具"（VS Code 式），再加持久缓存，nonce 直接复用 R7 已算的命令指纹（VS Code 正是用启动配置哈希当 nonce），零新机制。 | 未入选：(a) 跨重启落盘缓存（VS Code 式）——拒因：本期无 lazy-start、无 list_changed 刷新，缓存只增陈旧与失效复杂度，收益（省几秒后台连接）对非阻塞启动可忽略；(b) 无任何缓存、每次用工具都 `tools/list`——拒因：没必要，会话内内存缓存足矣（工具已注册进 registry）。 | 每次连上现问一遍「你有哪些工具」，只在这次会话内存里记着、不写文件。反正我们后台悄悄连、几秒就好，缓存省不了多少还容易记着过时工具。真要做「不连也显示工具」再加持久缓存，且直接用信任那套命令指纹当失效标记，不另造轮子。 |
| **R11 命令面形态（本期 CLI + 配置文件，无 webview）与 Cursor 能力对标** | 本期用什么承载「管理连接器」，要对齐 Cursor GUI 的哪些能力？ | **采用** 本期仅 **CLI 斜杠命令 + 直接编辑 `mcp.json`**（**无 webview GUI**；serve capabilities 同步就绪，GUI 作 fast-follow）。CLI 对标 Cursor GUI 补齐三项：`list` **富状态**（来源 User/Workspace、连接态 `●Connected`/`Failed`/`Blocked`、工具数+资源数、信任态）、`reload`（重读 `mcp.json`+重连，对标 Cursor 的 **Reload**）、`tools <server>`（查看/按 `toolFilter` include/exclude 裁剪要暴露的工具，对标 Cursor 的 **per-tool 开关**，但用**声明式配置过滤**而非独立开关存储）；`add`/`remove`/`trust`/`deny`/`test` 见 R-C1/R-C2 | 外部：**Cursor MCP 设置（用户截图）**：连接态绿点 +「33 tools, 1 resource enabled」+ 来源 User/Workspace + per-tool 开关 + Reload 按钮 + New MCP Server；`openclaw src/agents/mcp-content.ts` 的 tool include/exclude 过滤；Tomcat：`api/chat/commands/cmd_connector.rs`、`api/serve/control.rs` capabilities | **第一性原理**：GUI 的每个可视能力背后都是一次「读状态 / 改配置 / 重连」——CLI 只要覆盖这三类语义就与 GUI 等价、还能进 CI。Cursor 的 per-tool 开关本质是「裁剪工具面」（33 个 playwright 工具太吵），用 `mcp.json` 里的 `toolFilter` 声明式实现比额外维护一份 per-tool 开关状态更简洁、可复制、可审计（config-as-truth）。`reload` 对标 Reload：改完配置不必重启会话。webview 推迟不减能力：serve 已备 capabilities，GUI 只是再套一层门面。 | 未入选：(a) 本期就做 webview——拒因：用户明确本期不做 GUI，先 CLI 落地价值最高。(b) 学 Cursor 的 per-tool **有状态开关存储**——拒因：多一份状态源、与「config 即事实」冲突，`tool_filter` 声明式更优。 | 本期只有命令行 + 直接改 `mcp.json`（不做界面）；但命令行要能干 Cursor 界面能干的事：看状态、重连、裁剪工具；界面以后再补，能力不打折。 |

### 3.2 实施点（已闭环）

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| **P0 连接器框架** | `Connector` trait + `ConnectorType {Mcp,Cli,A2a}` + `ConnectorRegistry`（按配置源构造）+ `CompositeToolExecutor`（按 `plugin_id` 路由）；`[connector] enabled` 总开关（MCP 的 `mcp.json` schema 见 P1/R10） | `core/connector/mod.rs`（trait/enum/registry/`CompositeToolExecutor`）、`infra/config/types/connector.rs`（`ConnectorConfig` 总开关）、`infra/config/types/mod.rs`（挂 `AppConfig.connector`）；`Cli`/`A2a` 仅枚举预留 | `core::connector::tests::{composite_routes_by_plugin_id,connector_disabled_master_switch_does_not_connect_or_register_tools}` | 先把「连接器插座 + 总开关」搭好，MCP 插上去。 |
| **P1 MCP 配置与物化（`mcp.json`/`mcpServers`，R6/R10）** | `McpServerConfig`（`command`+`args` 必填、`env`/`cwd`/`trusted`/`integrity`/`startupTimeoutMs`/`callTimeoutMs`/`toolFilter` 可选）、`mcpServers` schema、user+workspace 分层覆盖、`{work_dir}/cache/playwright` 浏览器缓存目录、init 物化含 `@playwright/mcp` 的默认 | `core/connector/mcp/config.rs`（`McpServerConfig` + `serde(default)` 解析全局 `mcp.json` + 项目级 `.tomcat/mcp.json` 覆盖）、`core/connector/mcp/builtin.rs`（`materialize_default_mcp_json()`，仿 `skill/builtin.rs`） | `core::connector::mcp::config::tests::{minimal_cursor_style_server_uses_optional_field_defaults,project_server_overrides_global_server_with_same_name}`；`builtin::tests::materializes_cursor_style_pinned_playwright_config_idempotently` | 用生态标准 `mcp.json`：只填 command/args 就能加 server，自带 playwright，浏览器共用缓存。 |
| **P2 传输与管理器** | `rmcp` stdio 传输封装、`McpManager`（连接/`tools/list`/`call_tool`）、连接状态机、启动期退避/调用超时/不 replay；发 `Ready{tools}` / `NotReady` 生命周期事件 | `core/connector/mcp/transport.rs`、`core/connector/mcp/manager.rs`（`ServerState`/`ServerStatus`/`McpToolDef`/`ServerLifecycleEvent`）；`Cargo.toml` 加 `rmcp` | `manager::tests::{fake_stdio_server_lists_and_calls_tools,call_tool_timeout_returns_error_and_marks_server_disconnected,transport_drop_marks_disconnected_without_replaying_call,reconnect_refetches_tools_without_a_persistent_cache}` | 把 server 拉起来、拿到工具、能调用；状态变化通知协调器挂/撤工具。 |
| **P3 信任模型（Cursor 对齐，R7）** | `TrustStore`（`connector-trust.json`）；**默认 config 即信任 + 命令指纹绑定（防 MCPoison）**：全局/curated 无感、项目来源首见或命令变化才确认；用户浮动版本告警不拦；可选强档 integrity/SHA256；`/connector trust\|deny\|list` | `core/connector/mcp/trust.rs`；`api/chat/commands/cmd_connector.rs`（斜杠命令/确认弹窗）；serve 侧 `set_connector_trust` | `core::connector::mcp::trust::tests::{global_config_server_auto_trusted_no_prompt,curated_trusted_by_default,project_server_first_seen_requires_confirm,command_fingerprint_change_requires_reconfirm,user_floating_version_warns_not_blocks,optional_strong_tier_integrity_mismatch_blocks}` | 你自己配的直接用；只在项目来源/命令被改时确认一次。 |
| **P4 工具注册与路由（Option B）** | 把 MCP 工具 `register_tool` 进 `DefaultToolRegistry`（`plugin_id="mcp:{server}"`）；`CompositeToolExecutor` 按 `plugin_id` 路由到 `McpManager`；命名映射；`list_tools` 自动上工具面 | `core/connector/mod.rs::CompositeToolExecutor`（新增）、`core/connector/mcp/executor.rs::McpToolExecutor`（新增，impl `ToolExecutor` 包 `McpManager`）、`core/connector/mcp/naming.rs`、`api/chat/context.rs`（连接后 `register_tool` + 用 `CompositeToolExecutor` 建 `DefaultToolRegistry`） | `core::connector::tests::mcp_tools_appear_in_surface_via_list_tools`、`core::connector::mcp::executor::tests::mcp_call_routes_to_mcp_manager`、`core::connector::mcp::naming::tests::long_name_keeps_readable_head_plus_short_hash` | MCP 工具像插件一样登记进注册表、按 plugin_id 路由，自动出现在工具菜单。 |
| **P5 出口媒体转换器 + 图片回流（Option B 关键）** | `tool_dispatcher` registry 分支用 `extract_tool_result_media()`：text→`model_text`、`image` 块→`follow_up_parts`（复用共享图片原语）；**对纯文本插件 no-op**；非 vision 降级 | `core/agent_loop/tool_dispatcher.rs::extract_tool_result_media`（registry 分支 Ok 臂）、`types.rs::ChatMessageContentPart` / `openai_files.rs` / `multimodal.rs::degrade_unsupported_multimodal` | `tool_dispatcher::tool_result_media_tests::{mcp_image_block_becomes_input_image,text_only_plugin_result_preserves_prior_empty_follow_up_parts_behavior,text_block_becomes_model_text,unknown_block_becomes_a_text_summary}` | 注册表出口加个小转换器：MCP 图片翻成 InputImage，纯文本插件完全不受影响。 |
| **P6 启动装配与工具生命周期（R9）** | `ConnectorRegistry`（含 MCP 连接器）注入 `GlobalServices`；`spawn_connect_all()` 对已信任 server **后台并行预连**（非阻塞，首轮/serve handshake 不 await）；事件协调器在 Ready `register_tool`、NotReady `unregister_plugin_tools`，只持 `Weak<dyn ToolRegistry>` | `api/chat/context.rs`（按总开关构造并注入）、`api/chat/run_loop/mod.rs`（每轮入口懒触发）、`api/chat/session_runtime.rs::GlobalServices`（持有 `connector_registry`） | `context::tests::connector_registry_constructed_when_enabled`、`connector::tests::{startup_connect_is_non_blocking,ready_mcp_tools_register_into_the_shared_tool_registry}`、E2E `E2E-MCP-001` | 开机悄悄并行去连、不等它；连好才挂工具，断了就撤下，避免菜单里留必失败的工具。 |
| **P7 命令面（本期 CLI + serve，无 webview；R-C1/R-C2/R11）** | 「添加」`/connector add` + serve `add_connector`；「查看/管理」`/connector list\|trust\|deny\|remove\|test` + serve 对等；`list` 富状态（来源/连接态/工具数+资源数/信任态）、`reload`、`tools <server>`（按 `toolFilter` 裁剪）与 serve `list_connector_tools` 对等 | `api/chat/commands/cmd_connector.rs` + `commands/parse.rs`（Chat）、`api/serve/types.rs::ServeCommand` + `commands.rs` + `control.rs`；写盘统一走 `config.rs::{upsert_global_server,remove_global_server,set_global_tool_filter}` 后 reload | `cmd_connector_test::cmd_connector_add_and_list_round_trips_filtered_tools`、`control_test::serve_connector_commands_keep_list_light_and_tools_on_demand` | 本期命令行 + 直接改 `mcp.json`（不做界面）；命令能干 Cursor 界面能干的事：看状态、重连、裁剪工具，落到同一份配置。 |

> **已实现：**
> - **P8 MCP 远程传输**：`core/connector/mcp/transport.rs::HttpTransport` 使用 rmcp Streamable HTTP client（2026-07-28 单端点）；`McpServerConfig.url` 与 `headers` 支持无认证、Bearer/custom headers。
> - **P8 OAuth**：`core/connector/mcp/oauth.rs` 按 401 challenge → protected-resource metadata → RFC 8414/OIDC metadata 发现端点，支持预注册 client_id、动态注册、PKCE、token refresh 与 `connector-oauth.json` 安全存储；`oauth_callback.rs` 使用动态 loopback 端口并校验 state。
> - **P9 其它连接器类型**：CLI/A2A 当前只保留 `ConnectorType::{Cli,A2a}` 枚举值；真正落地时再定义其目录、协议和工具路由。

#### 3.2.1 P5 图片回流细节（复用而非新建）

> 专业：`tool_dispatcher` 的 `extract_tool_result_media()` 收到注册表返回的 JSON（MCP 的 `CallToolResult.content`），遍历 `content`：`text` 拼进 `model_text`；`image { data, mimeType }` 走与 `read.rs` 完全相同的两分支——`upload_decision_by_size(bytes)` 决定内联还是上传。**对不含 image 块的结果（今天的插件）是 no-op**。
> 说人话：这一步是整个模块存在的理由，且**一行新图片管道都不用写**——read 工具早就把「字节→模型能看的图」这条路铺好了，MCP 只是把图片块塞进同一条路；纯文本插件没图片块，转换器直接跳过。

```text
CallToolResult.content[i]
   ├─ TextContent            → model_text.push_str(text)
   ├─ ImageContent{data,mime}
   │     decode base64 → bytes
   │     upload_decision_by_size(bytes):
   │        InlinePreferred → ChatMessageContentPart::image_base64_data(mime, b64)
   │        否则            → 落临时文件 → OpenAiFilesRuntime::resolve_or_upload_path
   │                           → ChatMessageContentPart::image_file_id(id)
   │     → follow_up_parts.push(part)
   └─ Resource/Audio/未知    → 文本摘要 append 到 model_text（不伪造图片）
   ⇒ ToolExecOutcome{ model_text: text + "（截图见下一条消息）", follow_up_parts }
```

---

## 4. 协议（配置 Schema + 工具名映射 + 结果转换）

单一事实源：`core/connector/mcp/config.rs::McpServerConfig`（配置）、`core/connector/mcp/naming.rs`（工具名）、`core/agent_loop/tool_dispatcher.rs::extract_tool_result_media`（结果→模型，含图片回流）。

### 4.1 `mcp.json` 配置（`mcpServers`，对齐生态标准）

文件位置：`~/.tomcat/mcp.json`（全局）；`<workspace>/.tomcat/mcp.json`（项目级，覆盖同名 server）。**形状与 Cursor / Claude Desktop 的 `mcpServers` 完全一致**——从生态复制的 MCP 片段零翻译即用（R6/R10）。顶层是 `mcpServers`：`server 名 → 配置对象`。每个 server **只有 `command` + `args` 必填**（server 名 = 键），其余全部可选；可选字段是 Tomcat 对 Cursor 形状的**超集**（安全/超时/工具裁剪），一段裸 Cursor 片段照样能跑。

| 字段 | JSON 类型 | 必填 | 默认值 | 说明 | 说人话 |
|------|-----------|------|--------|------|--------|
| `mcpServers` | object | 是 | — | 顶层：`server 名 → 配置` 映射 | 所有 MCP server 一张表。 |
| `<name>`（键） | string | 是 | — | server 标识，进 `mcp__{name}__*` | server 名就是这个键。 |
| `command` | string | **是** | — | 子进程可执行文件（`npx`/`node`/…） | 用什么命令启动。 |
| `args` | string[] | **是** | — | 命令行参数；npm 型建议钉死精确版本，`@latest`/`@next`/无版本会告警但不阻止连接（R7） | 启动参数，版本写死更稳；没写死会提醒。 |
| `env` | object | 否 | `{}` | 注入子进程的环境变量（`env_clear` 后仅注入这些 + 最小白名单 `PATH`/`HOME`；值**不经 shell 展开**） | 环境变量（PATH 由白名单兜底）。 |
| `cwd` | string | 否 | 当前 workspace | 子进程工作目录 | 在哪个目录起。 |
| `trusted` | bool | 否 | `false` | 声明式信任（供 CI/非交互）；**仍过「启动方式指纹+内容身份」双重校验**；curated 由代码 allowlist 默认信任（R7） | 是否已授权（仍要过身份校验）。 |
| `integrity` | string | 否（安全敏感部署） | — | **本地预装入口文件**的 `sha256:<hex>` / `sha256-<base64>` / `sha512-<base64>`；spawn 前逐字节比对。纯 `npx` 没有可校验的本地入口，不能填这一项假装已验（R7） | 要最严就记住本地启动入口的内容指纹。 |
| `startupTimeoutMs` | number | 否 | `30000` | spawn+initialize+首次 list 总超时 | 起不来多久算失败。 |
| `callTimeoutMs` | number | 否 | `120000` | 单次 `tools/call` 超时 | 一次调用最多等多久。 |
| `toolFilter` | `{include?,exclude?}` | 否 | 全部 | glob 裁剪暴露给模型的工具子集（R11 的声明式 per-tool 控制） | 只暴露/排除部分工具。 |

> **HTTP/OAuth 已实现**：`core/connector/mcp/transport.rs::HttpTransport` 使用 rmcp 的 Streamable HTTP client，支持无认证、Bearer 与 custom headers；`core/connector/mcp/oauth.rs` 实现 protected-resource → authorization-server/OIDC metadata discovery、动态 client registration、PKCE、token refresh 与安全文件存储；`oauth_callback.rs` 绑定 `127.0.0.1:0` 并校验 state。

主配置 `tomcat.config.toml` 侧（`AppConfig.connector`）：

| 字段 | 类型 | 默认 | 说明 | 说人话 |
|------|------|------|------|--------|
| `[connector] enabled` | bool | `true` | 连接器模块总开关；关则完全不发现/连接 | 一个总闸，默认开；没有 mcp.json server 时不会拉起任何进程。 |
| `[connector] disabled` | string[] | `[]` | 按 name 禁用某些连接器 | 点名停用。 |

**默认放行 + 命令绑定（R7 落地，Cursor 对齐）**：默认「config 即信任」——**没有 per-server 信任提示**。为堵 MCPoison，信任**绑定到启动命令指纹**（`command/args/env/cwd` 的哈希）而非 server 名：

- **全局 `~/.tomcat/mcp.json` / curated / 已确认的项目 server**：命令指纹匹配即**直接 spawn，无感**。
- **项目 `<workspace>/.tomcat/mcp.json` 首次出现，或任意已知 server 的命令指纹变化**：spawn 前**一次确认**（附命令 diff），确认后把指纹记入 `connector-trust.json`。这一步专防「共享仓库同名换命令」（CVE-2025-54136 MCPoison）。

**版本与内容强度（不给普通用户添堵）**：`@latest`/`@next`/无版本对**用户 server 只告警、不拦**（Cursor 亦不拦）；**curated 内置仍钉死精确版本**（我们作者、零用户摩擦）。内容哈希是**可选强档**，非默认必需：

- **【默认·命令绑定】**：信任绑到 `command+args+env+cwd` 指纹（同 Cursor 1.3+）；命令没变就放行，命令变了就再确认。够挡 MCPoison。
- **【可选强档·预装 + integrity/SHA256】**：安全敏感者可 `npm ci`（带 `package-lock.json`）预装、`command` 改 `node <dir>/.../mcp` 直启入口，填写该**本地入口文件**的 `sha256:<hex>` / `sha256-<base64>` / `sha512-<base64>`，spawn 前逐字节校验。`package-lock.json` 可让安装可复现，但其中 tarball 的 integrity **不能直接冒充入口文件哈希**。**诚实边界**：纯 `npx` 无法 spawn 前逐字节校验（要自己重放 npm 解析），强档才有此保证。

**三个信任来源，同一道命令指纹闸（避免"两份真相"）**：放行一个 server，「我批准」可来自三处——① init 物化到**全局** `mcp.json` 的 curated 条目（因此走全局默认信任）；② 用户 `/connector trust <server>`（或未来确认弹窗）写入 `~/.tomcat/connector-trust.json`；③ 项目 `mcp.json` 里 `"trusted": true`（声明式，供 CI/非交互）。**三者都只表达"我批准"，最终一律经同一道命令指纹匹配放行**；指纹变了（命令被换）就需重新确认。`connector-trust.json` 是确认记录，`"trusted": true` 是配置声明，两者不冲突。

**Playwright 浏览器目录（与 Phase 1 无头截图共用）**：`env.PLAYWRIGHT_BROWSERS_PATH` 只指定 Chromium 二进制的目录，**不指定 npm JavaScript 包的 `node_modules` 目录**。Tomcat 统一把它解析为 `{work_dir}/cache/playwright`（默认即 `~/.tomcat/cache/playwright`）；Phase 1 的 `verify/scripts/bootstrap.mjs` 往此处安装 Chromium，Phase 2 的 `@playwright/mcp` 子进程从此处寻找 Chromium。这样两种验收方式共用一份数百 MB 的浏览器，而 Tomcat 升级更新 skill 文件也不会重下浏览器。

**macOS 13 兼容分支（两条链必须一起接）**：新版 Playwright 不再提供 macOS 13 的 bundled Chromium。Phase 1 的 `bootstrap.mjs` 在该**特定平台不支持**错误下查找系统 Chrome，并把绝对路径写成 `{work_dir}/cache/playwright/system-browser.json`；`shot.mjs` 读取该标记，用 `executablePath` 启动 Chrome。`@playwright/mcp` 是**另一条独立 Node 子进程**，不会读这个标记，所以 `core/connector/mcp/transport.rs` 对 curated `playwright` 连接器读取标记并注入 MCP 官方环境变量 `PLAYWRIGHT_MCP_EXECUTABLE_PATH`（用户显式配置同名变量优先）。因此 Phase 1 与 Phase 2 都会落到同一套系统 Chrome；其它平台仍优先使用受管 Chromium。

> `env` 的值不经过 shell 展开：不能写 `~/.tomcat/cache/playwright` 或 `$HOME/...` 期待子进程替你展开。`core/connector/mcp/builtin.rs` 物化 curated `mcp.json` 时，必须用 `get_work_dir(cfg)` 写入**规范化后的绝对路径**；自定义 server 也由 `core/connector/mcp/config.rs` 在加载期规范化相对路径。

调用样例（`~/.tomcat/mcp.json`，init 物化的 curated 默认；形状同 Cursor/Claude，`mcp.json` 为纯 JSON、无注释）：

```json
{
  "mcpServers": {
    "playwright": {
      "command": "npx",
      "args": ["-y", "@playwright/mcp@0.0.79", "--headless"],
      "env": { "PLAYWRIGHT_BROWSERS_PATH": "/absolute/path/to/work_dir/cache/playwright" },
      "startupTimeoutMs": 60000
    }
  }
}
```

> 关于上例：`args` 里 `@0.0.79` 是**钉死精确版本**，`--headless` 保证 MCP 浏览器不弹窗口；curated `playwright` 的**默认信任来自代码 allowlist（R7 来源①）**，故物化的最小条目**连 `trusted` 都不必写**。`/absolute/path/to/work_dir/...` 是占位——init 写入 `get_work_dir(cfg)` 的绝对值。

用户加自己的 server，最少只要三样（**与 Cursor 完全一致，可从任意 MCP 文档整段粘贴**）：

```json
{
  "mcpServers": {
    "my-server": { "command": "npx", "args": ["-y", "some-mcp@1.2.3"] }
  }
}
```

> **【强档·更严格部署】** 把 npm 型换成「预装 + 直接启动入口 + `integrity`/SHA256」，spawn 前逐字节校验（R7）：

```json
{
  "mcpServers": {
    "playwright": {
      "command": "node",
      "args": ["/absolute/path/to/work_dir/mcp/playwright/node_modules/@playwright/mcp/cli.js"],
      "env": { "PLAYWRIGHT_BROWSERS_PATH": "/absolute/path/to/work_dir/cache/playwright" },
      "integrity": "sha512-...",
      "trusted": true
    }
  }
}
```

### 4.2 工具名映射

```text
常态（99% 情况，名字没超限）—— 模型看到的是【完整可读名】
  server="playwright", tool="browser_take_screenshot"
    ──►  "mcp__playwright__browser_take_screenshot"   （40 字符，未超 64，原样保留）
  调用：用模型名查 McpToolDef 对照表 → (playwright, browser_take_screenshot) → tools/call

罕见兜底（仅当拼出来 > provider 名长上限，如 OpenAI 64 字符 / 或含非法字符）
  借 cline name-transform.ts / codex tools.rs 的做法：
  【保留可读头 + 只加短哈希后缀】，不是"整名变哈希"
  server="playwright", tool="navigate_and_wait_for_networkidle_then_capture_full_screenshot"
    ──►  "mcp__playwright__navigate_and_wait_for_networkidle_the_a1b2c3d4"
                         └──────── 尽量保留原名（截断） ───────┘ └短哈希┘
  → 头部仍可读，短哈希只为"不同长名不撞车"
```

单一事实源：`core/connector/mcp/naming.rs::to_model_name`；原始 `(server, raw_tool)` 存于 `McpToolDef` 对照表，模型名仅用于暴露与查表分派。

> **为什么必须"另存原始名"**：常态下显示名含完整工具名，本可直接拆前缀反解；但**兜底路径截断/哈希后，显示名已丢失完整原名、且哈希单向不可逆**，无法从显示名反推真名。所以统一在 `McpToolDef` 存一张「显示名 → (server, 原始 tool)」对照表，`tools/call` 一律查表取原始名，不依赖反解析。
>
> **可读性说明（回应"哈希伤可读性"）**：短哈希**只在超 provider 名长上限时**出现，且**保留可读头**（对齐 `cline/name-transform.ts:26-34` 的 `${baseName}_${hash}`、`codex/codex-rs/codex-mcp/src/tools.rs:265-285` 的 `truncate_name + append_hash_suffix`）。我们的首要目标 `@playwright/mcp` 工具名都很短（`browser_click` 等），**永不触发兜底**——模型始终看到完整可读名。这是一条 API 正确性的安全网（OpenAI 硬限 64 字符），不是常态。

> **`mcp__` 前缀是命名约定（Option B 下不参与路由）**：路由靠注册表 `plugin_id`（`mcp:{server}`）而非字符串前缀，故不存在「抢注前缀→误路由」；前缀只用于给模型一个带命名空间的可读名、并降低与 builtin/插件撞名的概率。真正的撞名由 `DefaultToolRegistry::register_tool_local` 的 builtin/跨-plugin 重名校验兜底。

### 4.3 结果转换（MCP CallToolResult → 模型消息）

见 §3.2.1 的 ASCII。要点：**text→工具消息正文；image→下一条 user 消息的 `InputImage`；其他类型→文本摘要，不伪造图片**。这是与 continue（丢图）/pi_agent_rust（图片入 details）的关键区别。

> **`@playwright/mcp` 的截图特例（由受忽略的真实 LLM E2E 覆盖）**：它只有在 `browser_take_screenshot` **省略 `filename` 且不传 `fullPage=true`** 时，才把 PNG 作为 MCP `image` block 返回；指定文件名或全页截图会只落盘、返回 Markdown 文件链接，模型看不到图。故 `assets/skills/verify/SKILL.md` 与 MCP 工具描述都明确该约束；命名/全页截图只用于人工保留文件，不能充当视觉回流证据。

### 4.4 MCP 传输（stdio 与 Streamable HTTP；OAuth 为 HTTP 的认证层）

MCP 规范（2026-07-28）定义两种一等传输；本模块把它们做成 `McpTransport` trait 后面的可插拔实现：

```text
传输              形态                                   本期    落点
──────────────    ─────────────────────────────────     ────    ──────────────────────────────────
stdio             client 把 server 当子进程,               ✅本期   core/connector/mcp/transport.rs::StdioTransport
                  stdin/stdout 上跑 newline-JSON-RPC             （经 rmcp；@playwright/mcp 默认；零网络零 OAuth）
Streamable HTTP   单 POST 端点(/mcp)，回 JSON 或            ✅已实现  core/connector/mcp/transport.rs::HttpTransport（rmcp；远程/托管 server）
                  request-scoped SSE 流                          @playwright/mcp 加 --port 8931 即此模式
OAuth             认证远程 server 的授权流                  ✅已实现  core/connector/mcp/oauth.rs + oauth_callback.rs；标准 discovery/PKCE/refresh
HTTP+SSE(旧)      两端点                                   不做    已废弃(2025-03)，不实现
```

- **纠正误解**：stdio 不是"简化版"，是规范里 "All versions" 的一等且本地最常见传输；"MCP 都是 HTTP" 指的是远程/托管场景。本期视觉验收（本地浏览器）用 stdio 完全够。
- **接缝**：`McpTransport` trait 后面已有 `HttpTransport`；`rmcp` 同一 crate 提供 Streamable HTTP client，OAuth 增量集中在标准 discovery、PKCE、loopback callback、token store 与 `url`/`headers` 配置。
- **证据**：MCP 规范 `modelcontextprotocol.io/specification/2026-07-28/basic/transports`（stdio + Streamable HTTP 两种一等传输，HTTP+SSE 已废弃）；`@playwright/mcp` 默认 stdio、`--port` 启 HTTP（`github.com/microsoft/playwright-mcp`）。

---

## 5. 文件职责总览（One-Glance Map）

> 专业：新增 `core/connector/` 模块——框架层（`mod.rs` trait/registry/`CompositeToolExecutor`）+ MCP 连接器 `core/connector/mcp/*`（含 `executor.rs::McpToolExecutor`）+ 命令面（Chat `cmd_connector.rs` + serve `*_connector` 三件套）+ `types/connector.rs`。CLI/A2A 只在 `ConnectorType` 枚举中预留，**没有**空目录。**Option B（§3.1 R4）**：MCP 工具进 `ToolRegistry`、执行经 `CompositeToolExecutor` 路由、图片经 `tool_dispatcher` 出口转换器回流；`run_loop` 额外增加一行懒触发启动，其余工具面逻辑复用。下图自顶向下即一次调用链路，`←新增 / 改 / 复用 / 预留` 四态标注。
> 说人话：带 `←新增` 的是全新文件，`改` 是往既有文件插一小段，`复用` 是直接使用已有能力，`预留` 是本期只有类型名、没有空实现。

```text
框架层（连接器抽象）
  core/connector/mod.rs                ←新增  Connector trait, ConnectorType{Mcp,Cli,A2a}, ConnectorRegistry(按配置源构造), CompositeToolExecutor(按 plugin_id 路由)
  infra/config/types/connector.rs      ←新增  ConnectorConfig{enabled,disabled}（主配置总开关）
  infra/config/types/mod.rs            改      AppConfig.connector: ConnectorConfig
  ConnectorType::{Cli,A2a}                   ←预留  未来连接器类型（本期无目录/实现）
        │
        ▼
MCP 连接器（本期实现）
  Cargo.toml                           改      + rmcp 依赖
  core/connector/mcp/config.rs                   ←新增  McpServerConfig(command/args 必填, env/cwd/trusted/integrity/*TimeoutMs/toolFilter 可选; serde default); parse ~/.tomcat/mcp.json(mcpServers) + 项目级覆盖
  core/connector/mcp/builtin.rs                  ←新增  materialize_default_mcp_json()(curated playwright; 仿 skill/builtin.rs)
  core/connector/mcp/transport.rs                ←新增  McpTransport trait + StdioTransport（封装 rmcp Transport<RoleClient>；子进程收尾交 rmcp）；http 预留(P8)
  core/connector/mcp/trust.rs                    ←新增  TrustStore(~/.tomcat/connector-trust.json); command_fingerprint(); config 即信任 + 项目首见/命令变化确认；可选 integrity(SHA256/SHA512 本地预装入口)
  core/connector/mcp/manager.rs                  ←新增  McpManager::{new,connect_server,tool_defs,call_tool,reconnect_server}; ServerState/Status/McpToolDef；状态机/退避/不replay
  core/connector/mcp/naming.rs                   ←新增  to_model_name（mcp__server__tool + 哈希；反解查 McpToolDef）
        │  McpManager: Option<Arc<..>>
        ▼
启动装配 + 工具注册（Option B）
  core/connector/mod.rs::CompositeToolExecutor ←新增  按 plugin_id 路由：mcp:* → McpToolExecutor、其余 → PluginToolExecutor
  core/connector/mcp/executor.rs       ←新增  McpToolExecutor(impl ToolExecutor)：调 McpManager::call_tool，返回 MCP content JSON
  api/chat/context.rs                  改      按总开关构造 ConnectorRegistry，并用 CompositeToolExecutor 建 DefaultToolRegistry
  api/chat/run_loop/mod.rs             改      每轮入口懒触发后台 MCP 启动；Ready 后由 ConnectorRegistry 生命周期协调器 register_tool
  api/chat/session_runtime.rs          改      GlobalServices.connector_registry: Option<Arc<ConnectorRegistry>>（供生命周期/命令面）
        │
        ▼
工具面（每轮，无需为 MCP 改动）
  api/chat/run_loop/mod.rs             复用  observe_tool_surface() → list_tools() 自动含已注册的 MCP 工具
  core/llm/system_prompt.rs            复用  ToolSurface::from_plugin_tools 一并纳入 MCP（不特判）
        │  ChatRequest.tools 含 mcp__*
        ▼
分派 + 执行（统一走注册表 + 出口转换器）
  core/agent_loop/tool_dispatcher.rs   改      registry 分支使用 extract_tool_result_media()
                                              （text→model_text、image 块→follow_up_parts；对纯文本插件 no-op）
        │  复用（不改）：
  core/agent_loop/tool_exec/branches/read.rs   复用  image_base64_data / image_file_id 路径（图片回流）
  core/llm/types.rs                            复用  ChatMessageContentPart::{image_base64_data,image_upload}
  core/llm/openai_files.rs                     复用  upload_decision_by_size / OpenAiFilesRuntime
  core/llm/multimodal.rs                       复用  degrade_unsupported_multimodal（非 vision 降级）
        │
        ▼
命令面（Chat + serve 对等，R-C1/R-C2）
  api/chat/commands/cmd_connector.rs   ←新增  /connector add|list|trust|deny|remove|test|reload|tools（无 webview，R11）
  api/chat/commands/parse.rs           改      注册 /connector 解析
  api/serve/types.rs                   改      ServeCommand: Add/List/ListTools/SetTrust/Remove/Test/Reload/Filter Connector 变体
  api/serve/commands.rs                改      连接器命令处理（写盘走 config.rs 的 upsert/remove/filter，再 reload）
  api/serve/control.rs                 改      capabilities 登记 add_connector/list_connectors/list_connector_tools/...
配套测试
  core/connector/tests/                ←新增  registry(按 plugin_id 路由)/config(mcp.json 最小字段)/commands 单测
  core/connector/mcp/tests/            ←新增  transport/manager/trust/naming 单测
  tests/connector_mcp_tests.rs         ←新增  假 stdio MCP server 集成测试
```

> 阅读顺序（说人话）：框架层定义「有哪些连接器、怎么构造」→ MCP 连接器（rmcp + trust）把 server 拉起来、把工具登记进注册表 → 工具菜单经 `list_tools` 自动含 MCP → 模型调用经注册表按 `plugin_id` 路由到 `McpManager`，结果经 `tool_dispatcher` 出口转换器把图片翻成 `InputImage`（纯文本插件 no-op）→ 命令面让 Chat/serve 都能增删查管连接器。新增集中在 `core/connector/*` + `tool_dispatcher` 出口转换器，其余是既有文件的小改与复用。

---

## 6. 配置与环境变量

| 变量 | 取值 | 含义 | 优先级 | 说人话 |
|------|------|------|--------|--------|
| `TOMCAT__CONNECTOR__ENABLED` | `1`/`true` | 覆盖 `[connector] enabled` 总开关 | env（最高） | 环境变量一设就开/关 MCP。 |
| `[connector] enabled` | bool | 连接器模块总开关 | config | 配置文件里的总闸，默认开；没有 server 时不产生进程。 |
| `[connector] disabled` | string[] | 按 name 停用连接器 | config | 点名停用某个连接器。 |
| `~/.tomcat/mcp.json` | 文件 | MCP server 列表（`mcpServers`，全局） | config | 全局 MCP 清单（形状同 Cursor）。 |
| `<workspace>/.tomcat/mcp.json` | 文件 | 项目级覆盖同名 server | config（高于全局） | 项目自己的 MCP 清单。 |
| `~/.tomcat/connector-trust.json` | 文件 | 信任记录（启动方式指纹 + 内容身份） | 运行时状态 | 记住批准过哪些连接器、以及批准的是哪一版。 |

总则：**env > config > 默认**；`enabled` 默认 `true`，但**没有 `mcp.json` server 时不拉起任何进程**。这样用户只需添加 `name / command / args`，不必再记一个总开关；需要一键停用全部连接器时才显式设为 `false`。

---

## 7. 错误模型 / 截断 / 警告

```text
[connector] enabled=false            → 完全不发现/连接，工具面无 mcp__*（静默）
全局 mcp.json / curated / 命令未变    → 默认放行：直接 spawn（无感，Cursor 对齐）
项目 mcp.json 首见 / 命令指纹变化     → NeedsConfirm：不自动连（后台预连跳过，不阻塞会话），`/connector list` 列为「待确认」+命令 diff；`/connector trust` 批准后 spawn 并记指纹（防 MCPoison 同名换命令）
用户 /connector deny（或不批）        → Blocked：不 spawn，工具面不含其工具（可行动错误，非 Err 中断）
用户 server 用浮动版本(@latest/无版本) → 仍连接 + 一行告警建议钉死（不拦；curated 内置本就钉死）
integrity 已配但比对不一致（同版本被掉包）→ Blocked：需重新确认（可选强档才有此校验）
spawn 失败 / initialize 超时    → Failed：warning，该 server 缺席工具面（不影响其它 server 与主循环）
tools/list 分页失败            → 本次连接失败，server 转 Failed；该 server 缺席工具面
tools/call 超时                → 返回明确错误文本给模型、server 转 Disconnected，不重放调用
tools/call 途中传输断裂        → 当前调用返回错误 + server 转 Disconnected；用 test/reload 显式重连；【不 replay】
image 块解码失败 / 超 IMAGE_MAX_BYTES → 跳过该图 + 文本注明 `[MCP image omitted: …]`；其余内容照常回流
非 vision 模型收到 image parts → degrade_unsupported_multimodal 占位替换（不报错）
重连预算耗尽                    → server 停留 Failed；不无限重试（有界退避）
```

原则：**单个 server 的任何失败都不拖垮主循环，也不拖垮其它 server**；致命面只有「配置解析非法」在启动时 warn 并跳过该条目。

---

## 8. 测试矩阵（验收）

| 维度 | 用例 / 编号 | 状态 | 说人话 |
|------|-------------|------|--------|
| 单元 | `config::tests::{minimal_cursor_style_server_uses_optional_field_defaults,project_server_overrides_global_server_with_same_name,identifies_floating_npx_package_versions}`；`builtin::tests::materializes_cursor_style_pinned_playwright_config_idempotently` | EXISTS | `mcpServers` 最小字段、默认值、分层覆盖、物化浏览器缓存路径与浮动版本检测。 |
| 单元 | `naming::tests::{short_names_remain_readable,long_names_keep_a_readable_head_and_unique_hash,underscore_in_server_and_tool_names_is_preserved}` | EXISTS | 常态可读、超长哈希不撞、下划线不丢；反解由 manager 的 `McpToolDef` 查表承担。 |
| 单元 | `trust::tests::{global_config_is_auto_trusted_but_command_change_requires_confirmation,configured_curated_server_is_trusted_by_default,project_config_requires_one_explicit_approval,user_floating_version_warns_not_blocks,environment_change_never_leaks_value}` | EXISTS | 默认放行、项目确认、命令/环境变化与脱敏，浮动版本只告警。 |
| 单元 | `manager::tests::{fake_stdio_server_lists_and_calls_tools,call_tool_timeout_returns_error_and_marks_server_disconnected,transport_drop_marks_disconnected_without_replaying_call,reconnect_refetches_tools_without_a_persistent_cache}` | EXISTS | 连接、超时、断裂不 replay；显式重连现取 `tools/list`，不落盘缓存。 |
| 单元 | `tool_dispatcher::tool_result_media_tests::{mcp_image_block_becomes_input_image,text_block_becomes_model_text,unknown_block_becomes_a_text_summary,text_only_plugin_result_preserves_prior_empty_follow_up_parts_behavior}`；`multimodal_test::degrade_unsupported_multimodal_replaces_images_and_files_with_placeholders` | EXISTS | 出口转换器分流、图片回流、纯文本插件 no-op、非 vision 降级。 |
| 单元 | `connector::tests::{composite_routes_by_plugin_id,connector_disabled_master_switch_does_not_connect_or_register_tools,startup_connect_is_non_blocking,ready_mcp_tools_register_into_the_shared_tool_registry}`；`context::tests::connector_registry_constructed_when_enabled` | EXISTS | plugin_id 路由、总开关、后台启动、Ready 挂工具与 NotReady 撤工具。 |
| 集成 | `tests/connector_mcp_tests.rs::{pending_confirm_project_server_is_absent_until_confirmed,pasted_cursor_style_mcp_snippet_connects_without_translation,fake_stdio_server_end_to_end_image_reflow}` | EXISTS | 项目配置确认门、Cursor 片段零翻译、真实 stdio→注册表→下一轮 `InputImage`。 |
| 集成 | `cmd_connector_test::cmd_connector_add_and_list_round_trips_filtered_tools`；`control_test::serve_connector_commands_keep_list_light_and_tools_on_demand` | EXISTS | Chat/serve 对等写盘、重载、富摘要、按需工具表与 `toolFilter`。 |
| E2E | `tests/ui_acceptance_real_llm_e2e.rs::{e2e_2_real_llm_uses_phase_2_mcp_and_receives_a_screenshot,e2e_3_real_llm_switches_from_phase_1_to_phase_2_when_interaction_is_required,e2e_4_real_llm_directly_drives_a_headed_playwright_browser}` | `#[ignore]`，不进默认 CI / `test-groups.sh` | 真实视觉 LLM + `@playwright/mcp`：对 `interactive.html` 调用 `browser_navigate` / `browser_click` / `browser_take_screenshot`，断言 PNG 回流为 `InputImage` 且模型读出点击后才出现的 token。`e2e_3` 还要求先走 Phase 1 `shot.mjs`；`e2e_4` 省略 `--headless`，只经 MCP 驱动可见 Chrome，并将点击后的 PNG 持久保留至仓库根的 `.tomcat/shots/headed-interaction.png`，不走 verify/shot 流程。需 `TOMCAT_REAL_LLM_E2E=1` + API key + npx + 浏览器，按需显式运行。 |
| 关键承诺 | R4/R5 图片→`fake_stdio_server_end_to_end_image_reflow`；R7→trust 组；R8→`transport_drop_marks_disconnected_without_replaying_call`；R9→connector 组；R-C1/R-C2→Chat/serve 命令组。 | EXISTS | 每一条已落地承诺均有对应测试；运行结果以本次验证记录为准。 |

---

## 9. 风险与应对

| 风险 | 影响 | 应对（具体动作） | 说人话 |
|------|------|--------------------|--------|
| 共享仓库同名换命令（MCPoison, CVE-2025-54136） | 高（任意子进程执行） | 信任**绑命令指纹而非 server 名**（R7）：项目 `.tomcat/mcp.json` 首见 / 命令指纹变化 → spawn 前一次确认（附命令 diff）；全局/curated 无感放行；`env_clear` + 白名单注入 | 别人在你仓库里同名换了启动命令，会先弹确认——正是 Cursor 当年栽的坑。 |
| npm 供应链掉包（同版本被上游替换） | 中 | 默认命令绑定挡不住「同版本被掉包」；提供**可选强档**（预装 + `integrity`/SHA256 spawn 前逐字节校验）给安全敏感用户；curated 用广泛使用的钉死版 `@playwright/mcp`；**不谎称"已校验内容"** | 想更严就用强档预装+校验；默认够挡换命令，挡不住同版本掉包，我们不吹牛。 |
| 用户 server 用 `@latest`（内容会漂移） | 低-中 | 用户 server **只告警不拦**（Cursor 亦不拦，且命令指纹能捕捉解析出的具体命令变化）；一行提示建议钉死；curated 内置本就钉死 | 不挡你用 @latest，但会提醒你钉死更稳。 |
| `rmcp` 依赖过重/不稳 | 中 | 传输封装在 `core/connector/mcp/transport.rs` 单点，`McpManager` 只依赖内部 `McpTransport` trait；若 `rmcp` 不可用，按 pi_agent_rust 手写 NDJSON 传输替换该单点（推翻条件已写入 R1） | 把 rmcp 关在一个盒子里，不行就换实现，不动上层。 |
| 子进程僵尸/泄漏 | 中 | 当前交由 `rmcp::TokioChildProcess` 的 Drop 生命周期收尾；Tomcat 尚未实现 `McpManager::shutdown`、进程组隔离或 stderr 落盘。若生产观测证明 rmcp 收尾不足，再补显式 shutdown（不能在文档里假称已有）。 | 协议库目前负责收尾；需要用运行证据决定是否补更强的进程管理。 |
| 一次调用重复副作用 | 中 | 在途断裂不 replay（R8）；`tools/call` 幂等性未知时只报错 | 断在半路不重来，避免重复点击/提交。 |
| 大截图撑爆上下文/写放大 | 中 | `upload_decision_by_size` 大图走 Files API 不内联；`IMAGE_MAX_BYTES` 上限；超限跳过 + 文本注明 | 大图走上传通道，别把上下文塞爆。 |
| 非 vision 模型收到图片 | 低 | `degrade_unsupported_multimodal` 占位替换，不报错 | 模型看不了图就换成占位说明。 |
| server 启动慢阻塞首轮 / 首轮看不到工具 | 低 | **后台启动期连接（非阻塞）**，不 block 首轮；就绪前工具缺席 surface、就绪后下一轮自动出现；`startup_timeout_ms` 兜底；失败 server 缺席工具面不阻塞主循环。**不采用"首次调用才连接"**——那会让模型看不到工具、无从调起 | 后台连、不卡对话；连上了下一轮工具就出现，别等到调用才连。 |
| schema 破坏（provider 工具名 64/128 字符限制） | 低 | 常态原样保留；仅超限时消毒 + 截断可读头 + 短哈希（R3），单测 `long_name_keeps_readable_head_plus_short_hash` 锁死 | 常态是完整可读名；太长才截断加小尾巴，不撞协议限制。 |
| 同一 server 并发 `tools/call` 打乱 stdio | 中 | 每 server 一把请求锁串行化 `call_tool`（仿 pi_agent_rust `_rpc_lock`）；rmcp 靠 JSON-RPC id 关联响应，锁只防同进程读写交错 | 一个 server 同时只处理一个调用，别把管道搅乱。 |
| server 运行时改工具集（`tools/list_changed`） | 低 | 首期**不处理** `notifications/tools/list_changed`（`@playwright/mcp` 工具集静态）；工具集变化在下次连接/刷新时生效——列为非目标，与持久缓存一起做 fast-follow（R12；cline/vscode 有现成 debounced 刷新范式） | playwright 工具是固定的，暂不追它的动态变更通知。 |
| MCP 工具名与插件/builtin 重名 | 低 | Option B 下路由靠注册与 `plugin_id`（非字符串前缀），不会误路由；重名由 `register_tool_local` 的 builtin/跨-plugin 重名校验拒绝；`mcp__` 前缀本身也让撞名概率极低 | 靠登记路由不靠猜前缀；重名直接被注册校验挡下。 |

---

## 10. 历史决策 / 跨文档修订

- 本模块为**前端 UI 验收计划**（规划产物 `frontend_ui_acceptance_capability_a3f1bb7e`，用户 `~/.cursor/plans/`，不在本仓）的 **Phase 2** 落地设计；该计划 §4「Phase 2」是需求来源，本文是其技术方案，计划侧已回链本文。
- ~~最初把本文定位为「MCP 客户端子系统」（`core/mcp/`）~~ → 升华为**连接器(Connector)模块**（`core/connector/`）：MCP/CLI/A2A 都是「接入外部能力→暴露工具→回流结果」的同构问题，抽象出 `Connector` trait / `ConnectorType` / `ConnectorRegistry`（R-C0）。**本期只实现 MCP 连接器**（CLI/A2A 预留），MCP 传输**本期只 stdio**、Streamable HTTP+OAuth 设计紧跟（R2/§4.4）；命令面 Chat+serve 对等增删查管（R-C1/R-C2）。代码/配置随之升华：`core/mcp/*`→`core/connector/mcp/*`、`mcp-trust.json`→`connector-trust.json`、`[mcp]`→`[connector]`；`mcp__` 工具名前缀保留（**仅命名约定**，Option B 下路由靠注册表 `plugin_id`、非前缀）。
- ~~初稿把 MCP 配置定为自研多类型配置文件、每条带 `type`~~ → **改采生态标准 `mcpServers` JSON（`~/.tomcat/mcp.json`，R6/R10；用户以 Cursor 截图拍板）**：Cursor/Claude/VS Code 均用 `mcpServers`/`servers` JSON，用户从生态复制片段零翻译即用；必填收敛到 `command`+`args`（与 Cursor 一致），`type` 留代码侧（只有 MCP 时不摊派冗余字段，YAGNI）。**推翻条件**：CLI/A2A 落地、需同文件声明多类型时再评估统一 `type` 配置。连带：`ConnectorType` 抽象仍在**代码**保留（R-C0 不变），只是 MCP 的**配置文件**改用 `mcp.json`。
- ~~命令面未显式界定形态~~ → **本期命令面 = CLI + 直接编辑 `mcp.json`，无 webview GUI**（R11）；CLI 对标 Cursor GUI 补齐 `list` 富状态 / `reload` / `tools`(toolFilter 裁剪)，serve capabilities 同步就绪、GUI 作 fast-follow。
- ~~最初把不进 registry 的理由写成「`PluginToolExecutor` 执行路径**清空** `follow_up_parts`」~~ → 更正：注册表/插件契约是 **JSON-only**（`ToolExecutor::execute -> serde_json::Value`，因插件是 rquickjs JS-VM），从来**没有**承载原生图片 parts 的通道，dispatcher 只能填 `Vec::new()`（是「无通道」，不是「清空」；git 溯源 `e8a5b7fc`/`2ffd58a2`，`follow_up_parts` 由 PR-RJ T3-c 仅给 builtin `read` 引入）。
- ~~首期「只并入 `ToolSurface` + `mcp__` 前缀走 builtin 分派」以隔离插件路径~~ → **改采 Option B（R4，用户拍板）**：MCP 工具**注册进 `ToolRegistry`**（`plugin_id="mcp:{server}"`，`CompositeToolExecutor` 路由），`list_tools` 自动上工具面；在 `tool_dispatcher` 出口加向后兼容的「JSON 图片块→`follow_up_parts`」转换器（`extract_tool_result_media`，对纯文本插件 no-op）。理由：消除「可见走 surface / 执行走前缀」不对称与 `mcp__` 前缀抢注隐患、少改 run_loop/system_prompt/tool_exec，并顺带解锁「能返图的插件」。
- ~~最初设想「复用 `PermissionGate` 门控 MCP」~~ → 否：`PermissionGate` 是路径/bash 语义，且插件执行期本就绕过它；MCP 风险在 spawn，改为 MCP 专属 spawn 前信任门（R7）。
- 信任模型两次修订：~~初稿「仅 `hash(command+args+env+cwd)`」~~ → 一度升级为「启动方式指纹 + 强制内容身份 + 每 server spawn 前 `/connector trust` + 禁用 `@latest`」 → **最终（Cursor 对齐，用户拍板）回落为「config 即信任」**（R7）：全局 `~/.tomcat/mcp.json`/curated **无感直连、零额外步骤**；只保留 Cursor 踩坑后补的那一个门——信任**绑命令指纹而非 server 名**，**项目来源首见 / 命令变化才确认一次**（防 CVE-2025-54136 MCPoison）。内容哈希（integrity/SHA256）降为**可选强档**；用户 server 的浮动版本**告警不拦**（Cursor 亦不拦），curated 内置仍钉死。**推翻条件**：若未来要面向企业/团队分发不可信 server，可加回 vscode 式 workspace-trust 或强制内容哈希。
- 跨文档修订：`assets/skills/verify/SKILL.md` 的「UI acceptance」节需在本子系统落地后补「交互路径工具清单」（`@playwright/mcp` 的 `browser_*` 工具）与「默认无头脚本、需交互时切 MCP」的选择准则（P6）。
- **v2 演进（渐进式披露，最新且权威）**：R4 Option B「MCP 工具注册进 `ToolRegistry`、随 `list_tools` 进每轮前缀」导致「易变完整目录进了缓存前缀」→ 缓存失效 + token 爆炸。已被修订——**R4 的「进前缀」部分与 R9「Ready 即注册」**改走渐进式披露；完整设计见 [v2-progressive-disclosure.md](./v2-progressive-disclosure.md)，总纲/导航见 [../mcp-client.md](../mcp-client.md)。本文其余决策（R2 传输 / R5 图片回流 / R7 信任 / R10 配置形状）v1 仍是权威。

---

## 一句话总结

**连接器(Connector)模块**把「外部能力（MCP/CLI/A2A）的工具」统一接入 Agent；**本期实现 MCP 连接器**：**config 即信任（Cursor 对齐，全局配好零步骤直连；仅项目来源/命令变化才确认一次防 MCPoison）**拉起，工具**和插件一样注册进 `ToolRegistry`**（`plugin_id="mcp:{server}"`，`CompositeToolExecutor` 路由到 `McpManager`）、经 `list_tools` 自动进 **ToolSurface** 让模型可见（**Option B**），返回图片经 `tool_dispatcher` 出口的向后兼容转换器（`extract_tool_result_media`，纯文本插件 no-op）走 Tomcat **既有的 `follow_up_parts`+`InputImage` 回流管道**喂回模型；Chat 与 serve 都提供**添加/查看管理连接器**命令。**MCP 传输本期只 stdio（用 `rmcp`）、Streamable HTTP+OAuth 设计紧跟；CLI/A2A 预留；不新建图片管道、不新增门禁类型**——为前端 UI 验收 Phase 2 的 `@playwright/mcp` 交互式验收提供底座。

> ⚠️ 上述 §3.1 R4 与 R9 的「MCP 工具进前缀」结论已被 v2 修订，详见 [v2-progressive-disclosure.md](./v2-progressive-disclosure.md)。
