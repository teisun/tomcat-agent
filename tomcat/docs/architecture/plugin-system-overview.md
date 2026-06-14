# 插件系统总览（历史索引）

> 本文保留为 **WasmEdge 时代插件系统** 的历史索引，不代表当前实现。
>
> 当前生效的设计与实现，请直接阅读：
>
> - [plugin-system-overview_new.md](plugin-system-overview_new.md)
> - [src/ext/README.md](../../src/ext/README.md)

## 当前现状

- 插件运行时已从 **WasmEdge + `wasmedge_quickjs.wasm`** 迁移到 **进程内 `rquickjs`**。
- `engine_wasmedge.rs` / `instance_wasmedge.rs`、`assets/wasm/`、`assets/modules/`、`install-wasmedge.sh` 等旧资产已删除。
- 插件能力边界以 `pi.*` hostcall、`PluginRuntimeManager`、`VmActor`、`PluginToolExecutor` 为准。

## 为什么保留此页

- 便于从旧讨论、旧任务单、旧提交消息跳转过来时知道“文档已经换版”。
- 避免历史链接直接 404。

若你想了解当前实现，请不要继续把旧版 Wasm/WasmEdge 设计当作现状参考。
