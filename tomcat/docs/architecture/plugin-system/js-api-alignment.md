# JS API 对齐范围（现行摘要）

## 当前结论

Tomcat 当前**不做**“与 pi-mono / WasmEdge QuickJS 完整 API 对齐”。

现行对齐范围只包括：

- `pi.*` 核心 hostcall 习惯
- Tier-A 轻量能力：`path` / `util.format` / `events.EventEmitter` / `Buffer` / `crypto`
- 少量工具级 shim：`@sinclair/typebox`、`ms`
- `node:*` 的 alias / fail-closed 约束

## 不再承诺

- 整套 Node 兼容层
- pi-mono UI/AI/coding-agent/sandbox 生态包的运行时注入
- WasmEdge guest 环境下的行为一致性

## 现行参考

- 总览：[`../plugin-system-overview_new.md`](../plugin-system-overview_new.md)
- 运行时代码：`src/ext/instance_rquickjs.rs`、`src/ext/crypto_native.rs`
- 用户说明：[`../../user-guide.md`](../../user-guide.md)
