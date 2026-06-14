# Host / Guest 边界（现行摘要）

Tomcat 当前插件系统的 “guest” 已不是 Wasm guest，而是**进程内 `rquickjs` VM**。

## 边界划分

- **guest（插件 VM）**：执行插件 JS/TS、维护局部状态、运行轻量工具与同步 crypto
- **host（Rust 宿主）**：提供文件、命令、会话、LLM、事件总线、审计等真实能力

两者之间的唯一敏感通道仍是：

`globalThis.pi.*` → `__pi_host_call` → `HostApiDispatcher`

## 当前事实

- 不再有 Wasm 线性内存交换
- 不再依赖 `wasmedge_quickjs.wasm`
- `node:*` 只保留少量 alias 或 fail-closed 拒绝桩

## 现行参考

- 总览：[`../plugin-system-overview_new.md`](../plugin-system-overview_new.md)
- 代码：`src/ext/instance_rquickjs.rs`、`src/ext/host_binding.rs`、`src/ext/dispatcher/dispatch.rs`
