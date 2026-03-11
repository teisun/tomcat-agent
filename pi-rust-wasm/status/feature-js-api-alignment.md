| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tom | 2026-03-11 | DONE | feature/js-api-alignment | 60.98% |

### TASK-13 JS API 与 pi-mono 对齐（T1-P0-008-jsapi）

#### 子任务完成情况

- [✓] **8.7.1** `pi_bridge.js`：新增 `hostCallAsync` 函数（submit/poll 包装，返回 Promise），含 callId 生成、指数退避轮询逻辑（1ms→50ms 上限）
- [✓] **8.7.2** `pi_bridge.js`：`exec`/`createChatCompletion` 改为调用 `hostCallAsync`，返回 Promise，返回值解包为 pi-mono 格式（`ExecResult`/`CompletionResult`）
- [✓] **8.7.3** `pi_bridge.js`：修复 `off`/`emit` 重复定义 bug；`off` 合并为单一实现，支持按 handler 引用（pi-mono 风格）或 listenerId（向后兼容）两种取消模式
- [✓] **8.7.4** `pi_bridge.js`：新增 `pi.once(event, handler)` 单次监听方法
- [✓] **8.7.5** 集成测试：`js_api_async_test.js` 覆盖 `await pi.exec("echo hello")`，验证 Promise 正确 resolve 及 `{stdout, stderr, exitCode}` 格式
- [✓] **8.7.6** 集成测试：`js_api_async_test.js` 覆盖 `await pi.createChatCompletion({...})`，验证 Promise resolve/reject 行为正确
- [✓] **8.7.7** `pi_bridge.js`：`readFile`/`writeFile`/`editFile` 改为返回 Promise（同步结果包装为 `Promise.resolve/reject`，与 pi-mono 签名兼容）
- [✓] **8.7.8** `pi_bridge.js`：新增 `pi.setModel(model)`（返回 Promise）、`pi.getModel()`（同步）、`pi.complete(prompt, options)`（封装 createChatCompletion 返回 `Promise<string>`）、`pi.unregisterTool(name)`（同步）

#### 涉及文件

- `assets/js/pi_bridge.js`：主要改动（全部 8.7.x 子任务）
- `src/ext/dispatcher.rs`：新增 `("llm", "getModel")` 和 `("llm", "setModel")` 路由（MVP stub）
- `tests/fixtures/wasmedge_quickjs/bridge_test.js`：更新为 async/await 写法（兼容 Promise API）
- `tests/fixtures/wasmedge_quickjs/js_api_async_test.js`：新增异步 API 集成测试脚本（8.7.5/8.7.6）
- `tests/js_api_alignment_tests.rs`：新增 Rust 集成测试（两个测试用例）
- `agents/TASK_BOARD.md`：状态更新

#### 覆盖率

- `cargo tarpaulin --lib --package pi_wasm` 结果：**60.98%**（1491/2445 lines）
- 注：整体覆盖率因 `instance_wasmedge.rs`（WasmEdge E2E 路径，需真实 WasmEdge 环境）等文件为 0% 拉低；核心改动 `dispatcher.rs` 356/440 = 80.9%，`host_binding.rs` 12/12 = 100%

#### 风险与说明

- `llm.setModel`/`llm.getModel`：MVP stub，已确认记录；`LlmProvider` trait 未暴露模型名，后续扩展时在 dispatcher 中维护 per-instance 模型覆盖 map
- 3 个既有 clippy warning（`src/infra/config.rs`、`src/infra/logging.rs`）：非本次引入，未修改
