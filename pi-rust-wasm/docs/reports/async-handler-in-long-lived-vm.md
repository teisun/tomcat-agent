# 长生命周期 VM 中 async handler 执行问题与解决方案

> 本报告服务于 TASK-05d E2E 测试整改，阐述 pi-rust-wasm 在长生命周期 VM 中执行 async 命令 handler 时遇到的核心技术障碍及其解决方案。
> 所有引擎层面的结论均基于 `wasmedge-quickjs` 仓库源码审计。

---

## 1. 背景：wasmedge-quickjs 的真实执行模型

### 1.1 两层架构

pi-rust-wasm 通过 `wasmedge_sdk::Vm::run_func("quickjs", "_start")` 运行 `wasmedge_quickjs.wasm`。这个 wasm 二进制的源码在 `wasmedge-quickjs/` 仓库，它自身由两层构成：

```
┌─────────────────────────────────────────────────┐
│  上层: Rust 运行时 (wasmedge-quickjs/src/)       │
│    main.rs    → tokio 单线程运行时               │
│    Runtime    → 管理 QuickJS 上下文 + EventLoop  │
│    EventLoop  → next_tick / immediate / sub_tasks│
│    core.rs    → setTimeout / setInterval 实现     │
├─────────────────────────────────────────────────┤
│  下层: C 引擎 (lib/libquickjs.a)                │
│    JS_ExecutePendingJob  → 处理 Promise 微任务   │
│    js_std_loop           → 声明但从未被调用       │
│    js_std_eval_binary    → 执行 JS 字节码        │
└─────────────────────────────────────────────────┘
```

**关键发现**：`js_std_loop`（C 函数）虽然在 `lib/binding.rs:1375` 中声明了 `extern "C"` 绑定，但在整个 `wasmedge-quickjs/src/` 中**从未被调用**。Rust 层用自己的 `run_loop_without_io()` + tokio 运行时完全替代了它。

### 1.2 EventLoop 结构体

`wasmedge-quickjs/src/event_loop/mod.rs` 定义了 `EventLoop`：

```rust
pub struct EventLoop {
    next_tick_queue: LinkedList<Box<dyn FnOnce()>>,   // nextTick 回调
    immediate_queue: LinkedList<Box<dyn FnOnce()>>,   // setTimeout(fn, 0) 回调
    pub(crate) waker: Option<std::task::Waker>,       // 唤醒 tokio Runtime
    pub(crate) sub_tasks: LinkedList<tokio::task::JoinHandle<()>>, // 异步任务句柄
}
```

它是 Rust 层自己实现的任务队列，**不是** QuickJS C 引擎的一部分。

### 1.3 run_loop_without_io — 真正的"微任务 drain"函数

`wasmedge-quickjs/src/quickjs_sys/mod.rs` 中的 `run_loop_without_io()` 是**替代 `js_std_loop` 的核心函数**：

```rust
unsafe fn run_loop_without_io(&mut self) -> i32 {
    let event_loop = /* 从 Runtime opaque 取出 EventLoop */;
    loop {
        // 步骤 A: 循环调用 JS_ExecutePendingJob 直到队列空
        //         → 处理所有 Promise 微任务（await 恢复点、.then 回调）
        'pending: loop {
            let err = JS_ExecutePendingJob(rt, &mut pctx);
            if err <= 0 { break 'pending; }
        }
        // 步骤 B: 执行 EventLoop 中的 immediate/nextTick 回调
        if event_loop.run_tick_task() == 0 {
            break;  // 没有更多任务 → 退出
        }
        // 有新任务 → 回到步骤 A，处理可能产生的新 Promise
    }
}
```

它的工作方式是一个 drain 循环：

```
run_loop_without_io
    │
    ├─► JS_ExecutePendingJob (重复直到队列空)
    │     处理 Promise .then / await 恢复点
    │
    ├─► event_loop.run_tick_task()
    │     执行 immediate_queue 和 next_tick_queue 中的回调
    │     （setTimeout(fn, 0) 的回调就在这里执行）
    │
    ├─► 如果 run_tick_task 执行了回调 → 回到顶部
    │     （回调可能产生新的 Promise，需要再次 drain）
    │
    └─► 如果 run_tick_task 返回 0 → 退出
```

### 1.4 EventLoop 与 run_loop_without_io 的关系

```
EventLoop (数据结构)              run_loop_without_io (执行引擎)
┌──────────────────────┐         ┌─────────────────────────────────┐
│ immediate_queue      │────────►│ event_loop.run_tick_task()      │
│   setTimeout(fn, 0)  │         │   取出所有 immediate 回调并执行  │
│                      │         │                                 │
│ next_tick_queue      │────────►│   取出所有 nextTick 回调并执行   │
│   nextTick(fn)       │         │                                 │
├──────────────────────┤         │ JS_ExecutePendingJob(rt, ...)   │
│ sub_tasks            │         │   处理 Promise 微任务            │
│   JoinHandle 列表    │─ ─ ─ ─►│   （sub_tasks 由 tokio 轮询,     │
│   (tokio 异步任务)   │         │    不在这里处理）                │
├──────────────────────┤         └─────────────────────────────────┘
│ waker                │
│   唤醒 tokio Runtime │─ ─ ─ ─► tokio 事件循环被唤醒，
│                      │          重新 poll Runtime Future
└──────────────────────┘
```

`EventLoop` 是存任务的容器，`run_loop_without_io` 是取出并执行任务的引擎。两者是生产者-消费者关系，配合完成"阶段 2"的工作。

一句话总结各任务类型的写入和执行路径：

```
setTimeout(fn, 0) ──写入──► EventLoop.immediate_queue ──读取执行──► run_loop_without_io
nextTick(fn)      ──写入──► EventLoop.next_tick_queue  ──读取执行──► run_loop_without_io
setTimeout(fn,ms) ──写入──► EventLoop.sub_tasks        ──读取执行──► tokio 运行时
await / .then     ──写入──► QuickJS 内部微任务队列      ──读取执行──► run_loop_without_io
                                                                     (JS_ExecutePendingJob)
```

### 1.5 完整示例：一段 JS 代码走过的完整流程

以下用一段涵盖所有任务类型的 JS 代码，完整演示阶段 1 和阶段 2 的协作过程。

```javascript
async function work() {
    console.log("1");
    var x = await Promise.resolve(42);   // 产生 Promise 微任务
    console.log("2", x);
}
work();
setTimeout(function() { console.log("3"); }, 0);    // delay=0
setTimeout(function() { console.log("4"); }, 100);   // delay=100
console.log("5");
```

#### 阶段 1：eval_buf 执行脚本

JS 引擎从上到下同步执行每一行：

```
执行 work()
  → 打印 "1"
  → 遇到 await Promise.resolve(42)
  → work 暂停，Promise 微任务入队（QuickJS 内部队列）
  → 继续执行 work() 之后的代码

执行 setTimeout(fn, 0)
  → wasmedge-quickjs 的 set_timeout 处理：
    delay === 0，走 immediate 路径：
    event_loop.add_immediate_task(回调"打印3")
    → EventLoop.immediate_queue: [回调"打印3"]

执行 setTimeout(fn, 100)
  → wasmedge-quickjs 的 set_timeout 处理：
    delay > 0，走 tokio 路径：
    ctx.future_to_promise(async { tokio::time::timeout(100ms); 回调"打印4"() })
    → EventLoop.sub_tasks: [tokio任务"100ms后打印4"]

执行 console.log("5")
  → 打印 "5"

脚本同步部分结束，eval_buf 返回。
```

此时的状态：

```
已打印: "1", "5"

QuickJS 内部微任务队列:
  [await Promise.resolve(42) 的恢复点 → 将打印 "2"]

EventLoop:
  immediate_queue: [回调"打印3"]
  next_tick_queue: []
  sub_tasks: [tokio任务"100ms后打印4"]
```

#### 阶段 2 第一步：run_loop_without_io 登场

```
run_loop_without_io 开始工作
    │
    │  第一轮:
    │
    ├─► JS_ExecutePendingJob (循环调用直到队列空)
    │     → 取出微任务：await Promise.resolve(42) 的恢复点
    │     → 执行：x = 42, console.log("2", 42)
    │     → 打印 "2 42"
    │     → 再次调用 JS_ExecutePendingJob → 返回 0（队列空了）
    │
    ├─► event_loop.run_tick_task()
    │     → 取出 immediate_queue 里的回调"打印3"
    │     → 执行：console.log("3")
    │     → 打印 "3"
    │     → 返回 1（执行了 1 个任务）
    │
    │  返回值 > 0，继续第二轮:
    │
    ├─► JS_ExecutePendingJob → 返回 0（队列空）
    │
    ├─► event_loop.run_tick_task() → 返回 0（队列空）
    │
    └─► 返回 0 → run_loop_without_io 退出
```

此时：

```
已打印: "1", "5", "2 42", "3"

EventLoop:
  immediate_queue: []  (已清空)
  sub_tasks: [tokio任务"100ms后打印4"]  (还没到时间)
```

#### 阶段 2 第二步：tokio 接管处理 sub_tasks

```
Runtime::poll 检查 sub_tasks
    │
    │  sub_tasks 里还有一个 tokio 任务 → 返回 Pending
    │
    │  ......100ms 过去......
    │
    │  tokio 定时器到期
    │  tokio 任务执行：回调"打印4"()
    │  → 打印 "4"
    │  → waker.wake() 唤醒 Runtime
    │
    ▼
Runtime 被唤醒，再次 poll
    │
    ├─► run_loop_without_io()
    │     JS_ExecutePendingJob → 0
    │     run_tick_task → 0
    │     退出
    │
    └─► 检查 sub_tasks → 全部完成 → Ready
```

最终输出顺序：`1, 5, 2 42, 3, 4`

### 1.6 setTimeout 的两条路径

`wasmedge-quickjs/src/internal_module/core.rs` 中自定义了 `setTimeout`（覆盖了 C 引擎内置版本）：

```
setTimeout(callback, delay)
    │
    ├─ delay === 0
    │    → event_loop.add_immediate_task(callback)
    │    → 放入 immediate_queue
    │    → 由 run_loop_without_io → run_tick_task 执行
    │
    └─ delay > 0
         → ctx.future_to_promise(async {
               tokio::time::timeout(delay, ...).await
               callback.call()
           })
         → 创建 tokio 异步任务 → 放入 sub_tasks
         → 由 tokio 运行时轮询，到期后执行
```

### 1.6 完整执行时序

`wasmedge-quickjs/src/main.rs` 的入口是 `#[tokio::main(flavor = "current_thread")]`：

```
main() 入口 (tokio 单线程运行时)
    │
    ▼
rt.async_run_with_context(|ctx| {
    ctx.eval_buf(code)              ← 执行 JS 脚本
}).await
    │
    │  await 展开为 RuntimeResult::poll:
    │
    ▼
第一次 poll:
    ├─ 执行闭包: ctx.eval_buf(code)      ← "阶段 1"
    │    JS 脚本从第一行到最后一行同步执行
    │    遇到 await → 暂停 async 函数，继续下文
    │    遇到 setTimeout(fn, 0) → 放入 immediate_queue
    │    遇到 setTimeout(fn, delay) → 创建 tokio 任务
    │    所有同步代码执行完 → eval_buf 返回
    │
    └─ poll Runtime Future:             ← "阶段 2"
         ├─ run_loop_without_io()
         │    JS_ExecutePendingJob × N    (drain Promise 微任务)
         │    run_tick_task               (drain immediate/nextTick)
         │
         └─ 检查 sub_tasks
              有未完成 tokio 任务 → Pending (等 tokio 唤醒)
              全部完成 → Ready (退出)

后续 poll (tokio 定时器到期触发):
    ├─ run_loop_without_io() 再次 drain
    └─ 检查 sub_tasks → Ready?
    ...循环直到所有 sub_tasks 完成
```

### 1.7 示例

```javascript
async function doWork() {
    console.log("1");
    var result = await pi.exec("git status");  // 暂停，后续入 Promise 微任务队列
    console.log("2", result);                  // 由 run_loop_without_io 的
}                                              //   JS_ExecutePendingJob 驱动执行
doWork();
console.log("3");
// 阶段 1 (eval_buf) 输出: 1, 3
// 阶段 2 (run_loop_without_io + tokio) 输出: 2
```

---

## 2. 长生命周期 VM 中的阻塞问题

### 短生命周期 VM（run_script_file 路径）

脚本正常执行完毕 → eval_buf 返回 → 阶段 2 启动 → async 任务正常处理。

```
eval_buf: [pi_bridge.js] [shims] [用户脚本] → 返回
                                                │
阶段 2:                                   [run_loop_without_io]
                                          [tokio poll sub_tasks]
                                          → 处理 await / setTimeout → 退出
```

没有问题。

### 长生命周期 VM（VmActor + init_vm 路径）

`instance_wasmedge.rs` 的 `init_vm` 在组合脚本尾部注入一行：

```javascript
__pi_start_event_loop();
```

这是一个同步的 `for(;;)` 死循环（`pi_bridge.js` L540-586），通过 `waitForEvent(50ms)` 阻塞等待宿主事件。

### __pi_start_event_loop 的作用

它是我们在 `pi_bridge.js` 中编写的 JS 函数，用于长生命周期 VM。职责是：**死循环轮询 Rust 宿主，拿到事件后交给 `__pi_dispatch_event` 分发给 JS 侧注册的 handler。**

```javascript
globalThis.__pi_start_event_loop = function () {
    for (;;) {
        var raw = __pi_host_call(JSON.stringify({
            module: '__session', method: 'waitForEvent',
            params: { timeoutMs: 50 }
        }));
        // ...
        if (res.data.type === '__tick') continue;
        if (res.data.type === '__shutdown') return;
        __pi_dispatch_event(JSON.stringify(res.data));
    }
};
```

它存在的意义是让 VM "活着"——不停地问 Rust "有事件吗？"，有就处理，没有就等 50ms 再问。

### 为什么阻塞是致命的

```
eval_buf: [pi_bridge.js] [shims] [插件代码] [__pi_start_event_loop()]
                                                   │
                                            for(;;) 死循环
                                            永远不退出
                                                   │
                                            eval_buf 永远不返回
                                                   │
                                                   ╳
阶段 2:                                     永远不会启动！
  run_loop_without_io → JS_ExecutePendingJob
  tokio poll sub_tasks
```

**后果**：
- `JS_ExecutePendingJob` 不被调用 → Promise 微任务（`await` 恢复点）永远不会被处理
- tokio 运行时被阻塞 → `setTimeout(fn, delay>0)` 的定时器任务永远不会触发
- `run_tick_task` 不被调用 → `setTimeout(fn, 0)` 的 immediate 回调也不会执行

插件通过 `pi.registerCommand` 注册的 handler 通常是 `async` 函数（如 `diff.ts`、`files.ts`），handler 内部使用 `await pi.exec(...)` 等异步操作。这些 `await` 恢复点存入微任务队列后，因阶段 2 未启动而永远得不到执行。

### 为什么不能用 "阶段 2" 来接收宿主事件？

直觉想法：`run_loop_without_io` + tokio 本身就是一个循环，能不能让它同时处理 Promise/setTimeout **和** Rust 宿主事件？

**做不到。** `run_loop_without_io` 和 tokio 运行时都在 Wasm 模块**内部**，pi-rust-wasm 通过 `wasmedge_sdk` 跨 Wasm 边界调用它们，无法触及其内部结构。而且阶段 2 的循环只认 QuickJS 微任务、EventLoop 回调和 tokio 异步任务——它不知道怎么从 pi-rust-wasm 的 mpsc channel 里取事件，也没有接口让我们注册自定义事件源。

```
阶段 2 能做的:                      __pi_start_event_loop 能做的:
  ✅ JS_ExecutePendingJob (Promise)    ✅ 从 Rust channel 取事件
  ✅ run_tick_task (immediate/tick)    ✅ 分发给 JS handler
  ✅ tokio 定时器 (setTimeout >0)     ❌ 处理 Promise 微任务
  ❌ Rust channel 事件 → 无能力       ❌ 处理 setTimeout 回调
```

**这就是为什么需要 `__pi_start_event_loop`**：阶段 2 不能替代它。后面会看到，两层循环方案正是让这两者交替运行，合力完成"接收宿主事件 + 执行 async 命令"。

---

## 3. command_invoke 事件完整链路

`command_invoke` 是 **Rust 宿主发起** 的事件，用于告知 JS 侧"用户请求执行某个命令"。

### 当前状态

`events.rs` 中尚无 `WIRE_COMMAND_INVOKE` 常量，`pi_bridge.js` 中 `__pi_start_event_loop` 也未识别 `command_invoke` 事件类型。当前通过 `__pi_invoke_command`（同步调用）测试命令执行，但该函数遇到 async handler 会直接返回错误。

### 设计链路（实施后）

```
用户终端: 输入 "/diff"
    │
    ▼
Rust: PluginManager
    │  解析命令名 "diff"，找到注册该命令的插件
    │
    ▼
Rust: PluginManager.dispatch_session_event(
    │      session_id,
    │      plugin_id,
    │      "command_invoke",                       ← 事件类型
    │      { "name": "diff", "args": "" },          ← 事件数据
    │      { "hasUI": true, "cwd": "/project" }     ← 上下文
    │  )
    │
    ▼
Rust: VmActorHandle.dispatch(VmCommand::DispatchEvent { ... })
    │
    │  通过 tokio::sync::mpsc channel 发送
    ▼
Rust: VmActor (spawn_blocking 线程)
    │
    │  HostApiDispatcher 的 waitForEvent 实现从
    │  std::sync::mpsc::Receiver<EventEnvelope> 取出事件
    ▼
JS: __pi_start_event_loop 内部
    │  waitForEvent(50ms) 返回:
    │  { type: "command_invoke",
    │    data: { name: "diff", args: "" },
    │    context: { hasUI: true, cwd: "/project" } }
    │
    ▼
JS: 识别 type === "command_invoke"
    │  保存 { name, args, context } 到 __pi_pending_command_invoke
    │  return（退出事件循环）
    │
    ▼
JS: async main loop（外层循环）
    │  检测到 __pi_pending_command_invoke
    │  从 __pi_commands["diff"] 取出 handler
    │  构建 ctx = __pi_build_ctx(context)
    │
    ▼
JS: await handler(args, ctx)
    │  handler 内部:
    │    var result = await pi.exec("git diff --name-only");
    │    // pi.exec → hostCallAsync → Promise + setTimeout 轮询
    │    // run_loop_without_io + tokio 驱动 setTimeout 回调和 Promise 解析
    │    ctx.ui.custom(factory);
    │    ...
    │
    ▼
JS: handler 执行完毕
    │  hostCall("context", "commandCompleted", { name: "diff" })
    │
    ▼
JS: continue → 回到 async main loop 顶部
    │  重新进入 __pi_start_event_loop()
    │  阻塞等待下一个事件
    ▼
    ...
```

### E2E 测试中的模拟

在测试代码中，"用户输入 /diff"由 Rust 测试函数直接构造：

```rust
mgr.dispatch_session_event(
    "s1", plugin_id,
    "command_invoke",
    json!({ "name": "diff", "args": "" }),
    json!({ "hasUI": true, "cwd": "/tmp" }),
)?;
```

---

## 4. 为什么不能在事件循环内直接处理 async handler

承上节，我们有两个各司其职的机制：
- **阶段 2**（`run_loop_without_io` + tokio）：能处理 Promise 和 setTimeout，但不能从 Rust channel 取事件
- **`__pi_start_event_loop`**（我们写的 JS）：能从 Rust channel 取事件，但不能处理 Promise

那能不能把 async handler 的执行"塞进" `__pi_start_event_loop` 里？以下是三种尝试及其失败原因。

### 方案 A：在同步 for 循环中直接 await — 语法不允许

```javascript
for (;;) {
    var event = waitForEvent(50);
    if (event.type === "command_invoke") {
        await handler(args, ctx);  // ← 语法错误，for 不在 async 函数内
    }
}
```

### 方案 B：直接调用 async handler 但不 await — Promise 悬空

```javascript
for (;;) {
    var event = waitForEvent(50);
    if (event.type === "command_invoke") {
        handler(args, ctx);  // 返回 Promise，但无人处理
        // 事件循环立刻回到 for 顶部
        // Promise 排入微任务队列但 JS_ExecutePendingJob 不会被调用
    }
}
```

### 方案 C：把整个事件循环放入 async 函数 — waitForEvent 阻塞 tokio

```javascript
async function eventLoop() {
    for (;;) {
        var event = waitForEvent(50);   // 同步阻塞调用
        if (event.type === "command_invoke") {
            await handler(args, ctx);   // 语法 OK
        }
    }
}
eventLoop();
```

第一次 `await handler(...)` 可以工作：

1. `await` 暂停 `eventLoop`
2. `eventLoop()` 返回的 Promise 处于 pending 状态
3. eval_buf 返回（脚本同步部分结束）
4. 阶段 2 启动：`run_loop_without_io` 处理 handler 的 Promise 链，tokio 处理 `setTimeout` 定时器
5. handler 完成，`eventLoop` 恢复

但恢复后执行到 `waitForEvent(50)` —— 这是同步阻塞调用（Rust 侧 `mpsc::recv_timeout`）。此时代码运行在 tokio 的 `poll` 上下文中。同步阻塞会冻结 tokio 单线程运行时本身：

```
tokio 单线程运行时:
    │  poll RuntimeResult
    │    → run_loop_without_io → JS_ExecutePendingJob
    │      → 恢复 eventLoop 函数
    │        → waitForEvent(50)  ← 同步阻塞，冻结 tokio 线程！
    │
    │  tokio 线程被卡住：
    │    → 无法 poll 其他 Future
    │    → 无法处理定时器任务 (setTimeout delay>0)
    │    → 无法再次调用 run_loop_without_io
    │
    │  如果 waitForEvent 等到了新事件并 await handler...
    │    → handler 内部的 await pi.exec(...)
    │    → hostCallAsync 注册 setTimeout 轮询
    │    → 但 tokio 被阻塞，定时器永远不触发
    │    → 死锁
```

### 小结：矛盾的本质

```
处理宿主事件 → 需要 waitForEvent → 同步阻塞 → 冻结当前线程
处理 async   → 需要阶段 2 运行   → 需要当前线程空闲

同一时刻只能做其中一件事。
```

这就是为什么需要两层循环交替运行的根本原因——见下一节。

---

## 5. 解决方案：async main loop + 事件循环暂停/恢复

### 核心思路

第 4 节证明了：同步阻塞等事件和 async 执行不能在同一时刻进行。解决办法是**分时复用**——把它们拆成两层循环，交替运行：

```
我们想要但做不到的（单循环同时处理所有事）:
┌──────────────────────────────────┐
│ 一个循环同时处理:                 │
│  - Rust channel 宿主事件         │
│  - Promise/setTimeout            │
│  - await handler(...)            │
└──────────────────────────────────┘

我们实际做的（两层循环交替运行）:
┌──────────────────────────────────┐
│ 内层: __pi_start_event_loop      │
│  专职: 同步阻塞等 Rust 事件      │
│  遇到 command_invoke → 退出      │
├──────────────────────────────────┤
│ 外层: async __pi_main_loop       │
│  专职: await handler(args, ctx)  │
│  由阶段 2 驱动 Promise 链        │
│  完成后 → 重新进入内层            │
└──────────────────────────────────┘
```

- **内层**：`__pi_start_event_loop()`（同步 `for(;;)`），负责高效阻塞等待宿主事件。收到 `command_invoke` 时 `return` 退出。
- **外层**：`async __pi_main_loop`（async IIFE），在内层退出后通过 `await handler(args, ctx)` 驱动 async 命令执行。完成后重新进入内层。

### 代码结构

`instance_wasmedge.rs` 在组合脚本尾部注入以下代码（替代当前的 `__pi_start_event_loop();`）：

```javascript
(async function __pi_main_loop() {
    for (;;) {
        __pi_start_event_loop();
        // 事件循环退出意味着: command_invoke 到达或 __shutdown

        var pending = globalThis.__pi_pending_command_invoke;
        if (!pending) break;  // __shutdown → 退出
        globalThis.__pi_pending_command_invoke = null;

        var entry = globalThis.__pi_commands[pending.name];
        if (entry && typeof entry.handler === 'function') {
            var ctx = __pi_build_ctx(pending.context || {});
            try {
                await entry.handler(pending.args || '', ctx);
                hostCall('context', 'commandCompleted', { name: pending.name });
            } catch (err) {
                hostCall('context', 'commandFailed', {
                    name: pending.name,
                    error: String(err)
                });
            }
        }
        // continue → 回到 for 顶部 → 重新进入事件循环
    }
})();
```

### 执行时序（基于 wasmedge-quickjs 真实机制）

```
vm.run_func("quickjs", "_start")
    │
    ▼
main() → tokio 单线程运行时
    │
    ▼
rt.async_run_with_context(|ctx| ctx.eval_buf(combined_js)).await
    │
    │  RuntimeResult::poll 第一次调用:
    │  执行 eval_buf(combined_js)
    │
    │  combined_js 内容:
    │    pi_bridge.js → shims → 插件代码 → async IIFE
    │
    ▼
eval_buf 执行 async IIFE:
    (async function __pi_main_loop() { ... })()
    │
    │  IIFE 立刻开始执行
    │  进入 for 循环
    │  调用 __pi_start_event_loop() ← 同步阻塞
    │
    │  eval_buf 被阻塞在这里
    │  （__pi_start_event_loop 是 for(;;) 循环，
    │    内部 waitForEvent 阻塞 Rust 线程）
    │
    │  ......等待......
    │
    │  宿主通过 channel 投递 command_invoke 事件
    │
    │  __pi_start_event_loop 识别到 command_invoke
    │  保存命令信息到 __pi_pending_command_invoke
    │  return 退出
    │
    │  __pi_main_loop 继续执行
    │  遇到 await entry.handler(args, ctx)
    │  __pi_main_loop 暂停（返回 pending Promise）
    │
    │  eval_buf 返回（async IIFE 已返回 Promise）
    │
    ▼
RuntimeResult::poll 继续:
    poll Runtime Future
    │
    ▼
run_loop_without_io()                  ← 阶段 2 启动
    │
    │  JS_ExecutePendingJob × N
    │    → 恢复 handler 执行
    │    → handler: var r = await pi.exec("git diff")
    │      → hostCallAsync 提交请求
    │      → setTimeout(poll, 1) 注册轮询
    │        → delay>0: 创建 tokio 定时器任务 → sub_tasks
    │
    │  run_tick_task → 无 immediate 任务
    │  run_loop_without_io 返回
    │
    ▼
检查 sub_tasks: 有未完成的 tokio 任务 → Pending
    │
    │  tokio 定时器到期 → waker.wake()
    ▼
再次 poll Runtime:
    │
    │  run_loop_without_io()
    │    JS_ExecutePendingJob
    │      → 执行 setTimeout 回调（轮询 hostCall）
    │      → 结果未就绪 → 再次 setTimeout
    │    run_tick_task
    │
    │  ......重复直到 exec 结果就绪......
    │
    │  hostCallAsync Promise resolve
    │  handler 恢复执行
    │  handler 完成 → hostCall("commandCompleted")
    │  __pi_main_loop 恢复
    │  continue → 回到 for 顶部
    │  调用 __pi_start_event_loop() ← 再次同步阻塞
    │
    │  eval_buf 再次被阻塞（等效）
    │  tokio 线程被冻结
    │  （安全的：此时无待处理任务）
    │
    │  ......等待下一个 command_invoke......
    ▼
```

### 控制权交替图

```
时间 →

eval_buf 阻塞:   [事件循环等待中......]     [事件循环等待中......]
(waitForEvent)         │                          │
                       │ command_invoke            │ command_invoke
                       ▼                          ▼
阶段 2 运行:     [run_loop_without_io]     [run_loop_without_io]
(tokio poll       [await handler(..)]       [await handler(..)]
 驱动)            [tokio 定时器轮询]         [tokio 定时器轮询]
                  [commandCompleted]         [commandCompleted]
                       │                          │
                       ▼                          ▼
eval_buf 阻塞:   [事件循环等待中......]     [事件循环等待中......]
```

### 关键性质

1. **栈深度不增长**：每次 `await` 都 yield 回 `run_loop_without_io`，没有递归。`__pi_start_event_loop` 被调用 → 退出 → 再被调用，每次都是同一层栈帧。
2. **同步阻塞不会死锁**：只有在没有待处理微任务时才进入 `__pi_start_event_loop`，此时 tokio 线程被冻结是安全的——没有 Future 需要 poll。
3. **hostCallAsync 正常工作**：handler 中的 `await pi.exec(...)` 通过 `setTimeout` 轮询 → tokio 定时器任务 → `run_loop_without_io` 处理回调，完整链路通畅。
4. **普通事件（非 command_invoke）** 仍在内层事件循环中同步处理，不受影响。

---

## 6. 相关代码引用

### pi-rust-wasm 侧

| 文件 | 关键位置 | 说明 |
|------|----------|------|
| `assets/js/pi_bridge.js` | L29-70 `hostCallAsync` | async 宿主调用的 submit/poll 模式，依赖 setTimeout 轮询 |
| `assets/js/pi_bridge.js` | L484-511 `__pi_invoke_command` | 当前同步命令调用入口，遇 async handler 直接返回错误 |
| `assets/js/pi_bridge.js` | L540-586 `__pi_start_event_loop` | 同步 for(;;) 事件循环，使用 50ms 超时 waitForEvent |
| `src/ext/vm_actor.rs` | L136-200 `run` / `run_vm` | VmActor 在 spawn_blocking 线程中调用 `init_vm` + `_start` |
| `src/ext/instance_wasmedge.rs` | L279-290 `init_vm` | 组合脚本构建，尾部注入 `__pi_start_event_loop()` |
| `src/infra/events.rs` | `wire::vm` 模块 | 待新增 `WIRE_COMMAND_INVOKE` 常量 |
| `src/core/primitives.rs` | L28-34 `BashResult` | `exit_code` 字段待加 `#[serde(rename = "code")]` 以对齐 pi-mono `ExecResult` |
| `src/ext/dispatcher.rs` | `dispatch_async` | 待新增 `commandCompleted` / `commandFailed` 路由 |

### wasmedge-quickjs 侧（引擎内部机制参考）

| 文件 | 关键位置 | 说明 |
|------|----------|------|
| `src/main.rs` | L24-51 | `#[tokio::main]` 入口，调用 `async_run_with_context` |
| `src/quickjs_sys/mod.rs` | L193-221 `run_loop_without_io` | 替代 js_std_loop 的 drain 函数：`JS_ExecutePendingJob` + `run_tick_task` |
| `src/quickjs_sys/js_promise.rs` | L45-88 `impl Future for Runtime` | 阶段 2 轮询逻辑：`run_loop_without_io` + `sub_tasks` 检查 |
| `src/quickjs_sys/js_promise.rs` | L90-113 `impl Future for RuntimeResult` | eval_buf 后的 Future 驱动入口 |
| `src/event_loop/mod.rs` | L219-250 `EventLoop` | immediate_queue / next_tick_queue / sub_tasks / waker |
| `src/internal_module/core.rs` | L35-83 `set_timeout` | 自定义 setTimeout：delay=0 → immediate_queue，delay>0 → tokio timer |
| `src/host_call.rs` | L18-79 `PiHostCallFn` | `__pi_host_call` 的 Wasm-内部桥接实现 |
| `lib/binding.rs` | L1375 | `js_std_loop` extern 声明——存在但从未被调用 |
