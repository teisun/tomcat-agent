# 11. 异步 Hostcall 与事件循环设计

本文为 [Architecture](../../Architecture.md) 中第 11 节的详细设计，总览见主文档。

---

## 11.1 问题陈述

### 当前调用链路（全链路同步阻塞）

```
JS 插件: pi.exec("ls")
  → pi_bridge.js: hostCall("fs", "executeBash", {...})
    → __pi_host_call(requestJson)                       // 同步 Wasm 导入
      → host_call_impl (instance_wasmedge.rs)           // 同步 host 函数
        → invoke(&request_json)
          → dispatcher.dispatch()
            → rt = Runtime::new(); rt.block_on(dispatch_async(...))  // 阻塞！
              → do_execute_bash().await                  // 实际执行
            ← 结果返回
          ← JSON 响应
        ← 写入线性内存
      ← 响应长度
    ← JSON.parse(response)
  ← 返回给插件
```

**瓶颈**：`dispatcher.dispatch()` 每次新建 `tokio::runtime::Runtime` 并 `block_on`，整个 Wasm 实例在 hostcall 期间被阻塞。

**影响**：
- 插件调用 LLM（耗时数秒到数十秒）时，Wasm 实例完全挂起
- 多个插件无法真正并发——每个 hostcall 独占一个线程
- 与 pi-mono / openclaw 的 async/await 编程模型不兼容（pi-mono 的 `pi.exec()` 返回 `Promise<ExecResult>`，插件用 `await` 等待）

### pi 生态兼容性要求

| 生态 | LLM 调用 | exec/文件操作 | 工具 execute | 整体模式 |
|------|---------|-------------|------------|---------|
| pi-mono | `await complete(...)` | `await pi.exec(...)` | `async execute(...)` | async/await 为主 |
| openclaw | `await runEmbeddedPiAgent(...)` | `api.runtime.*` 均异步 | `async execute(...)` | async/await 为主 |

---

## 11.2 设计目标

1. **耗时 Hostcall 不阻塞**：LLM、网络、文件操作等不阻塞 Wasm 实例执行
2. **pi 生态兼容**：`pi.exec()` 等 API 返回 Promise，插件可用 `async/await`
3. **跨平台**：macOS / Linux / Windows 均可运行，不依赖 WasmEdge 平台限定 API
4. **渐进式**：同步 API 保持现状不破坏，仅耗时 API 走异步路径
5. **最小改动**：复用已有的 `__pi_host_call`、`HostRequest/HostResponse`、`callId` 字段

---

## 11.3 方案选型与决策记录

### 候选方案

| 方案 | 描述 | 可行性 |
|------|------|--------|
| WasmEdge native async | 使用 `wasmedge-sdk` 的 `async_host_function` | 排除：`#[cfg(target_os = "linux")]` 门控，macOS 不可用 |
| 宿主侧自建事件循环 | Rust 侧实现 pending 队列 + 回调重入 | 排除：需要长生命周期 VM，改动大 |
| `pi_eval_js` 长生命周期 VM | 定制 wasmedge_quickjs 导出新函数 | Phase 2：架构干净但需要 VM 生命周期重构 |
| **复用 `__pi_host_call` + submit/poll** | 利用 wasmedge_quickjs 已有事件循环 | **MVP 选定**：零 Wasm 改动，最小侵入 |

### 决策理由

**选择"复用 `__pi_host_call` + submit/poll"**，核心理由：

1. **wasmedge_quickjs 已有完整事件循环**：支持 `setTimeout`、`Promise`、`async/await`，`_start` 会运行事件循环直到所有异步任务完成——不需要宿主自建事件循环
2. **wasmedge_quickjs.wasm 零改动**：复用同一个 `__pi_host_call` Wasm 导入，通过 `module/method` 路由区分同步和异步
3. **协议层零改动**：`HostRequest.callId` 字段已预留，`HostResponse` 已有 `ok/data/error/callId`，只需约定新的 `module: "__async"` 路由
4. **改动集中在 3 个文件**：`dispatcher.rs`、`pi_bridge.js`、`instance_wasmedge.rs`

### Phase 2 演进方向

MVP 验证后，Phase 2 通过长生命周期 VM 解决跨调用持久状态问题（全局变量、会话级定时器、`session_start` 初始化数据等），完整支持 pi-mono 生态核心插件（git-checkpoint、todo、plan-mode、ssh 等）。两种候选方案（`pi_eval_js` 导出 vs `waitForEvent` 阻塞等待）详见 [Phase 2 长生命周期 VM 方案设计](phase2-long-lived-vm.md)，**推荐方案 B（`waitForEvent`）**。

---

## 11.4 MVP 方案详细设计

### 11.4.1 总体架构

```
JS 插件: await pi.exec("ls")
  → pi_bridge.js: hostCallAsync("fs", "executeBash", {...})
    ┌──────────────── 提交阶段（同步，微秒级） ────────────────┐
    │ __pi_host_call({module:"fs", method:"executeBash",       │
    │                 params:{command:"ls"}, callId:"req-42"}) │
    │   → host_call_impl → dispatcher                          │
    │     → 识别 callId 非空 → spawn Tokio 任务                │
    │     → 立即返回 {ok:true, data:{pending:true},            │
    │                  callId:"req-42"}                         │
    └──────────────────────────────────────────────────────────┘
    ← 返回 Promise（内部启动 setTimeout 轮询）
    
    ┌──────────── QuickJS 事件循环自动驱动 ────────────┐
    │ setTimeout 触发 → poll:                          │
    │   __pi_host_call({module:"__async",              │
    │                   method:"poll",                  │
    │                   params:{callId:"req-42"}})     │
    │   → host 检查共享 map → 未就绪 → 返回 ready:false│
    │   → setTimeout(poll, 1) 再试                     │
    │                                                   │
    │ ... Tokio 后台任务完成，结果写入共享 map ...       │
    │                                                   │
    │ setTimeout 触发 → poll:                          │
    │   → host 检查共享 map → 就绪！                    │
    │   → 返回 {ready:true, result:{stdout:"...",      │
    │           stderr:"", exitCode:0}}                 │
    │   → Promise resolve                              │
    └──────────────────────────────────────────────────┘
    
  ← await 继续，插件拿到结果
  ... 无更多 pending 任务 → _start 自然退出
```

### 11.4.2 协议设计

**复用已有 HostRequest/HostResponse**，无结构变更：

#### 异步提交请求

请求：标准 HostRequest，但 `callId` 非空。

```json
{
  "module": "fs",
  "method": "executeBash",
  "params": { "command": "ls" },
  "callId": "req-42"
}
```

响应：`data.pending` 为 `true` 表示已提交异步任务。

```json
{
  "ok": true,
  "data": { "pending": true },
  "callId": "req-42"
}
```

#### 轮询结果

请求：使用保留模块 `__async`。

```json
{
  "module": "__async",
  "method": "poll",
  "params": { "callId": "req-42" }
}
```

响应（未就绪）：

```json
{ "ok": true, "data": { "ready": false } }
```

响应（已就绪）：

```json
{
  "ok": true,
  "data": {
    "ready": true,
    "result": { "stdout": "file1.txt\nfile2.txt", "stderr": "", "exitCode": 0 }
  }
}
```

响应（异步执行出错）：

```json
{
  "ok": false,
  "error": "executeBash: command timed out",
  "callId": "req-42"
}
```

#### 同步/异步 API 分类

| 分类 | module/method | 走同步（无 callId） | 走异步（有 callId） |
|------|-------------|-------------------|-------------------|
| 日志 | `agent.log/info/warn/error/debug` | 是 | — |
| 配置 | `context.*` | 是 | — |
| 事件注册 | `events.on/once/off` | 是 | — |
| 事件发布 | `events.emit` | 是 | — |
| 工具注册 | `tools.registerTool/registerCommand` | 是 | — |
| 文件读取 | `fs.readFile` | 可选 | 可选 |
| 文件写入 | `fs.writeFile/editFile` | 可选 | 可选 |
| 命令执行 | `fs.executeBash` | — | **是** |
| LLM 调用 | `llm.createChatCompletion` | — | **是** |
| LLM 流式 | `llm.createChatCompletionStream` | — | **是** |
| 工具调用 | `tools.callTool` | — | **是** |
| 会话消息 | `session.getMessages` | 是 | — |
| 轮询 | `__async.poll` | 是（固有） | — |

**规则**：请求中 `callId` 为空或不存在时走同步路径（现有行为不变）；`callId` 非空时走异步路径。

### 11.4.3 宿主侧改动（Rust）

#### dispatcher.rs：新增异步任务管理

```rust
use std::sync::Arc;
use dashmap::DashMap;
use tokio::runtime::Handle;

pub enum AsyncCallStatus {
    Pending,
    Done(HostResponse),
    Error(String),
}

pub struct HostApiDispatcher {
    // ... 现有字段 ...
    async_results: Arc<DashMap<String, AsyncCallStatus>>,
    tokio_handle: Handle,  // 共享 Tokio runtime handle
}
```

`dispatch_async` 改动逻辑：
1. 检查 `request.call_id`：为空走现有同步路径
2. `call_id` 非空：spawn 任务到 `tokio_handle`，结果写入 `async_results`，立即返回 `{pending: true}`
3. `module == "__async" && method == "poll"`：从 `async_results` 查结果

#### instance_wasmedge.rs：共享 Tokio Runtime

`dispatch()` 不再每次 `Runtime::new().block_on()`，改为使用宿主全局共享的 Tokio runtime handle。

### 11.4.4 JS 侧改动（pi_bridge.js）

新增 `hostCallAsync` 函数：

```javascript
var __callSeq = 0;
var POLL_INTERVAL_MS = 1;
var POLL_MAX_INTERVAL_MS = 50;

function hostCallAsync(module, method, params) {
    var callId = '__call_' + (++__callSeq) + '_' + Date.now();
    var req = JSON.stringify({
        module: module, method: method,
        params: params || {}, callId: callId
    });
    var submitRes = __pi_host_call(req);
    var parsed = typeof submitRes === 'string' ? JSON.parse(submitRes) : submitRes;

    if (!parsed.ok) {
        return Promise.reject(new Error(parsed.error || 'hostcall failed'));
    }
    if (!parsed.data || !parsed.data.pending) {
        return Promise.resolve(parsed);
    }

    return new Promise(function (resolve, reject) {
        var interval = POLL_INTERVAL_MS;
        function poll() {
            var pollReq = JSON.stringify({
                module: '__async', method: 'poll',
                params: { callId: callId }
            });
            var pollRes = __pi_host_call(pollReq);
            var pr = typeof pollRes === 'string' ? JSON.parse(pollRes) : pollRes;

            if (!pr.ok) {
                reject(new Error(pr.error || 'async poll error'));
                return;
            }
            if (pr.data && pr.data.ready) {
                resolve({ ok: true, data: pr.data.result });
                return;
            }
            interval = Math.min(interval * 2, POLL_MAX_INTERVAL_MS);
            setTimeout(poll, interval);
        }
        setTimeout(poll, POLL_INTERVAL_MS);
    });
}
```

耗时 API 改为使用 `hostCallAsync`：

```javascript
exec: function (command, args, options) {
    return hostCallAsync('fs', 'executeBash', {
        command: command, args: args, cwd: options && options.cwd
    });
},

createChatCompletion: function (params) {
    return hostCallAsync('llm', 'createChatCompletion', params);
},
```

### 11.4.5 wasmedge_quickjs 事件循环驱动

**无需任何改动**。wasmedge_quickjs 的 `_start` 内部事件循环会自动：

1. 执行 JS 代码（pi_bridge.js + 插件代码）
2. 遇到 `setTimeout(poll, 1)` → 注册到 EventLoop 的 immediate_queue
3. 运行 `run_loop_without_io()`：执行 `JS_ExecutePendingJob`（Promise 微任务）+ `run_tick_task()`（setTimeout 回调）
4. 有 pending Promise → 事件循环继续运行
5. 所有 Promise resolve + 无 pending setTimeout → `_start` 自然返回

---

## 11.5 错误处理与超时

### 异步 Hostcall 超时

宿主侧为每个异步任务设置超时（可配置，默认 30 秒）：

```rust
let result = tokio::time::timeout(
    Duration::from_secs(30),
    self.dispatch_async(instance_id, request),
).await;
match result {
    Ok(resp) => async_results.insert(call_id, AsyncCallStatus::Done(resp)),
    Err(_) => async_results.insert(call_id, AsyncCallStatus::Error("timeout".into())),
}
```

### Promise reject 映射

JS 侧在 poll 时检测到错误，通过 `reject(new Error(...))` 传递给插件的 `try/catch` 或 `.catch()`。

### 实例销毁时的清理

当 WasmInstance 销毁时，清除该实例的所有 pending 异步任务：

```rust
impl Drop for WasmInstance {
    fn drop(&mut self) {
        // 清理 async_results 中属于本实例的所有条目
    }
}
```

---

## 11.6 并发模型

### 单实例内并发

一个插件可以同时发起多个异步 hostcall（多个 callId 同时 pending），QuickJS 事件循环会交替轮询它们。

### 多实例间并发

多个插件各自运行在独立 Wasm 实例中，各自的 `_start` 在不同线程上执行。宿主侧共享同一个 Tokio runtime，异步任务在 Tokio 线程池中并行处理。

### 资源竞争

- `async_results`（`DashMap`）：无锁并发读写
- Session 读写：通过现有 `Arc<RwLock>` / 分片锁解决
- LLM 并发限制：通过 `Semaphore` 控制最大并发请求数

---

## 11.7 Phase 2 演进：长生命周期 VM

MVP 验证后，Phase 2 通过让 VM 实例在整个会话期间存活，解决 MVP 短生命周期 VM 无法支持的核心场景：**跨调用持久状态**（插件全局变量、`setInterval` 会话级定时器、`session_start` 初始化数据跨事件共享）。

收益：
- 跨调用状态保持（插件全局变量、注册的 handler 持久存在）
- `setInterval` 等持久定时器正确运行
- 更高效的事件分发（无需每次重新执行插件脚本）
- 零轮询延迟（结合方案 B，事件驱动替代 setTimeout 轮询）

有两种实现路线，详细设计、代码示例、优缺点对比及推荐选择见子文档：

**[Phase 2 长生命周期 VM 方案设计（详细）](phase2-long-lived-vm.md)**

- **方案 A：`pi_eval_js` Wasm 新导出**——宿主主动调用 Wasm 导出函数注入 JS 代码；需改造 wasmedge-quickjs 全局 Context，含 `unsafe` 代码
- **方案 B：`waitForEvent` 阻塞等待（推荐）**——复用现有 `__pi_host_call`，JS 侧运行事件循环，宿主侧采用 VM actor + event channel 驱动；Tokio 天然兼容

### 11.7.1 两步改造路径（定版）

1. **结构改造（先做）**
   - 将“每次执行新建 VM”改为“长寿命运行单元”。
   - 解耦 VM 启动与事件分发，预留运行时状态机。
2. **事件驱动（后做）**
   - 引入 `event_tx/rx + spawn_blocking`，事件通过 channel 投递。
   - `_start` 常驻循环，但通过 `session_end/shutdown` 可控退出。

### 11.7.2 与 submit/poll 的职责边界

- `waitForEvent`：负责“取事件 + 触发 handler”。
- `submit/poll`：负责 handler 内耗时 hostcall（LLM、executeBash、tools.callTool）的异步执行。
- Phase 2 不引入第二套异步协议；继续复用 `callId + __async.poll`。

### 11.7.3 作用域与并发口径

- VM 运行时作用域采用 `session_id + plugin_id`（或 `session_id -> plugin runtimes`）模型。
- 同一插件可在不同会话拥有独立 VM，避免跨会话状态污染。
- 外部不直接并发操作可变 `Vm`，统一通过 VM actor 命令通道（`Init/DispatchEvent/Shutdown`）。

### 11.7.4 实现状态（待实现）

- [ ] 结构改造：长寿命运行单元 + 启动/分发解耦
- [ ] 事件驱动：event channel + spawn_blocking + shutdown 协议
- [ ] 作用域升级：`session_id + plugin_id` 运行时键
- [ ] 协同验证：waitForEvent 与 submit/poll 一致性

---

## 11.8 与现有模块的关系

| 模块 | 关系 |
|------|------|
| [host-api-layer.md](host-api-layer.md) 3.3.2 | 本文替代原"WasmEdge 异步转译"方案 |
| [wasmedge-runtime-layer.md](wasmedge-runtime-layer.md) 4.4 | 补充异步逃生通道的具体实现 |
| [js-bridge-layer.md](js-bridge-layer.md) | pi_bridge.js 新增 hostCallAsync + API Promise 化 |
| [host-call-protocol.md](host-call-protocol.md) | 新增 `__async.poll` 模块路由与 callId 使用规范 |
| [js-api-alignment.md](js-api-alignment.md) | 异步化是 API 对齐的前置依赖 |

---

**导航**：返回 [插件系统全貌](../plugin-system-overview.md) | 上一节：[沙箱执行层](sandbox-layer.md) | 下一节：[事件系统设计](events.md)
