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

Phase 2 宿主侧改造要点：将 `Vm` 提升为 `WasmInstance` 的持久字段：

```rust
pub struct WasmInstance {
    vm: Option<Vm<'static, dyn SyncInst>>,  // Phase 2：Vm 持久化
    vm_handle: Option<JoinHandle<()>>,       // spawn_blocking 句柄
    event_tx: mpsc::Sender<Event>,           // 向 VM 发事件
    // ...
}
```

对方案 B：`_start` 永不返回（JS 事件循环），Vm 自然保持存活，不需要 wasmedge-quickjs 导出新函数。

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
