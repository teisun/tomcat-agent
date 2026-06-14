# pi-mono 兼容策略（历史说明）

## 当前结论

Tomcat **不再追求 pi-mono 硬兼容**。

当前运行时只保留：

- `pi.*` hostcall 语义
- Tier-A 轻量能力（`path` / `util.format` / `events.EventEmitter` / `Buffer` / `crypto`）
- 少量工具 shim（`@sinclair/typebox`、`ms`）
- `node:*` 的 alias / fail-closed 行为

以下旧兼容层不再是运行时默认注入面：

- `@mariozechner/pi-tui`
- `@mariozechner/pi-ai`
- `@mariozechner/pi-coding-agent`
- `@anthropic-ai/sandbox-runtime`

## 为什么

- 旧兼容层主要服务 WasmEdge 时代的迁移与试验插件。
- 这些 shim 在仓库内没有现行 rquickjs 运行时测试作为必要性证据。
- 继续保留会让文档和实现误导读者，以为 Tomcat 仍承诺一整套 pi-mono 生态兼容。

## 现行参考

- 总览：[`../plugin-system-overview_new.md`](../plugin-system-overview_new.md)
- 运行时代码：`src/ext/instance_rquickjs.rs`、`src/ext/ts_compiler.rs`
