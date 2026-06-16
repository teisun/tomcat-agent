# 沙箱层（现行摘要）

Tomcat 当前的“沙箱”不是 Wasm 内存硬墙，而是 **进程内 `rquickjs` + 软隔离**：

- `VmActor` 专属线程
- `catch_unwind` 兜 Rust panic
- `call_timeout_ms`
- `interrupt_budget`
- `js_heap_mb`
- `PluginRuntimeManager` + 机会式 idle 回收

真正敏感的宿主能力仍统一收口到 `pi.*` hostcall。

更多细节请看 [`../plugin-system-overview_new.md`](../plugin-system-overview_new.md) 的隔离与风险章节。
