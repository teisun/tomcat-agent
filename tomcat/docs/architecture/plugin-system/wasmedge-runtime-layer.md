# WasmEdge 运行时层（历史说明）

> 该文档描述的是 **已删除** 的 WasmEdge 运行时层。
>
> 当前插件运行时不再使用 WasmEdge，也不再依赖 `wasmedge_quickjs.wasm`。

## 现行替代

- 引擎与实例：`src/ext/engine_rquickjs.rs`、`src/ext/instance_rquickjs.rs`
- 配置：`src/ext/engine_config.rs`
- 长生命周期 VM：`src/ext/vm_actor.rs`、`src/ext/runtime_manager.rs`
- 总览：[`../plugin-system-overview_new.md`](../plugin-system-overview_new.md)

## 历史边界

本文仅用于说明“旧实现曾经如何工作”，不应再被用作：

- 当前架构事实源
- 当前部署/安装指南
- 当前测试入口说明

如需了解现行运行时，请直接以 `plugin-system-overview_new.md` 与 `src/ext/README.md` 为准。
