# TASK-05c Tier 2 插件兼容：代码改动技术报告（学习版）

**范围**：`feature/plugin-compat-tier2` 上为 TASK-05c（T1-P1-002c）落地的宿主/桥接/测试改动。  
**目的**：说明每一处主要改动的**用意、作用、意义**，辅以术语与 ASCII 示意图，便于对照源码阅读。

**相关看板**：[agents/TASK_BOARD.md](../../agents/TASK_BOARD.md) TASK-05c；**状态文档**：[docs/status/feature-plugin-compat-tier2.md](../status/feature-plugin-compat-tier2.md)。**常见概念问答**见文末 **§7 附录：FAQ**。

---

## 1. 总览：业务目标与架构位置

**用意**：让 pi-mono 风格插件在「命令注册 + 带参数的 exec + 基础 UI + 工具 schema + 会话消息」链路上与宿主契约更接近，可在 **WasmEdge QuickJS + 宿主 Rust** 闭环里跑通并可测。

**作用**：在 **Host API 边界**（`pi_bridge.js` ↔ `HostApiDispatcher` ↔ `PrimitiveExecutor` / `SessionManager` / 工具注册）补齐语义缺口。

**意义**：Tier 2 是社区扩展的主流量能力层；不补会导致扩展**静默行为错误**（例如 argv 丢失、UI 永远 stub、消息不进 transcript），调试成本高且与 pi-mono 心智模型不一致。

---

## 2. 分层与术语速查

| 层 | 术语 | 本次相关 |
|----|------|----------|
| 插件 JS 运行时 | QuickJS on Wasm、Hostcall、事件循环 | `hostCall` / `hostCallAsync`、`__pi_invoke_command` |
| 桥接层 | Bridge / shim、`globalThis.pi` | [assets/js/pi_bridge.js](../../assets/js/pi_bridge.js) |
| 宿主分发 | Host API dispatcher、按 module/method 路由 | [src/ext/dispatcher.rs](../../src/ext/dispatcher.rs) |
| 领域服务 | 4 原语执行器、会话 transcript、工具注册 | `PrimitiveExecutor`、`SessionManager::append_message`、`parse_tool` |
| 可观测性 | tracing、E2E host 调用计数 | `tracing::info!`、`wasmedge_e2e_tests` |

---

## 3. 核心四图（ASCII）

### 图 1：端到端调用拓扑

```
┌─────────────────────────────────────────────────────────────────┐
│  QuickJS 插件脚本 (用户/社区扩展)                                │
│  globalThis.pi.*  + __pi_* 入口                                  │
└───────────────────────────┬─────────────────────────────────────┘
                            │ __pi_host_call(JSON)
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│  pi_bridge.js                                                   │
│  - 组 HostRequest { module, method, params, callId? }           │
│  - registerCommand: 本地 __pi_commands[name] = handler          │
│  - exec: hostCallAsync('fs','executeBash', {command,args,cwd})  │
└───────────────────────────┬─────────────────────────────────────┘
                            │ JSON 往返
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│  Rust: HostApiDispatcher::dispatch(_async)                      │
│  - 路由 (module,method) -> do_*                                 │
│  - 可选: SessionManager / PrimitiveExecutor / plugin_commands   │
└───────────────────────────┬─────────────────────────────────────┘
                            │
            ┌───────────────┼───────────────┐
            ▼               ▼               ▼
    PrimitiveExecutor  SessionManager   DashMap 元数据
    (bash argv/shell)  append_message   (registerCommand)
```

### 图 2：`execute_bash` 双模式（Shell 串 vs Argv）

术语：*subprocess invocation*、*shell interpolation*、*argv-style execution*。

```
                    JSON: executeBash
                    ┌──────────────────┐
                    │ command          │
                    │ args?  (array)   │
                    │ cwd?             │
                    └────────┬─────────┘
                             │
               args 键存在且为数组 ?
                    /              \
                  否                是
                 /                    \
                ▼                      ▼
    ┌─────────────────────┐   ┌──────────────────────────┐
    │ Shell 模式           │   │ Argv 模式                 │
    │ sh -c "<整串命令>"     │   │ Command::new(prog)       │
    │ (兼容旧插件/LLM工具)   │   │   .args(argv)            │
    └─────────────────────┘   └──────────────────────────┘
```

**用意**：对齐 `pi.exec('gh', ['pr','list'], opts)` 的 argv 语义。  
**作用**：Trait 与 dispatcher 同时识别 `args` 数组。  
**意义**：减少经 shell 拼接参数带来的错误与安全面；白名单仍以程序名/审计串为策略入口。

### 图 3：`registerCommand`：JS handler + Rust 元数据

术语：*cross-language boundary*（JS 函数不可序列化过 FFI）、*registry*。

```
  pi.registerCommand(name, {description, handler})
           │
           ├──────────────────────────────┐
           │                              │
           ▼                              ▼
   JS 侧 __pi_commands[name]      HostCall tools.registerCommand
   { description, handler }         { name, description }
           │                              │
           │                              ▼
           │                    Rust: plugin_commands
           │                    DashMap<instance_id, Vec<(name,desc)>>
           │
           ▼
   __pi_invoke_command(name, argsJson)   // 同 VM 内同步调用
   (async handler -> 明确报错，避免假成功)
```

### 图 4：`sendMessage` 与会话 transcript

术语：*session transcript*、*append-only log*、`options.silent`。

```
  pi.sendMessage(msg, options)
           │
           ▼
  HostCall agent.sendMessage { message, options }
           │
           ▼
  HostApiDispatcher
    ├─ 无 SessionManager -> 仅日志/空操作（兼容 Wasm 单测桩）
    ├─ options.silent == true -> 跳过 append
    └─ 否则 agent_send_message_wire(msg, options)
              │
              ▼
        SessionManager::append_message(JSON)
              │
              ▼
        transcript / JSONL（项目既有机制）
```

---

## 4. 分模块改动说明

### 4.1 [src/core/primitives.rs](../../src/core/primitives.rs)

| 改动 | 用意 | 作用 | 意义 |
|------|------|------|------|
| `execute_bash` 增加 `argv: Option<&[String]>` | 区分 shell 整串与 argv 执行 | 统一 Trait 契约 | 避免调用点字符串拼接模拟 argv |

### 4.2 [src/core/executor.rs](../../src/core/executor.rs)

| 改动 | 用意 | 作用 | 意义 |
|------|------|------|------|
| 白名单/黑名单/确认覆盖两模式 | 策略不随路径分叉 | argv 模式以 `command` 为程序名参与策略 | 与 pi-mono 调用方式一致 |
| `Command::new` + `args` | 减少 shell 介入 | 直接 spawn | 更接近最小权限执行 |

### 4.3 [src/ext/dispatcher.rs](../../src/ext/dispatcher.rs)

| 改动 | 用意 | 作用 | 意义 |
|------|------|------|------|
| `do_execute_bash` 解析 `args` | JSON 边界 -> Rust | 调用 `execute_bash(..., argv)` | Host API 与 4 原语对齐 |
| `normalize_tool_parameters` + `parse_tool` | schema 规整 | 展开 `schema` 包装、剥 `default`、补 object 外壳 | TypeBox/包装 JSON 更易被 LLM tool 消费 |
| `plugin_commands` + `do_register_command` | 命令注册表 | 可查询、可测 | 为 CLI/Agent 预留 |
| UI 路由替代 stub | 去掉恒 `{stub:true}` | 确定性结构化返回 | 无 TTY 的 CI 也可断言 |
| `agent.sendMessage` / `sendUserMessage` | 持久化路径 | 写当前会话 transcript | 与 chat 路径共享事实来源 |
| `registered_plugin_commands()` | 自省 API | 测试与未来 CLI | 可验证注册发生 |

### 4.4 [src/core/agent_loop.rs](../../src/core/agent_loop.rs) 与 [tests/agent_loop_tests.rs](../../tests/agent_loop_tests.rs)

| 改动 | 用意 | 作用 | 意义 |
|------|------|------|------|
| `execute_bash` 工具传 `args` | 与插件 JSON 一致 | 把 argv 传给 `PrimitiveExecutor` | Agent 工具链与插件链语义一致 |
| Mock 补 `_argv` | Trait 签名变更 | 编译与测试通过 | 避免假实现掩盖接口变化 |

### 4.5 [tests/primitives_tools_tests.rs](../../tests/primitives_tools_tests.rs)

| 改动 | 用意 | 作用 | 意义 |
|------|------|------|------|
| `test_primitive_executor_execute_bash_argv_echo` | 回归 argv 模式 | 真实 `echo` + 多参数 | 防止退化为仅支持 shell 串 |

### 4.6 [assets/js/pi_bridge.js](../../assets/js/pi_bridge.js)

| 改动 | 用意 | 作用 | 意义 |
|------|------|------|------|
| `__pi_commands` + `registerCommand` 存 handler | JS 侧闭包注册表 | host 只收可序列化字段 | 符合 FFI 边界 |
| `__pi_invoke_command` | 可测执行入口 | 同步、结果 JSON 化 | E2E 不依赖未接好的 CLI |
| `ctx.ui.setStatus` | API 面补齐 | `context.uiSetStatus` | 对齐 pi-mono 状态条用法 |

### 4.7 Wasm E2E 与 fixture

| 文件 | 用意 | 作用 | 意义 |
|------|------|------|------|
| [tests/fixtures/wasmedge_quickjs/tier2_compat_test.js](../../tests/fixtures/wasmedge_quickjs/tier2_compat_test.js) | 单脚本覆盖多 host 路径 | registerTool/UI/argv/bash/invoke | 契约回归，不绑本地 pi-mono |
| [tests/wasmedge_e2e_tests.rs](../../tests/wasmedge_e2e_tests.rs) | 自动化门禁 | Tier2 用例 + 转译 TS 片段 | 与 SWC/组合脚本路径一致 |

### 4.8 [openspec/specs/guides/testing/E2E_SCENARIO_LIBRARY.md](../../openspec/specs/guides/testing/E2E_SCENARIO_LIBRARY.md)

| 改动 | 用意 | 作用 | 意义 |
|------|------|------|------|
| E2E-WASM-037 / 038 | 可追溯 | 场景编号锚定测试名 | 评审与集成流程可读 |

### 4.9 流程与计划文档

| 文件 | 改动要点 |
|------|----------|
| [agents/TASK_BOARD.md](../../agents/TASK_BOARD.md) | TASK-05c 子项勾选、状态 `PENDING_INTEGRATION` |
| [docs/status/feature-plugin-compat-tier2.md](../status/feature-plugin-compat-tier2.md) | 分支元数据与接口摘要 |
| [agents/TASK_BOARD.md](../../agents/TASK_BOARD.md) + [`extension_compat_matrix.md`](../reports/extension_compat_matrix.md) | TASK-05 系列范围与矩阵验收（已无独立 `PLAN_TASK05_*.md`） |

---

## 5. 边界与后续

1. **`__pi_invoke_command`**：当前偏**同步**；若 `handler` 返回 Promise，会返回明确错误，避免在无 await 场景假成功。宿主驱动异步命令需与 VM 事件循环进一步协作。  
2. **ctx.ui**：现为 **headless 友好**的确定性响应，非完整 TUI；高级组件见 TASK-05d 等。  
3. **社区矩阵 10～15 插件**：本任务用 **fixture + 小段 TS** 做自动化门禁；全矩阵验收属 **TASK-05e**。本地可对读 [pi-mono](../../../pi-mono)（与仓库并列，不纳入本仓提交），CI 仍以仓库内 fixture 为准。

---

## 6. 推荐阅读顺序

1. [assets/js/pi_bridge.js](../../assets/js/pi_bridge.js)：`registerCommand`、`__pi_invoke_command`、`exec`。  
2. [src/ext/dispatcher.rs](../../src/ext/dispatcher.rs)：路由、`do_execute_bash`、`normalize_tool_parameters`、agent `sendMessage`。  
3. [src/core/executor.rs](../../src/core/executor.rs)：`execute_bash` 双分支。  
4. [tests/wasmedge_e2e_tests.rs](../../tests/wasmedge_e2e_tests.rs)：Tier2 用例，对照 E2E 场景库 **E2E-WASM-037 / 038**。

---

## 7. 附录：FAQ（常见疑问）

### Q1：`pi_bridge.js` 里的 `__pi_hooks`、`__pi_tools`、`__pi_commands`、`__pi_nextId` 是干嘛的？是给自定义插件用的吗？

**A：** 是的。它们都是 **在 QuickJS 里跑的扩展/插件** 用的 **内部表**，在注入 `globalThis.pi` 时由 `pi_bridge.js` 维护；业务代码应优先用 `pi.on`、`pi.registerTool`、`pi.registerCommand` 等公开 API。

| 变量 | 含义 | 与 `globalThis.pi` 的关系 |
|------|------|----------------|
| `__pi_hooks` | 事件名 → 一组 `{id, fn}` | `pi.on` / `pi.once` / `pi.off`；宿主投递事件时按名调用 `fn` |
| `__pi_tools` | 工具名 → 工具对象（含 `execute`） | `pi.registerTool`；宿主 `callTool` 时经 `__pi_execute_tool` 调到 JS |
| `__pi_commands` | 命令名 → `{ description, handler }` | `pi.registerCommand`；**handler 只能留在 JS**（函数不能过 hostcall） |
| `__pi_nextId` | 自增 id | 给本地 listener 等分配 id |

---

### Q2：`plugin_commands` 是什么？跟执行 bash 命令有什么区别？

**A：** **不是一类东西。**

- **`executeBash` / `pi.exec`**：在**操作系统**里起进程——要么 `sh -c "整串"`（shell 模式），要么 `Command::new(prog).args(argv)`（argv 模式），跑的是 **外部程序/shell 命令**。
- **`plugin_commands`（Rust `DashMap`）**：只存 **「该 Wasm 实例向宿主登记过的斜杠命令」的元数据**（名称 + 描述）。**不执行 bash**，也不等于 shell 里的 command。

可理解为插件对宿主说「我提供一个叫 X 的命令，说明是 Y」的**目录**；真正执行 handler 目前在同 VM 内用 `__pi_invoke_command`，将来可接到 CLI/Agent。

---

### Q3：`registerCommand` 是干嘛的？

**A：** 对齐 pi-mono：**注册一个由扩展提供的、可被 UI/CLI 触发的命名命令**，带 **描述** 和 **handler**。

在 tomcat 里拆成两步：

1. **JS**：把 `handler` 放进 `__pi_commands[name]`。  
2. **Host**：`hostCall('tools','registerCommand', { name, description })`，Rust 写入 `plugin_commands`。

因此它是 **「插件命令」注册**，不是注册 bash 别名，也不是 OS 层面的 `PATH` 命令。

---

### Q4：图 2 里两种模式是不是都是「执行 bash」？只是有参数和无参数的区别？

**A：** **不完全是。**

- **无 `args` 数组**：**Shell 模式**（如 Unix 上 `sh -c` + 一整条字符串）。口语上常说「bash」，实现上取决于配置的 shell。
- **有 `args` 数组**：**不是**把参数拼进一条 shell 字符串，而是 **`Command::new(command).args(argv)`**——**直接启动可执行文件 + argv**，**不经过 shell 解析**。

区别不仅是「有没有参数」，而是 **是否经过 shell**（管道、`$()`、通配等由谁解释）。

---

### Q5：图 3 里「`__pi_commands[name]` 与 `HostCall tools.registerCommand` 并排」是什么意思？

**A：** 表示 **`pi.registerCommand` 一次调用里并行发生的两件事**，不是一行联合代码：

1. **JS 侧**：`__pi_commands[name] = { description, handler }` —— **可执行逻辑**留在 QuickJS。  
2. **Host 侧**：只发 **`name` + `description`** —— **handler 是函数，无法 JSON 序列化**，不能通过 `__pi_host_call` 传给 Rust。

即 **跨语言边界时，数据被故意拆成两份**：Rust 只存目录信息，JS 存真正要跑的函数。

---

### Q6：报告 4.2 里「白名单覆盖两模式」「`Command::new` + `args`」怎么理解？

**A：**

- **白名单/黑名单/确认**：执行前同一套策略要对 **shell 模式** 和 **argv 模式** 都生效。Shell 模式从整串里取「第一个 token」；argv 模式没有整串，就用 **`command` 当作程序名** 参与比对 —— 这样 **策略不随执行路径分叉**。  
- **`Command::new` + `args`**：argv 模式下 **不经 shell、直接 spawn**，参数不会被 shell 二次解释，通常 **更安全、更可控**。

---

### Q7：「无 TTY 的 CI 也可断言」是什么意思？TTY 是什么？

**A：** **TTY**（终端）指交互式终端设备；人在终端里选菜单、确认、输入，往往依赖 TTY（或伪终端 pty）。

**CI** 通常 **没有真人、没有交互终端**。若 `ctx.ui.select` **真去等人按键**，测试会 **挂死或失败**。

当前 UI 路径是 **确定性桩**（如总选第一项、总确认 true），**不读 stdin**，故 **无 TTY 也能在自动化里断言返回 JSON 结构** —— 这就是这句话的含义。

---

### Q8：`registered_plugin_commands()` 到底干什么用？

**A：** `HostApiDispatcher::registered_plugin_commands(instance_id)` 返回该实例在 **`plugin_commands`** 里已登记的 **`Vec<(命令名, 描述)>`**。

- **不执行任何命令**，只 **查询元数据**。  
- **测试**：断言 `registerCommand` 后宿主侧确实有记录。  
- **产品演进**：CLI/Agent 列出插件提供了哪些命令时可复用。

---

### Q9：`pi_bridge.js` 里 `registerCommand` 那段（约 190–200 行）什么意思？什么时候会被调用？

**A：** 逻辑是：

1. `options = options || {}` 做默认值。  
2. `__pi_commands[name] = { description, handler }` —— **本地保存可执行 handler**。  
3. `hostCall('tools','registerCommand', { name, description })` —— **只把可序列化字段发给 Rust**。

**调用时机**：插件或测试代码执行 **`pi.registerCommand(...)`** 时触发，常见于插件 **入口初始化**（例如 `export default function (pi) { ... }` 里）。顺序是：先写 JS 表，再发一次宿主 RPC。

---
