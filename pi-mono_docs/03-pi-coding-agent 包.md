# pi-coding-agent 包

## 先用大白话

**pi-coding-agent**（`@mariozechner/pi-coding-agent`）就是你在终端打的 **`pi`**：既能**像聊天软件一样交互**，也能**打印一行结果**、**吐 JSON**、或走 **RPC** 给别的程序遥控。它还负责：**会话存盘**、**扩展/技能/主题**、以及把「工程目录」和「模型、密钥、设置」对齐。

把它想成**瑞士军刀的外壳**：刀片是 **AgentSession + SessionManager**（真干活），外壳是 **CLI + TUI**。

---

## 往里说：两条入口线（很重要）

1. **你自己在别的程序里嵌入**：常用 **`createAgentSession`**（`packages/coding-agent/src/core/sdk.ts`），拿到 `session` 后 `prompt` / `subscribe` 即可。
2. **仓库里的 `pi` 命令行**：`main.ts` 走 **`createAgentSessionRuntime` → `createAgentSessionServices` → `createAgentSessionFromServices`**，把「扩展加载、第二轮 argv、诊断信息、缺 cwd 的会话恢复」等都包进去；交互模式里是 **`new InteractiveMode(runtime, …)`**，参数是 **`AgentSessionRuntime`**，不是裸的 `AgentSession`。

读代码时：**看到 runtime 就想到 CLI 这条线**；**看到 createAgentSession 就想到 SDK 这条线**。

---

## ASCII：CLI 启动后大块顺序

```
  cli.ts -> main()
       |
       v
  [包管理/配置子命令?] ---> 是 ---> return
       | 否
       v
  runMigrations / parseArgs(第一轮，为扩展路径)
       |
       v
  createSessionManager (含 --session / --resume)
       |
       v
  createAgentSessionRuntime(createRuntime, { cwd, agentDir, sessionManager })
       |
       +--- createRuntime 内部:
       |       createAgentSessionServices(...)  // ResourceLoader、ModelRegistry、Settings…
       |       buildSessionOptions(...)
       |       createAgentSessionFromServices(...)  // 得到 session
       |
       v
  mode 分支:  rpc -> runRpcMode(runtime)
              交互 -> new InteractiveMode(runtime, ...).run()
              其它 -> runPrintMode(...)
```

---

## 模块职责（仍成立的部分）

- **CLI**：`cli.ts` 设 `process.title`，转 `main.ts`。
- **SDK**：`createAgentSession`、`AgentSession`、`SessionManager`、工具工厂等仍从 `core/sdk.ts` 及关联模块导出。
- **资源**：`DefaultResourceLoader` 加载扩展、技能、Prompt 模板、主题、`AGENTS` 文件等；扩展可注册 Provider、slash、工具、CLI flag。
- **模式**：交互（TUI）、print、JSON、RPC（stdin/stdout JSON-RPC）。

---

## AgentSession（`core/agent-session.ts`）

- **prompt / subscribe**：和 Agent、持久化、compaction、模型切换等协作。
- **SessionManager**：JSONL 会话文件、分支、列表（见 [storage-design-comparison](storage-design-comparison.md) 与源码 `session-manager.ts`）。

---

## ResourceLoader 与扩展（`core/resource-loader.ts`）

- **reload()**：合并 CLI 路径与配置目录；**loadExtensions** 等；扩展冲突（重名命令/工具/flag）会报错但尽量继续加载。
- **扩展文档（在仓库里，不在 pi-mono_docs）**：`packages/coding-agent/docs/extensions.md`；RPC 说明：`packages/coding-agent/docs/rpc.md`；会话格式：`docs/session.md` 等。

---

## Skills 与 Prompt 模板

- **Skills**：`/skill:name` 或 `<skill …>` 块（细节见仓库 `docs/skills.md`）。
- **Prompt 模板**：`/模板名`（见 `docs/prompt-templates.md`）。

---

## 运行模式（源码路径）

- **InteractiveMode**：`src/modes/interactive/interactive-mode.ts`，接收 **`AgentSessionRuntime`**。
- **runPrintMode**：`src/modes/print-mode.ts`（不是 `.js` 源文件；构建产物在 dist）。
- **runRpcMode**：`src/modes/rpc/rpc-mode.ts`。

---

## 内置工具（`src/core/tools/index.ts`）

| 常量 / 对象 | 包含 |
|-------------|------|
| **codingTools** | read, bash, edit, write |
| **readOnlyTools** | read, grep, find, ls |
| **allTools** | 上述全部名字的对象映射 |

另有 `createCodingTools(cwd)`、`createReadOnlyTools`、`createAllTools` 等工厂，便于指定工作目录。

---

## 会话文件放在哪（默认）

目录由 `getDefaultSessionDir(cwd)` 计算：`~/.pi/agent/sessions/--<cwd 编码>--/`（把路径里的 `/` `\` `:` 换成 `-`，两侧加 `--`）。  
每个会话一个 **`.jsonl`** 文件，文件名 **`{ISO时间戳把:和.换成-}}_{sessionId}.jsonl`**，`sessionId` 为 **UUID v7**（见 `SessionManager.newSession`）。

---

## 关键文件路径

| 路径 | 说明 |
|------|------|
| `packages/coding-agent/src/cli.ts` | CLI 入口 |
| `packages/coding-agent/src/main.ts` | 参数解析、runtime、模式分发 |
| `packages/coding-agent/src/core/sdk.ts` | `createAgentSession`（嵌入用） |
| `packages/coding-agent/src/core/agent-session-runtime.ts` | CLI 用的 runtime 类型与工厂 |
| `packages/coding-agent/src/core/agent-session-services.ts` | 组装 services |
| `packages/coding-agent/src/core/agent-session.ts` | AgentSession |
| `packages/coding-agent/src/core/session-manager.ts` | 会话 JSONL、版本 CURRENT_SESSION_VERSION = 3 |
| `packages/coding-agent/src/core/resource-loader.ts` | 资源与扩展加载 |
| `packages/coding-agent/src/core/tools/index.ts` | 内置工具 |
| `packages/coding-agent/docs/` | 用户向文档：extensions、rpc、session、skills… |
