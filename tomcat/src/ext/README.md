# 插件运行时层与插件生命周期（ext）

## 1. 概述

- **职责**：提供基于 `rquickjs` 的插件运行时、宿主导入绑定、Hostcall 分发、插件生命周期管理，以及长生命周期 session VM 的隔离与回收。
- **所在层级**：扩展层；依赖 `infra`，通过 trait/contract 接入 `core` 的工具、会话与执行器能力。
- **现状**：这里已经不再使用 WasmEdge、QuickJS wasm、`engine_stub.rs` / `instance_stub.rs` 或 `assets/modules/` 那套旧兼容层。

### 核心文件

- `src/ext/runtime/engine_config.rs`：`PluginEngineConfig` 与默认预算值（heap/timeout/interrupt/idle TTL）。
- `src/ext/runtime/engine.rs`：`PluginEngine`，负责全局引擎与实例创建。
- `src/ext/runtime/instance.rs`：`PluginVmInstance`，负责脚本拼装、宿主全局注入、短命执行与长命 session VM 启动。
- `src/ext/runtime/crypto_native.rs`：同步原生 crypto（hash/hmac/random/aes-gcm/ed25519）。
- `src/ext/host_binding.rs`：`HostRequest` / `HostResponse` 与 `invoke_host_func[_with]`。
- `src/ext/dispatcher/`：`HostApiDispatcher` 与各类 hostcall 路由。
- `src/ext/plugin/`：manifest、catalog、manager、tool executor。
- `src/ext/runtime_manager.rs`：按 `(session_id, plugin_id)` 管理 `VmActorHandle`，支持机会式 idle 回收。
- `src/ext/vm_actor.rs`：长生命周期插件 VM 的专属线程、状态机与命令通道。
- `src/ext/ts_compiler.rs`：TypeScript 零构建转译与受支持 import 重写。
- `assets/js/*.js`：运行时注入脚本（prelude / bridge / node alias / crypto shim / main loop 等）。

## 2. 运行时注入分层

`PluginVmInstance::build_combined_script()` 会把宿主脚本和用户脚本按固定顺序拼起来：

1. `pi_runtime_prelude.js`
   提供 `console`、`timers`、`TextEncoder`、`TextDecoder`、`path`、`util.format`、`events.EventEmitter`、`Buffer`。
2. `pi_crypto_shim.js`
   把 `crypto.*` 包装到 Rust 侧 `__pi_crypto_*_native`。
3. `pi_bridge.js`
   提供 `globalThis.pi`、`__pi_host_call` 包装、事件循环入口、工具执行桥。
4. `pi_node_shim.js`
   提供 `node:*` import alias；`node:fs` / `node:child_process` / `node:os` 走 fail-closed 拒绝桩。
5. `pi_typebox_shim.js` / `pi_ms_shim.js`
   仅保留轻量工具级兼容。
6. 用户脚本
7. `pi_main_loop.js`（仅 session VM 且用户脚本未显式调用 `__pi_start_event_loop()` 时追加）

Rust 侧 `install_host_globals()` 另外会注入：

- `__pi_host_call`
- `__pi_sleep`
- `__pi_wait_for_event`
- `__pi_budget_reset`
- `__pi_interrupt_reason`
- `__pi_crypto_*_native`

**边界原则**：

- 纯计算/纯工具能力尽量留在 VM 内完成。
- `crypto` 走同步原生，不进 dispatcher。
- 真正敏感的能力（文件、命令、会话、LLM、事件总线等）统一走 `pi.*` → `__pi_host_call` → `HostApiDispatcher`。

## 3. PluginManager 生命周期

### `load_plugin(path)`

完整加载链路如下：

1. 解析插件根目录与 manifest（`plugin.json`）。
2. 读取 `main`，必要时做 TS → JS 转译。
3. 执行 `confirm_permissions` 扩展点（**可选**；未注入时默认放行）。
4. 创建短生命周期 `PluginVmInstance`。
5. 注入 host binding。
6. 执行初始化脚本，登记工具/命令/事件副作用。
7. 注册到 `PluginManager`，进入可管理状态。

### `start_session_vm(session_id, plugin_id)`

长生命周期路径与 `load_plugin` 不同：

1. 先对 `PluginRuntimeManager` 执行一次**机会式 idle 回收**。
2. 若 `(session_id, plugin_id)` 已有健康 VM，则直接复用。
3. 否则创建新的 `PluginVmInstance`，注册事件通道与 host binding。
4. 通过 `VmActor::spawn()` 在专属线程启动长跑事件循环。

### `end_session(session_id)`

- 按 session 批量移除所有 `VmActorHandle`
- 发送 shutdown
- 清理 dispatcher 中该 session 的实例事件通道

## 4. 现在不再承诺的东西

- 不再承诺 WasmEdge / Wasmtime / QuickJS wasm 运行时路径。
- 不再承诺 `assets/modules/` 整套 Node 兼容层。
- 不再承诺 `@mariozechner/pi-tui`、`@mariozechner/pi-ai`、`@mariozechner/pi-coding-agent`、`@anthropic-ai/sandbox-runtime` 这类 pi-mono 生态包会被运行时自动注入。

`ts_compiler.rs` 当前只为以下 import 提供运行时可落地的重写：

- Tier-A `node:*` / `path` / `util` / `events` / `buffer` / `crypto`
- `@sinclair/typebox`
- `ms`

其它 legacy npm import 会保留为原始 import，让调用方显式感知“当前运行时不支持”。

## 5. 配置与回收语义

- `js_heap_mb`：单实例 QuickJS 堆上限；`0` 表示不设置显式上限。
- `call_timeout_ms`：单次 JS 执行片段软超时；`0` 表示禁用。
- `interrupt_budget`：单次执行 interrupt budget；`0` 表示禁用。
- `event_channel_capacity`：宿主投递到 session VM 的事件队列深度；`0` 会退化为同步交接。
- `idle_ttl_ms`：机会式回收阈值；**没有后台 sweeper**，只在后续插件活动或 `end_session()` 时顺手清理。
- `PI_PLUGIN_DISABLE`：在 chat 入口直接短路整套插件运行时初始化。

## 6. 相关测试

- `src/ext/tests/instance_shim_test.rs`
- `src/ext/tests/runtime_manager_test.rs`
- `src/ext/plugin/tests/suite_test.rs`
- `src/api/chat/tests/runtime_split_test.rs`
- `tests/quickjs_e2e_tests.rs`
- `src/api/cli/tests/plugin_cmd_test.rs`

阅读顺序建议：先看 `runtime/engine_config.rs` / `runtime/engine.rs` / `runtime/instance.rs`，再看 `plugin/manager.rs`、`runtime_manager.rs`、`vm_actor.rs`，最后看 `dispatcher/` 与对应测试。
