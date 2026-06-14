# JS Bridge 层（现行摘要）

## 当前实现

当前 JS Bridge 由 `assets/js/pi_bridge.js` 提供，并在 `PluginVmInstance::build_combined_script()` 中注入到 `rquickjs` 运行时。

它负责：

- 暴露 `globalThis.pi`
- 包装 `__pi_host_call`
- 提供事件循环入口（`__pi_start_event_loop`）
- 承接工具调用桥（如 `__pi_execute_tool_async`）
- 与 session VM 主循环配合（`pi_main_loop.js`）

## 不再存在的旧路径

以下内容已不是现状：

- `wasmedge_quickjs.wasm`
- `build-custom-quickjs.sh`
- 通过 Wasm guest 导出 `_start` 来承载 bridge

## 现行参考

- 总览：[`../plugin-system-overview_new.md`](../plugin-system-overview_new.md)
- 协议：[`host-call-protocol.md`](host-call-protocol.md)
- 运行时代码：`src/ext/instance_rquickjs.rs`、`src/ext/host_binding.rs`
