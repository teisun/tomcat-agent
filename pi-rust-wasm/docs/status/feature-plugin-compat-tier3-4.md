### 元数据

| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Spike | 2026-03-22 | PENDING_INTEGRATION | feature/plugin-compat-tier3-4 | — |

### 任务

- [x] **[P2]** TASK-05d：pi-mono Tier 3-4（TUI 自定义组件 + 深度会话/模型 API + npm 包 import 基础设施）

### 子项进度

- [x] d.0 npm 包 import 基础设施（SWC 重写 + shim 注入）
- [x] d.1 `ctx.ui.custom()` + Container/SelectList/Text 兼容层
- [x] d.2 setWidget、setFooter、setHeader、editor
- [x] d.3 `ctx.sessionManager` 只读（含 getBranch 等）
- [x] d.4 `ctx.model` / `ctx.modelRegistry`
- [x] d.5 diff.ts E2E
- [x] d.6 files.ts E2E
- [x] d.7 固化为自动化 E2E

### INTERFACE

| 接口 | 位置 | 说明 |
| :--- | :--- | :--- |
| `ctx.ui.custom(factory)` | `pi_bridge.js` | 降级模式：调用 factory → render → hostCall uiCustom → done(undefined) |
| `ctx.ui.setWidget/setFooter/setHeader` | `pi_bridge.js` | MVP stub：记录日志后 ok |
| `ctx.ui.editor(title, prefill)` | `pi_bridge.js` + dispatcher | 无 TTY 返回 prefill |
| `ctx.sessionManager.getBranch(fromId?)` | `pi_bridge.js` + dispatcher | 委托 Rust SessionManager::get_branch |
| `ctx.sessionManager.getLeafEntry/getLeafId/getEntry/getHeader/getEntries` | `pi_bridge.js` + dispatcher | 同上模式 |
| `ctx.model` / `ctx.modelRegistry.getAll/getAvailable/getError` | `pi_bridge.js` + dispatcher | MVP：model 从 snapshot 或 getModel；listModels 返回空数组 |
| SWC npm import rewrite | `ts_compiler.rs` | `@mariozechner/pi-tui` 等 4 个包重写为 `globalThis.__xxx` |
| 4 个 shim JS | `assets/js/pi_{tui,coding_agent,ai,typebox}_shim.js` | 编译时 include_str! 注入 |

### BLOCKED

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |

### 验收自测

- `cargo test --lib` 282 passed, 0 failed
- `cargo test --test wasmedge_e2e_tests -- --test-threads=1` 22 passed, 0 failed
- E2E-WASM-039 (tier3_diff) 和 E2E-WASM-040 (tier4_files) 均通过
