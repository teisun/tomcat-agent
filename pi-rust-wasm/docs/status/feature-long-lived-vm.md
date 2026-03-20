| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| @Tom | 2026-03-16 22:30 | PENDING_INTEGRATION | feature/long-lived-vm | - |

### ✅ DONE (已完成)
- [x] **[P1]** 15.2 RuntimeManager：session_id + plugin_id 双键
- [x] **[P1]** 15.1 结构改造：长寿命运行单元（init_vm 解耦启动与执行）
- [x] **[P1]** 15.4 VM actor 命令通道 + spawn_blocking 专属线程
- [x] **[P1]** 15.5 dispatcher.rs 新增 __session.waitForEvent 路由 + 有界 channel
- [x] **[P1]** 15.6 _start 常驻循环：lazy start + setTimeout(loop, 0) + Shutdown 退出
- [x] **[P1]** 15.3 PluginManager 升级为 session 维度（start_session_vm / dispatch_session_event / end_session）
- [x] **[P1]** 15.7 废弃组合脚本 + __pi_dispatch_event（#[deprecated] 标注）
- [x] **[P1]** 15.8 队列上限/回压（deliver_event + try_send）、session_end 清理
- [x] **[P1]** 15.9 单元+集成测试（runtime_manager 4 + vm_actor 3 + dispatcher 7 + plugin 13 全通过）

### 🔌 INTERFACE (接口变更)
- 新增 `__session.waitForEvent` hostcall 路由（同步阻塞，spawn_blocking 线程内调用）
- 新增 `__pi_start_event_loop()` JS 全局函数（pi_bridge.js）
- `WasmInstance::dispatch_event` 标记 `#[deprecated]`
- 新增 `RuntimeManager`、`VmRuntimeKey`、`VmActor`、`VmActorHandle` 类型

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
