# VmActor Shutdown 死代码与通道架构分析

## 问题摘要

`VmActor` 的 `cmd_rx` 在收到 `Init` 后进入 `run_func("_start")`，此后**同一线程**被 QuickJS 事件循环阻塞，不再读取 `cmd_rx`。`end_session` 发送的 `VmCommand::Shutdown` 被缓冲但**永不被消费**。此外还有一套完全未使用的事件 channel。

**影响**：当前不会导致永久挂死（会话结束靠另一条路径），但代码语义与实际行为不一致，增加维护和调试成本。

---

## 现状：三套管道

### 管道 1：VmCommand 通道（`cmd_tx` / `cmd_rx`）

- **创建**：[vm_actor.rs L109](../../src/ext/vm_actor.rs) `tokio::sync::mpsc::channel::<VmCommand>(32)`
- **发送端**：`VmActorHandle::dispatch()`，被 `start_session_vm` 发 `Init`、`end_session` 发 `Shutdown` 调用
- **接收端**：`VmActor::run()` L140 `cmd_rx.blocking_recv()`
- **状态**：`Init` 有效（在 `_start` 前读取）；`Shutdown` 在 `_start` 阻塞期间**无法被读取**；`DispatchEvent` 在生产代码中**从未被发送**

### 管道 2：VmActor 自带的事件通道（`event_tx` / `event_rx`）— 完全未使用

- **创建**：[vm_actor.rs L111](../../src/ext/vm_actor.rs) `std::sync::mpsc::sync_channel::<EventEnvelope>(event_capacity)`
- **发送端**：`spawn()` 返回 `event_tx`，但 [plugin.rs L559](../../src/ext/plugin.rs) 用 `_event_tx`（下划线前缀）接收后丢弃
- **接收端**：存在 `VmActor.event_rx` 字段中，`run()` / `run_vm()` 中**从未读取**
- **状态**：**死代码**，是早期设计残留

### 管道 3：Dispatcher 事件通道（`event_senders` / `event_receivers`）— 唯一真正工作的

- **创建**：[dispatcher.rs `register_event_channel()`](../../src/ext/dispatcher.rs)
- **发送端**：`deliver_event()` 投递业务事件；`cleanup_instance()` 发送 `__shutdown`
- **接收端**：`do_wait_for_event()` 对应 JS 侧 `waitForEvent` 调用
- **状态**：**所有业务事件和 `__shutdown` 均走此通道**

### 通道关系示意

```
管道 1  cmd_tx ──────→ cmd_rx          VmActor.run() 里读
        (VmCommand)                    Init 有效；Shutdown 读不到

管道 2  event_tx ────→ event_rx        VmActor 里存着
        (EventEnvelope)                没人发，没人读 → 死代码

管道 3  senders ─────→ receivers       Dispatcher 里存着
        (EventEnvelope)                deliver_event 发，waitForEvent 读
                                       唯一干活的事件通道
```

---

## 根因分析

### 为什么 `Shutdown` 没人读？

`VmActor::run()` 的执行流：

```
① cmd_rx.blocking_recv()  → 读到 Init
② run_vm()
     └→ vm.run_func("_start")  ← 阻塞在 QuickJS 事件循环
        └→ JS: for(;;) { waitForEvent(50ms) ... }
③ _start 返回后继续          ← 此时 Shutdown 已堆在 cmd_rx 里
```

**从 ② 进入到 ③ 返回之间**，线程被 Wasm/JS 占用，Rust 的 `run()` 跑不到下一条 `cmd_rx.recv()`。`Shutdown` 消息在 `cmd_rx` 缓冲区里等，但**没有 Rust 代码在这段时间读取它**。

### 会话实际怎么结束的？

`end_session`（[plugin.rs](../../src/ext/plugin.rs)）的执行路径：

```
1. h.shutdown()          → VmCommand::Shutdown 发到 cmd_rx（没人读）
2. cleanup_instance()    → __shutdown 发到管道 3（事件通道）
                         → JS 的 waitForEvent 收到 __shutdown → return
                         → _start 返回 → VmActor.run() 继续
```

**真正让 JS 退出事件循环的是步骤 2**（管道 3），不是步骤 1。

### 诊断日志佐证

全量测试中每个长生命周期 VM 结束时均出现：

```
WARN [VmActor s1/xxx] drained 1 VmCommand(s) from cmd_rx after _start returned;
     these were not processed while VM blocked
```

证实 `Shutdown` 在 `_start` 运行期间**从未被处理**，只是事后被清空。

---

## 解决方案

### 第一步：清理死代码

| 改动 | 文件 |
|------|------|
| 删除管道 2（`event_tx` / `event_rx`） | `vm_actor.rs`：删字段、删 `sync_channel` 创建、删 `event_rx()` getter；`spawn()` 返回值简化为 `VmActorHandle` |
| 删除 `VmCommand::DispatchEvent` 变体 | `vm_actor.rs`：从枚举中删除 |
| 适配调用方 | `plugin.rs`：删 `_event_tx`；`long_lived_vm_tests.rs`：更新/删除 `DispatchEvent` 相关测试 |

### 第二步：让 shutdown 路径可靠

**核心改动**：`end_session` 调整执行顺序，`shutdown()` 不再依赖 `cmd_rx`。

```
当前顺序（有问题）:
  1. h.shutdown()         ← 发 VmCommand::Shutdown（没人读）
  2. cleanup_instance()   ← 发 __shutdown 到事件通道（真正起作用）

改为:
  1. cleanup_instance()   ← 发 __shutdown 到事件通道，JS 退出
  2. join_handle + 超时   ← 等 actor 线程结束（可选）
  3. 不再调 h.shutdown()  ← 或改为仅设置 state = ShuttingDown
```

具体改动：

- **`VmActorHandle`**：`shutdown()` 改为设置 `state = ShuttingDown`（语义标记），不再发 `VmCommand::Shutdown`
- **`VmActor::spawn()`**：存 `spawn_blocking` 返回的 `JoinHandle`
- **`end_session`**：先 `cleanup_instance`，再可选 `join_handle.await` 带超时确认线程退出
- **`VmCommand`**：简化为只剩 `Init`（`Shutdown` 可标 deprecated 或直接删除）

### 第三步（可选）：超时强杀保护

- `join_handle.await` 超时（如 5s）后打 `warn!`："VM 未在超时内退出，可能存在死循环"
- 当前 Rust 无法安全杀线程；如果将来 WasmEdge 支持 interrupt/cancel，可在此处调用
- 该保护机制防止单个卡死的 VM 阻塞整个 `end_session` 流程

### 改后的数据流

```
cmd_tx ──→ cmd_rx        仅用于 Init（一次性信号）
senders ──→ receivers    所有事件 + __shutdown 走这条（唯一管道）
join_handle              确认 actor 线程退出（可选超时）
```

---

## 风险与注意事项

- **`cleanup_instance` 的 `try_send` 可能因通道满而失败**：后续 `event_senders.remove()` 会断开通道，`rx.recv()` 返回 `Disconnected`，JS 侧 `waitForEvent` 将映射为 `__shutdown`，属于兜底路径。
- **WasmEdge 收尾阶段的 `Code: 0x8d` 错误**：通道关闭后 JS 再调宿主函数会报此错，不影响测试结果（以 harness 的 `ok`/`FAILED` 为准）。参见 [INTEGRATION_MERGE_AND_ACCEPTANCE.md](../../agents/INTEGRATION_MERGE_AND_ACCEPTANCE.md) "WasmEdge stderr 说明"。
- **先 `cleanup_instance` 再等 join**：需确认 `cleanup_instance` 对尚未进入 `waitForEvent` 的 VM 也能正确退出（当前 `do_wait_for_event` 找不到已移除的 channel 会返回错误，JS 侧 `catch(hostErr)` 同样走退出路径）。

## 相关文件

| 文件 | 说明 |
|------|------|
| [src/ext/vm_actor.rs](../../src/ext/vm_actor.rs) | VmActor 定义、spawn、run/run_vm |
| [src/ext/plugin.rs](../../src/ext/plugin.rs) | PluginManager: start_session_vm / end_session |
| [src/ext/dispatcher.rs](../../src/ext/dispatcher.rs) | HostApiDispatcher: waitForEvent / cleanup_instance / deliver_event |
| [assets/js/pi_bridge.js](../../assets/js/pi_bridge.js) | JS 侧 `__pi_start_event_loop` 事件循环 |
| [assets/js/pi_main_loop.js](../../assets/js/pi_main_loop.js) | 外层双循环（处理 command_invoke） |
| [integration_test_hang_remediation.md](./integration_test_hang_remediation.md) | 相关历史修复记录 |
