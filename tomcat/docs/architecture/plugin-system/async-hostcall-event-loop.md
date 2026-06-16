# 异步 hostcall 与事件循环（现行摘要）

## 当前模型

异步 hostcall 仍采用 submit/poll 思路，但承载运行时已经从 Wasm guest 改为 `rquickjs` session VM。

### 关键组件

- `pi_bridge.js`：提供 `__pi_start_event_loop`
- `pi_main_loop.js`：session VM 下处理 `command_invoke`
- `HostApiDispatcher`：维护 `async_results` / `instance_calls`
- `VmActor`：在专属线程里跑长生命周期 VM

## 当前语义

- 同步纯计算尽量在 VM 内完成
- 需要宿主能力的异步工作通过 `pi.*` 发起
- session VM 通过 `waitForEvent` 等待宿主事件
- `end_session()` 明确清理；`idle_ttl_ms` 走机会式回收

## 现行参考

- [`../plugin-system-overview_new.md`](../plugin-system-overview_new.md)
- `src/ext/vm_actor.rs`
- `src/ext/dispatcher/dispatch.rs`
- `assets/js/pi_bridge.js`
