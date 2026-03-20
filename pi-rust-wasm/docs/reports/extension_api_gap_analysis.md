# ExtensionAPI 差距分析（TASK-05a / Phase 0）

**基准**：pi-mono `ExtensionAPI`（`packages/coding-agent/src/core/extensions/types.ts`）与 pi-rust-wasm `assets/js/pi_bridge.js` + `HostApiDispatcher`。  
**策略来源**：[pi-mono-compat-strategy.md](../openspec/specs/architecture/plugin-system/pi-mono-compat-strategy.md) §13.4、§13.5。

---

## 1. Phase 0 已验证能力

| 项 | 状态 | 说明 |
|----|------|------|
| TS→JS（SWC strip + codegen） | 已落地 | `src/ext/ts_compiler.rs`：`transpile_typescript`、`transpile_pi_plugin_for_quickjs` |
| QuickJS 脚本入口 | 已落地 | 将 `export default function` 改写为具名函数并调用 `__pi_plugin_default(globalThis.pi)`，规避脚本模式无 ESM |
| wasmedge-quickjs `modules/` | 已落地 | `assets/modules/` 自 wasmedge-quickjs 同步；`instance_wasmedge.rs` 增加 `./modules` preopen；环境变量 `PI_WASM_QUICKJS_MODULES_PATH` 可覆盖 |
| 代表扩展 POC | 已落地 | `tps.ts` 转译后在 `wasmedge_e2e_tests::test_wasmedge_e2e_tps_transpile_run_script_poc` 中加载 |

---

## 2. API 级差距表（相对 §13.4，补充实现备注）

| API | pi_bridge 现状 | 差距 | Tier | 备注 |
|-----|----------------|------|------|------|
| `on` / `off` / `once` | `on`/`off` 有；`once` 有 | `on` 的 handler 需与 pi-mono 一致传入 `(event, ctx)`；事件名映射待 TASK-05b | 1 | 宿主桩下 POC 仅需 subscribe 不崩溃 |
| `exec` | 有，签名为 `(cmd, args, opts)` | 与 pi-mono 对齐情况需对照 types.ts | 2 | |
| `registerTool` | 简化版 | TypeBox schema、工具生命周期与 pi-mono 对齐 | 2 | |
| `registerCommand` | 有 | handler / ctx 与 pi-mono 一致 | 2 | |
| `registerShortcut` / `registerFlag` / `getFlag` | 无 | 需新增或 stub | 2 | |
| `sendMessage` | 有 | 选项与语义对齐 | 2 | |
| `readFile` / `writeFile` / `editFile` | 有（宿主原语） | pi-mono 扩展通常不经 ExtensionAPI 暴露；保留为宿主能力 | — | |
| `setModel` / `getThinkingLevel` | 无 | 需新增 | 2 | |
| `registerProvider` / `registerMessageRenderer` | 无 | 需新增 | 4 | |
| `events`（独立 EventBus 句柄） | 无 | pi-mono 部分扩展使用 | 2 | |
| `ctx.hasUI` / `ctx.cwd` | 无 | Tier 1 必需 | 1 | |
| `ctx.ui.notify` / `select` / `confirm` / `input` | `notify` 等部分存在 | 需统一经 ctx 注入，与 pi-mono 签名一致 | 1–2 | |
| `ctx.ui.custom` / TUI 组件 | 无 | Tier 3 | 3 | |
| `ctx.sessionManager` / `ctx.model` / `ctx.modelRegistry` | 无 | Tier 4 | 4 | |
| `ctx.isIdle` / `abort` / `shutdown` | 无 | Tier 2 | 2 | |

---

## 3. 入口与模块解析

| 主题 | pi-mono | pi-rust-wasm Phase 0 | 后续 |
|------|---------|----------------------|------|
| 扩展入口 | `export default function(pi)` | SWC + 字符串包装调用 `globalThis.pi` | TASK-05b：`pi_bridge` 与加载器统一 |
| npm 说明符 | jiti 解析 node_modules | QuickJS 无 Node 解析；`import type` 可由 SWC 剥离，值导入需 Rust 虚拟模块或打包 | TASK-05c+ |

---

## 4. Node 兼容层（相对 §13.5）

- **已启用**：`assets/modules/` 与 WASI `./modules` 映射；`require('path')` 等内置模块在 E2E 中验证。
- **仍缺**：`child_process`、`https`、`module`（createRequire）、`net`/`tls` 等，按策略文档优先级通过 hostcall 或 stub 补齐。

---

## 5. 建议实施顺序（与 TASK-05b/c 对齐）

1. Tier 1：`export default` 与长生命周期 VM 加载路径统一；`ctx` 最小对象；事件名映射；`tps.ts` 级 E2E。
2. Tier 2：`exec` / `registerTool` / `registerCommand` / 基础 `ctx.ui` 与权限。
3. Tier 3–4：TUI `ctx.ui.custom`、会话与 Provider API。

---

**维护**：重大变更 `pi_bridge.js` 或 `HostApiDispatcher` 时更新本表与 [extension_compat_matrix.md](./extension_compat_matrix.md)。
