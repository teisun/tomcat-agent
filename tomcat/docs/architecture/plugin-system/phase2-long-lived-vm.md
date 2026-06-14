# 长生命周期 VM（现行摘要）

> 旧版文档曾把长生命周期 VM 设计绑在 WasmEdge / `wasmedge_quickjs.wasm` 之上；那部分已经失效。

## 当前实现

长生命周期插件 VM 现在由以下组件共同完成：

- `PluginManager::start_session_vm()`：按 `(session_id, plugin_id)` 起/复用实例
- `PluginRuntimeManager`：管理 `PluginRuntimeKey -> VmActorHandle`
- `VmActor`：在专属线程中跑 session VM
- `PluginVmInstance::run_session_script()`：真正执行带事件循环的 rquickjs 代码

## 回收语义

- `end_session()`：明确清理该 session 下全部插件 VM
- `idle_ttl_ms`：**机会式**回收，不启动后台 sweeper
- 触发点：后续插件活动进入 `start_session_vm()` 时顺手回收已过 TTL 的实例

## 现行参考

- 总览：[`../plugin-system-overview_new.md`](../plugin-system-overview_new.md)
- 代码：`src/ext/plugin/manager.rs`、`src/ext/runtime_manager.rs`、`src/ext/vm_actor.rs`
- 验证：`tests/quickjs_e2e_tests.rs`、`src/ext/plugin/tests/suite_test.rs`
