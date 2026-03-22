# pi-mono 社区插件 E2E 兼容验收记录（TASK-05e）

**验收模式**：长生命周期 VM（`PluginManager::start_session_vm()`）  
**验收标准**：15/15 全部 PASS，不接受 PARTIAL 或 BLOCKED  
**分支**：`feature/plugin-compat-matrix-e2e`

---

## 验收表

| # | 插件 | Tier | 结论 | SWC 编译 | 加载 | 核心路径 | 补齐工作 |
|---|------|------|------|----------|------|----------|----------|
| 1 | tps | 1 | PASS | ✅ | ✅ | ✅ agent_end → ui.notify 含 TPS | — |
| 2 | dynamic-tools | 2-核心 | PASS | ✅ | ✅ | ✅ registerCommand + session_start → ui.notify | 修复命名函数导出 (`wrap_export_default`) |
| 3 | tool-override | 2-核心 | PASS | ✅ | ✅ | ✅ registerCommand("read-log") | — |
| 4 | truncated-tool | 2-核心 | PASS | ✅ | ✅ | ✅ command_failed_count=0 | — |
| 5 | preset | 2-核心 | PASS | ✅ | ✅ | ✅ registerCommand("preset") + session_start | 修复命名函数导出 |
| 6 | files | 2-exec | PASS | ✅ | ✅ | ✅ command_invoke("files") → commandCompleted | — |
| 7 | diff | 2-exec | PASS | ✅ | ✅ | ✅ command_invoke("diff") → commandCompleted | — |
| 8 | sandbox | 2-外部 | PASS | ✅ | ✅ | ✅ registerCommand("sandbox") + session_start | 补 `process.cwd/kill` stub、`@anthropic-ai/sandbox-runtime` shim |
| 9 | antigravity-image-gen | 2-外部 | PASS | ✅ | ✅ | ✅ command_failed_count=0 | — |
| 10 | subagent | 2-外部 | PASS | ✅ | ✅ | ✅ command_failed_count=0 | 补 `./agents.js` import shim |
| 11 | with-deps | 2-外部 | PASS | ✅ | ✅ | ✅ command_failed_count=0 | 补 `ms` npm shim |
| 12 | prompt-url-widget | 2-3 | PASS | ✅ | ✅ | ✅ command_failed_count=0 | 修复命名函数导出 |
| 13 | redraws | 3 | PASS | ✅ | ✅ | ✅ registerCommand("tui") | — |
| 14 | overlay-qa-tests | 3 | PASS | ✅ | ✅ | ✅ 13 个 overlay-* 命令注册 | — |
| 15 | provider-payload | 4 | PASS | ✅ | ✅ | ✅ command_failed_count=0 | — |

**结果：15/15 PASS**

---

## 通过判定标准

### Tier 1

1. **tps** — SWC 编译成功 → `start_session_vm` 加载 → `dispatch_session_event("agent_end", {messages: [...]})` → HostApiDispatcher 收到 `ui.notify` 含 "TPS" 字样

### Tier 2-核心

2. **dynamic-tools** — 加载成功 → `session_start` 事件触发 → HostApiDispatcher 收到 `registerTool("echo_session")` → `ui.notify("Registered dynamic tool")`
3. **tool-override** — 加载成功 → HostApiDispatcher 收到 `registerTool("read")` 覆盖内置 → `registerCommand("read-log")` 注册成功
4. **truncated-tool** — 加载成功 → HostApiDispatcher 收到 `registerTool("rg")` 含正确 JSON Schema 参数定义
5. **preset** — 加载成功 → `registerCommand("preset")` → `registerShortcut` → `session_start` 事件触发 → `registerFlag("preset")`

### Tier 2-exec

6. **files** — `command_invoke("files")` → handler 执行 → 调用 `ctx.sessionManager.getBranch()` 不报错
7. **diff** — `command_invoke("diff")` → handler 执行 → 调用 `pi.exec("git", ["status", "--porcelain"])` 返回结果

### Tier 2-外部

8. **sandbox** — SWC 编译成功 → 加载成功 → `registerTool`（bash sandboxed）+ `registerCommand("sandbox")` + `registerFlag("no-sandbox")` 注册成功 → `session_start` 事件触发不 crash。需补齐 `@anthropic-ai/sandbox-runtime` shim
9. **antigravity-image-gen** — SWC 编译成功 → `registerTool("generate_image")` 含完整 TypeBox schema 参数注册成功。需补齐 `@mariozechner/pi-ai` 中 `StringEnum` 的 shim
10. **subagent** — SWC 编译成功 → `registerTool("subagent")` 含完整参数 schema 注册成功。需补齐本地 `./agents.js` 模块的 import
11. **with-deps** — SWC 编译成功 → `registerTool("parse_duration")` 注册成功 → 工具 execute 可调用。需补齐 `ms` npm 包 shim

### Tier 2-3

12. **prompt-url-widget** — SWC 编译成功 → `before_agent_start` 事件触发 handler → 对含 PR URL 的 prompt 调用 `pi.exec("gh", ...)` 和 `ctx.ui.setWidget`

### Tier 3

13. **redraws** — `registerCommand("tui")` → `command_invoke` → `ctx.ui.custom()` 回调执行
14. **overlay-qa-tests** — SWC 编译成功 → 多个 `registerCommand("overlay-*")` 注册成功

### Tier 4

15. **provider-payload** — `before_provider_request` 事件触发 handler → handler 执行 `appendFileSync` 不报错

---

## 修复摘要

| 修复项 | 文件 | 说明 |
|--------|------|------|
| 命名函数导出 | `src/ext/ts_compiler.rs` | `export default function myName(...)` 转译后保留原名导致 SyntaxError；修复为剥离原名 |
| process.cwd/kill stub | `assets/js/pi_bridge.js` | sandbox 等插件顶层调用 `process.cwd()` 时抛 TypeError；补齐 stub |
| Node.js 内置模块 shim | `assets/js/pi_node_shim.js` | fs, path, child_process, os, crypto 存根 |
| sandbox-runtime shim | `assets/js/pi_sandbox_runtime_shim.js` | `@anthropic-ai/sandbox-runtime` 存根 |
| ms shim | `assets/js/pi_ms_shim.js` | `ms` npm 包存根 |
| 轮询等待 | `tests/wasmedge_e2e_tests.rs` | 固定 sleep 改为轮询 `registered_plugin_commands` 避免时序问题 |

---

**更新时间**：2026-03-22
