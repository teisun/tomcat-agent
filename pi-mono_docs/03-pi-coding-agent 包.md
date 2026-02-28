# pi-coding-agent 包

pi-coding-agent（`@mariozechner/pi-coding-agent`）是 pi 交互式编码 Agent 的 CLI 与 SDK：提供 `pi` 命令行、四种运行模式（交互 / print / JSON / RPC）、Extensions/Skills/Prompt 模板/Themes 扩展体系，以及 `createAgentSession` 供嵌入或自动化使用。

---

## 1. 模块职责

- **CLI 入口**：`cli.ts` → `main.ts`，解析参数、加载 ResourceLoader、创建 AgentSession、按模式进入 InteractiveMode / runPrintMode / runRpcMode。
- **SDK**：`createAgentSession(options)` 返回 `AgentSession`，封装 Agent 生命周期、会话持久化、compaction、模型与 thinking 管理；上层通过 `session.prompt()` 与 `session.subscribe()` 使用。
- **资源加载**：DefaultResourceLoader 负责 Extensions、Skills、Prompt 模板、Themes、AGENTS 文件、systemPrompt 等；扩展可注册 Provider、slash 命令、工具、UI 等。
- **运行模式**：交互（TUI）、print（单次输出）、RPC（JSON-RPC 供进程集成）。

---

## 2. CLI 入口与 main 流程

- **cli.ts**：`process.title = "pi"`，`main(process.argv.slice(2))`。
- **main.ts**：
  1. **handlePackageCommand** / **handleConfigCommand**：处理包/配置子命令，若命中则 return。
  2. **runMigrations**：迁移与弃用检查。
  3. **第一轮 parseArgs**：仅为了拿到 `--extension`/`--skill` 等路径，用于构造 ResourceLoader。
  4. **DefaultResourceLoader**：cwd、agentDir、settingsManager、additionalExtensionPaths 等，**reload()** 加载扩展与技能。
  5. **getExtensions()**：取 LoadExtensionsResult，把扩展的 **pendingProviderRegistrations** 写入 ModelRegistry，再根据扩展的 **flags** 做第二轮 **parseArgs**（含扩展自定义 flag）。
  6. 处理 **--version** / **--help** / **--list-models** / **--export**、stdin 管道、**prepareInitialMessage**。
  7. **createSessionManager**（根据 --session-dir 等）、可选 **selectSession**（--resume）。
  8. **buildSessionOptions**：拼出 createAgentSession 的 options（model、thinkingLevel、scopedModels、sessionManager、resourceLoader、authStorage、modelRegistry 等）。
  9. **createAgentSession(sessionOptions)** → `session`。
  10. 若 **mode === "rpc"**：**runRpcMode(session)**；若 **isInteractive**：`new InteractiveMode(session, {...}).run()`；否则 **runPrintMode(session, {...})**，结束后 `stopThemeWatcher`、exit。

---

## 3. createAgentSession 与 SDK（core/sdk.ts）

- **CreateAgentSessionOptions**：cwd、agentDir、authStorage、modelRegistry、model、thinkingLevel、scopedModels、tools、customTools、resourceLoader、sessionManager、settingsManager 等。
- **createAgentSession(options)**：
  - 若无 resourceLoader，则 new DefaultResourceLoader 并 **reload()**。
  - 用 sessionManager **buildSessionContext()** 判断是否有已有会话；若有则尝试从 session 恢复 model/thinkingLevel，否则 **findInitialModel**（settings + provider 默认）。
  - 确定 thinkingLevel（会话恢复 / settings / 默认），并按 model 能力 clamp（如无 reasoning 则 "off"）。
  - 构造 **convertToLlm**（内部用 coding-agent 的 convertToLlm，并可叠加 blockImages 等）。
  - **createAllTools**（或 options.tools）与扩展的 **wrapToolsWithExtensions**，得到 AgentTool 列表。
  - **buildSystemPrompt**（resourceLoader.getSystemPrompt、getAgentsFiles 等）。
  - new **Agent**（initialState、convertToLlm、transformContext、streamFn 等），再 new **AgentSession**（agent、sessionManager、resourceLoader、extensionsResult 等）。
  - 若有会话数据则 **restore** 到 session。
  - 返回 `{ session, extensionsResult, modelFallbackMessage? }`。

---

## 4. AgentSession（core/agent-session.ts）

- **职责**：封装 Agent 生命周期、事件订阅与自动持久化、模型/thinking 管理、compaction（手动与自动）、bash 执行、会话切换与分支；所有模式共用。
- **prompt(text, options?)**：内部转成 user 消息，调用 agent.prompt()，订阅 agent 事件并转发给 session 的 listeners，同时在适当时机持久化（如 message_end、turn_end 后）。
- **subscribe(fn)**：订阅 AgentSessionEvent（含 AgentEvent 及 auto_compaction_* 等）。
- **状态与模型**：get/set model、thinkingLevel、session 名、分支等；与 SessionManager 协作做 save/restore/switch/branch。
- **Compaction**：根据 token 估算与阈值做自动或手动压缩（保留摘要、移除旧消息），见 compaction 模块。
- **扩展集成**：ExtensionRunner、slash 命令、扩展工具包装、UI 上下文（扩展可挂载 overlay 等）。

---

## 5. ResourceLoader 与扩展体系（core/resource-loader.ts）

- **ResourceLoader 接口**：getExtensions()、getSkills()、getPrompts()、getThemes()、getAgentsFiles()、getSystemPrompt()、getAppendSystemPrompt()、getPathMetadata()、extendResources()。
- **DefaultResourceLoader**：
  - **reload()**：合并 cwd/agentDir 下的配置与 CLI 传入的路径，**loadExtensions**（扩展列表）、**updateSkillsFromPaths**、**loadPromptTemplates**、**loadThemes**、加载 AGENTS 文件与 systemPrompt；扩展冲突检测（重名命令/工具/flag）报错但继续加载。
  - 扩展可从包（package.json 的 pi 字段）或目录加载，产出 **LoadExtensionsResult**（extensions 数组、runtime、errors）；runtime 上有 **pendingProviderRegistrations**，main 中会注入到 ModelRegistry。
- **Extensions**：可提供 Provider、slash 命令、工具定义、flags、UI 组件/overlay、beforePrompt 等钩子；见 packages/coding-agent/docs/extensions.md。

---

## 6. Skills 与 Prompt 模板

- **Skills**：由 ResourceLoader 从配置路径与 CLI 路径加载，返回 Skill[]（name、description、指令等）；用户通过 `/skill:name` 或技能块 `<skill name="..." location="...">` 使用。
- **Prompt 模板**：loadPromptTemplates 得到 PromptTemplate[]，用户通过 `/templatename` 展开为一段 system/user 内容。

---

## 7. 运行模式

- **InteractiveMode**（modes/interactive/）：TUI（基于 pi-tui），Editor、消息区、footer、slash 命令、/model、/login 等；session.subscribe 驱动 UI 更新，输入提交后 session.prompt()。
- **runPrintMode**（modes/print-mode.js）：非交互，一次性 session.prompt(messages)，将输出打到 stdout，可选 JSON 等格式。
- **runRpcMode**（modes/rpc/rpc-mode.ts）：stdin/stdout JSON-RPC，与 RpcClient 配套，供外部进程调用 prompt、切换模型等，见 docs/rpc.md。

---

## 8. 内置工具（core/tools/index.ts）

- **codingTools**：read、write、edit、bash（默认）；**readOnlyTools**：read、grep、find、ls；**allTools** 含全部。每项为 AgentTool，execute 中调用 readFile、writeFile、edit（diff/patch）、bash 执行等，cwd 默认 process.cwd()，也可通过 createReadTool(cwd) 等工厂指定。

---

## 9. 会话与 Compaction

- **SessionManager**：持久化目录、当前 session 名、分支栈；**buildSessionContext()** 读出已保存的 messages、model、thinkingLevel 等供恢复。
- **Compaction**：当 context token 估算超过阈值或发生 context overflow 时，可自动或手动触发；**compact** 生成摘要、裁剪旧消息，结果写回 session 并通知订阅者（auto_compaction_start/auto_compaction_end 等）。

---

## 10. 关键文件路径

| 文件 | 说明 |
|------|------|
| `packages/coding-agent/src/cli.ts` | CLI 入口，调用 main |
| `packages/coding-agent/src/main.ts` | 参数解析、ResourceLoader、createAgentSession、分支 Interactive/Print/RPC |
| `packages/coding-agent/src/core/sdk.ts` | createAgentSession、CreateAgentSessionOptions、工具/AgentSession 导出 |
| `packages/coding-agent/src/core/agent-session.ts` | AgentSession、prompt、subscribe、compaction、扩展集成 |
| `packages/coding-agent/src/core/resource-loader.ts` | ResourceLoader、DefaultResourceLoader、reload、getExtensions/getSkills/getPrompts/getThemes |
| `packages/coding-agent/src/core/extensions/` | 扩展加载、ExtensionRunner、类型与 loader |
| `packages/coding-agent/src/core/tools/index.ts` | 内置 read/write/edit/bash/grep/find/ls 与工厂 |
| `packages/coding-agent/src/modes/index.ts` | InteractiveMode、runPrintMode、runRpcMode 导出 |
| `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | 交互模式主循环与 TUI |
| `packages/coding-agent/src/modes/rpc/rpc-mode.ts` | RPC 模式入口 |
