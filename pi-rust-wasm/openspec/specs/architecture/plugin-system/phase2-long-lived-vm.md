# 11.7 Phase 2 演进：长生命周期 VM 方案设计

本文为 [async-hostcall-event-loop.md](async-hostcall-event-loop.md) 第 11.7 节的详细设计。

---

## 背景与动机

MVP 的短生命周期 VM 方案能满足大多数"无状态"插件的 async/await 需求，但 pi-mono 生态中大量核心插件依赖**跨调用持久状态**，例如：

| 插件 | 跨调用状态 |
|------|-----------|
| `git-checkpoint` | `checkpoints: Map`、`currentEntryId`，跨 `tool_result`/`turn_start` |
| `todo` | `todos: Todo[]`、`nextId`，工具调用间持久存储 |
| `plan-mode` | `planModeEnabled`、`todoItems[]`，跨多个事件钩子 |
| `ssh` | `resolvedSsh`，`session_start` 解析后供所有工具使用 |
| `mac-system-theme` | `setInterval` 会话级持久定时器（2s 轮询暗色模式） |
| `titlebar-spinner` | `setInterval` 会话级动画定时器（80ms） |
| `claude-rules` | `ruleFiles`，session 初始化后跨 `before_agent_start` 读取 |

**短生命周期 VM 的根本限制**：每次事件触发都新建 VM、重跑全量插件代码，全局变量每次清零，`setInterval` 无法跨调用持续运行。

Phase 2 的目标是让 VM 实例在整个会话期间存活，插件的 JS 运行时状态跨多次事件调用保持不变。

---

## 收敛定版（本轮）

本轮技术方案统一为以下口径（用于后续实现）：

1. **两步改造（低风险）**
   - 第一步（结构改造）：将“每次执行新建 VM”改为“长寿命运行单元”，拆分启动与事件分发。
   - 第二步（事件驱动）：引入 `event_tx/rx + spawn_blocking`，由 channel 投递事件驱动 `_start` 常驻循环。
2. **主选方案 B（Actor 化）**
   - 方案 B 保持主选，方案 A 保留为备选。
   - VM 采用 actor 模型：VM 封装在专属执行线程，外部只发送命令（`Init` / `DispatchEvent` / `Shutdown`）。
3. **会话维度多 VM**
   - 实例作用域采用 `session_id + plugin_id` 双键（或 `session_id -> plugin runtimes`）。
   - 不采用“单 plugin_id 全局唯一 VM”模型。
4. **`_start` 常驻但生命周期可控**
   - 启动：`session_start` lazy create/lazy start。
   - 退出：`session_end` 或 `shutdown` 显式关闭。
   - 空闲：`blocking_recv()` 挂起线程，不 busy-loop。

### ASCII 核心四图

#### 1) 结构图（Host + VM Actor + 通道）

```text
┌──────────────────────────────────────────────────────────────────────┐
│ Host Runtime                                                         │
├──────────────────────────────────────────────────────────────────────┤
│ EventDispatcher                                                      │
│   └─ runtime_manager.get(session_id, plugin_id)                      │
│        └─ VmActorHandle                                               │
│            ├─ cmd_tx: Init/DispatchEvent/Shutdown                    │
│            └─ state: Created/Running/Idle/...                        │
│                                                                      │
│ Tokio Async Pool  <---- submit/poll async hostcalls ----> Dispatcher │
│ SpawnBlocking Pool <---- VM actor thread (_start + waitForEvent)     │
└──────────────────────────────────────────────────────────────────────┘
```

#### 2) 调用流图（事件进入 VM）

```text
DispatchEvent(session_id, plugin_id, event)
    -> RuntimeManager.lookup(key)
        -> miss ? lazy_init_actor() : reuse_actor()
            -> cmd_tx.send(DispatchEvent(event))
                -> VM thread wakes
                    -> JS handler executes
                        -> await pi.exec()/llm -> submit/poll
```

#### 3) 时序图（可控生命周期）

```text
session_start
  -> lazy create actor
  -> Init
  -> _start enters event loop

event_n
  -> DispatchEvent
  -> waitForEvent returns event
  -> handler run
  -> loop back to waitForEvent

session_end / shutdown
  -> send Shutdown
  -> loop exits
  -> actor stopped + resources cleaned
```

#### 4) 闭环图（事件 -> 状态 -> 输出）

```text
Event In
  -> VM Actor
    -> JS Global State Mutation
      -> Async Hostcall (submit/poll)
        -> ToolResult/Event Out
          -> Host Dispatcher / Agent Loop
```

---

## 两种候选方案

### 方案 A：`pi_eval_js` Wasm 新导出（Host → Guest）

**核心思路**：修改 wasmedge-quickjs，把 `Runtime`/`Context` 存入全局变量，暴露一个 Wasm 导出函数供宿主调用。

#### 调用方向

```
方案 A 调用方向：

宿主 → vm.run_func("pi_eval_js", [code])     ← 宿主主动调用 Wasm 导出
                    │
              QuickJS 上下文执行 code
              （上下文永久存活于全局变量）
```

#### wasmedge-quickjs 改动

```rust
// src/main.rs：改造前 Runtime 在 main() 栈上，main() 退出即销毁
// 改造后：泄漏到全局静态变量
static mut RUNTIME: Option<Runtime> = None;
static mut CONTEXT_PTR: *mut Context = std::ptr::null_mut();

#[no_mangle]
pub extern "C" fn pi_eval_js(code_ptr: i32, code_len: i32) -> i32 {
    let code = unsafe {
        let slice = std::slice::from_raw_parts(code_ptr as *const u8, code_len as usize);
        std::str::from_utf8_unchecked(slice)
    };
    let ctx = unsafe { &mut *CONTEXT_PTR };
    match ctx.eval_buf(code.as_bytes().to_vec(), "<eval>", 0) {
        JsValue::Exception => -1,
        _ => 0,
    }
}
```

#### 宿主侧事件分发改造

```rust
// 分发事件：宿主调用 pi_eval_js 注入代码
let js_code = format!(
    r#"__pi_dispatch_event('{}')"#,
    event_json.replace('\'', "\\'")
);
vm.run_func(
    Some("quickjs"),
    "pi_eval_js",
    [WasmValue::from_i32(code_ptr), WasmValue::from_i32(code_len)],
)?;
```

#### 优缺点

| 项目 | 评价 |
|------|------|
| 宿主控制权 | 强：宿主可以随时注入任意 JS |
| wasmedge-quickjs 改动 | 较大：全局 Context、`#[no_mangle]` 导出、需重新编译 .wasm |
| 线程模型 | 友好：宿主按需调用，VM 空闲时不占用线程 |
| 安全性 | 需处理全局静态变量的内存安全（`unsafe`） |
| Tokio 兼容 | 需要解决 `current_thread` runtime 在 `pi_eval_js` 内多次重入的问题 |

---

### 方案 B：`waitForEvent` 阻塞等待（复用 `__pi_host_call`，推荐）

**核心思路**：`_start` 不退出，JS 侧运行一个无限事件循环，每次通过现有 `__pi_host_call` 阻塞等待宿主推送事件。

> 定版补充：方案 B 在实现层采用 **VM actor 模型**，避免外部并发直接持有可变 `Vm`。

#### 调用方向

```
方案 B 调用方向：

JS（Wasm 内）→ __pi_host_call("waitForEvent") → 宿主 blocking_recv()
                                                        ↑
                                               宿主 event_tx.send(event)
                                               （从任意 Tokio 任务触发）
```

#### pi_bridge.js 改动（事件循环）

```javascript
// pi_bridge.js 末尾追加（Phase 2 模式，替换当前的单次执行）
(async function sessionEventLoop() {
    while (true) {
        // 阻塞等待宿主推送下一个事件（同步 hostcall，宿主端 blocking_recv）
        var res = JSON.parse(__pi_host_call(JSON.stringify({
            module: '__session',
            method: 'waitForEvent'
        })));

        if (!res.ok || (res.data && res.data.type === 'shutdown')) {
            break;  // 会话结束，_start 正常退出
        }

        // 分发事件，handler 里的 await pi.exec() 等走 submit/poll
        try {
            await __pi_dispatch_event_async(JSON.stringify(res.data));
        } catch (e) {
            try { pi.log('sessionEventLoop error: ' + e); } catch (_) {}
        }
        // 继续循环，QuickJS 事件循环在每次 await 间隙自动驱动微任务
    }
})();
```

#### wasmedge-quickjs 改动（host_call.rs 新增路由）

```rust
// host_call.rs（wasmedge-quickjs 内）：无需新增 Wasm 导出
// 只需在宿主侧 dispatcher.rs 新增 "__session.waitForEvent" 路由
```

#### 宿主侧实现

```rust
// dispatcher.rs：新增 waitForEvent 处理
"__session" | "waitForEvent" => {
    // 阻塞等待 event channel（注意：此函数在 spawn_blocking 线程内调用）
    let event = instance
        .event_rx
        .blocking_recv()
        .ok_or_else(|| HostCallError::Closed)?;
    HostResponse::ok(serde_json::to_value(event)?)
}

// instance_wasmedge.rs：VM 生命周期改造
// 每个插件的 _start 在独立 spawn_blocking 线程中运行
let vm_handle = tokio::task::spawn_blocking(move || {
    vm.run_func(Some("quickjs"), "_start", args)
});

// 分发事件：直接发送到 channel，waitForEvent 会取走
instance.event_tx.send(event).await?;
```

#### 方案 B 的 Actor 化落地（定版）

```rust
enum VmCommand {
    Init,
    DispatchEvent(Event),
    Shutdown,
}

type RuntimeKey = (SessionId, PluginId);

struct VmActorHandle {
    cmd_tx: mpsc::Sender<VmCommand>,
    state: Arc<RwLock<VmLifecycleState>>,
}
```

- `RuntimeManager` 使用 `RuntimeKey(session_id, plugin_id)` 管理 VM actor。
- VM thread 内独占 `Vm`，外部不直接借用可变 `Vm`。
- `DispatchEvent` 仅负责投递事件；实际执行在 VM actor 线程完成。

#### 线程模型

Tokio 内部有**两套完全独立的线程池**，`spawn_blocking` 使用专用的 blocking 池，不占用 async worker 线程：

```
Tokio Runtime
│
├─ Async Worker 线程池（少量，默认 = CPU 核心数，如 8 个）
│    ├─ 处理所有 .await 任务（LLM 响应、event_tx.send、__async.poll...）
│    ├─ 绝对不能阻塞，否则整个系统卡死
│    └─ VM 线程不在这里，始终空闲
│
└─ spawn_blocking 线程池（独立，默认最多 512 个）
     ├─ Plugin A：_start 阻塞在 blocking_recv() ← OS 挂起，CPU≈0
     ├─ Plugin B：_start 阻塞在 blocking_recv() ← OS 挂起，CPU≈0
     └─ ...（每个插件一个，互不影响）
```

**event_tx.send(event)** 是 async 操作，运行在 async worker 线程上，发完即返回；**blocking_recv()** 是 blocking 操作，只在 `spawn_blocking` 线程上等待，两者完全隔离。

> Tokio 官方建议：任何可能阻塞超过 10–100μs 的操作都应通过 `spawn_blocking` 移出 async worker 线程。这正是 `spawn_blocking` 的设计目的。

```rust
// ❌ 错误：直接在 async 任务里阻塞，会卡住一个 async worker
async fn bad() { vm.run_func(...); }

// ✅ 正确：移入 spawn_blocking，blocking 池处理，async worker 不受影响
async fn good() {
    tokio::task::spawn_blocking(|| vm.run_func(...)).await;
}
```

**实际资源消耗**：阻塞等待 channel 的线程被 OS 挂起，CPU 消耗接近零，每个线程仅占约 8MB 栈内存。20 个插件 = ~160MB 栈 + 8 个 async worker 完全空闲。Tokio 的 `spawn_blocking` 池默认上限 512 线程，对 pi-mono/openclaw 典型场景（5–20 个插件）完全够用。

#### 优缺点

| 项目 | 评价 |
|------|------|
| wasmedge-quickjs 改动 | **极小**：仅在 `host_call.rs` 或 `dispatcher.rs` 新增路由处理 |
| pi_bridge.js 改动 | 中等：末尾加无限事件循环，替换当前单次执行模式 |
| 线程模型 | 每插件一个阻塞线程；idle 线程 CPU≈0，内存约 8MB/个 |
| 宿主侧 | 新增 `event_tx/rx` channel 对；VM 用 `spawn_blocking` 启动 |
| 安全性 | 无需 `unsafe`，无全局静态变量 |
| Tokio 兼容 | 天然兼容：`spawn_blocking` 是 Tokio 的标准阻塞 I/O 模式 |
| submit/poll 复用 | 完全复用 MVP 的异步 Hostcall 机制 |

---

## wasmedge-sdk 层面的验证

经过对 wasmedge-sdk 0.13.5-newapi 源码的验证，**Vm 天然支持长生命周期和多次 `run_func` 调用**：

- `Vm::run_func(mod_name, func_name, args)` 每次调用从 Store 中取实例和 executor，没有"调用一次后失效"的逻辑
- 只要 `Vm` 和 `Store` 不被 drop，就可以在同一实例上反复调用不同导出函数
- SDK 官方测试中也有多次 `run_func` 的用法

**当前短生命周期不是 wasmedge-sdk 的限制，而是 pi-rust-wasm 自己选择的**。`instance_wasmedge.rs` 里每次 `run_script_file_impl` 都新建 `Vm`，执行完就丢弃。

Phase 2 宿主侧改造要点：将持久化运行时提升为“会话维度 runtime manager + VM actor”：

```rust
pub struct RuntimeManager {
    // session_id + plugin_id -> VM actor
    runtimes: DashMap<RuntimeKey, VmActorHandle>,
}
```

对方案 B：`_start` 在 actor 线程中常驻事件循环，直到接收 `Shutdown`。

对方案 A：需要 wasmedge-quickjs 新增 `pi_eval_js` Wasm 导出，然后宿主通过 `vm.run_func(Some("quickjs"), "pi_eval_js", [...])` 多次调用。

---

## 线程内存真实开销分析

"每个 VM 占 8MB 内存"是对虚拟地址空间的误读。实际物理内存消耗远低于此：

**OS demand paging 机制**：线程栈的 8MB 是虚拟地址空间预留，但只有真正写入过的内存页（4KB/页）才映射到物理 RAM。一个阻塞在 `blocking_recv()` 上的线程，调用栈只有几层函数帧，物理占用约 **4–16KB**。

```
虚拟地址空间（8MB/线程）           物理 RAM（实际占用）
┌─────────────────────────┐
│  未触及的页面（~7.99MB）  │ ──→ 零物理开销（OS 不分配）
├─────────────────────────┤
│  实际栈帧（~4-16KB）      │ ──→ 4-16KB 物理 RAM
│  blocking_recv 只需几层   │
└─────────────────────────┘
```

| 插件数量 | 虚拟地址空间 | 实际物理 RAM |
|---------|------------|------------|
| 5 个 | 40MB（虚拟） | ~80KB |
| 20 个 | 160MB（虚拟） | ~320KB |
| 100 个 | 800MB（虚拟） | ~1.6MB |

**进一步优化手段**：

1. **缩减线程栈**：用自定义线程池替代 `spawn_blocking`，设置 `std::thread::Builder::new().stack_size(256 * 1024)`（256KB 虚拟栈），20 个插件仅占 5MB 虚拟空间
2. **终极方案**：未来切换到 Wasmtime（支持跨平台 async host function / fiber），彻底消除 blocking 线程，实现真正的零线程开销

**结论**：方案 B 的线程内存开销对"性能优先"的定位没有实质影响。

---

## 方案对比总览

| 维度 | 方案 A（`pi_eval_js` 导出） | 方案 B（`waitForEvent` 阻塞，推荐） |
|------|--------------------------|----------------------------------|
| wasmedge-quickjs 改动量 | 大（全局 Context + Wasm 导出） | 极小（仅新增 host 路由） |
| 重新编译 .wasm | 是 | 是（需加事件循环） |
| 宿主线程模型 | 按需调用，无常驻线程 | 每插件一个 spawn_blocking 线程 |
| idle 线程开销 | 无 | CPU≈0，~8MB 栈/线程 |
| unsafe 代码 | 是（全局静态 Context） | 否 |
| Tokio 重入问题 | 有，需额外处理 | 无 |
| 状态保持 | 完整 | 完整 |
| setInterval 支持 | 是 | 是（事件循环持续运行） |
| submit/poll 复用 | 是 | 是 |
| 协议变更 | 无 | 无（复用 `__pi_host_call`） |
| **推荐** | — | **Phase 2 首选** |

---

## 推荐：采用方案 B

理由：

1. **改动最小、风险最低**：不需要 `unsafe` 全局状态，不需要处理 Tokio 在 `pi_eval_js` 内重入问题
2. **天然兼容 Tokio**：`spawn_blocking` 是 Rust async 生态对"必须阻塞的操作"的标准解法
3. **idle 线程实际成本极低**：阻塞等待 channel 的线程被 OS 挂起，CPU 消耗接近零
4. **submit/poll 完全复用**：事件 handler 里的 `await pi.exec()` 等异步操作无需任何改造
5. **插件数量有限**：pi-mono/openclaw 生态下典型 5-20 个插件，每个 ~8MB 栈，总计不超过 160MB，完全可接受

若未来需要支持数百个插件或追求极致资源效率，可再评估方案 A，或考虑切换到支持跨平台 async host function 的 Wasmtime。

---

## 生命周期状态机（定版）

```text
Created -> Running -> Idle -> Running
   |         |         |        |
   |         |         |        -> Error
   |         |         -> ShuttingDown -> Stopped
   |         -> ShuttingDown -> Stopped
   -> Error
```

- `Created`：runtime key 建立，actor 未完成初始化。
- `Running`：正在处理事件或 handler 执行中。
- `Idle`：阻塞在 `waitForEvent`。
- `ShuttingDown`：收到 `Shutdown`，执行收尾。
- `Stopped`：线程退出，资源释放完成。
- `Error`：初始化或执行失败，等待恢复/重建。

---

## 可靠性策略（文档层）

- 队列上限：`DispatchEvent` 使用有界 channel，超过上限时拒绝或回压。
- 超时策略：事件处理超时、hostcall 超时分离，避免互相吞错。
- 清理策略：`session_end` 触发 `Shutdown`，并清理 `RuntimeKey` 下的 pending call。
- 恢复策略：`Error` 态支持按需重建 actor（重建前记录诊断事件）。

---

## 待实现与验收清单

### 待实现

- [ ] 按 `session_id + plugin_id` 建立 runtime key 与 manager
- [ ] VM actor 命令通道（Init/DispatchEvent/Shutdown）
- [ ] `_start` 常驻循环的 lazy start + 显式 shutdown
- [ ] 事件队列上限、超时、清理与恢复策略

### 验收标准

- [ ] 插件全局变量可跨事件保持
- [ ] 已注册 handler 在多次事件中持续有效
- [ ] `setInterval` 在会话期间稳定运行
- [ ] 多会话上下文隔离（状态不串会话）
- [ ] 关闭流程无悬挂线程、无 pending 泄漏

---

**导航**：返回 [插件系统全貌](../plugin-system-overview.md) | 上一节：[JS API 与 pi-mono 对齐](js-api-alignment.md)
