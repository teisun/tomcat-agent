# Host API 层（现行摘要）

Host API 层的职责没有变：插件里所有敏感/异步能力都通过 `pi.*` 发起，再由宿主统一路由和审计。

## 当前实现

- JS 入口：`assets/js/pi_bridge.js`
- Rust 入口：`src/ext/host_binding.rs`
- 路由器：`src/ext/dispatcher/dispatch.rs` / `HostApiDispatcher`

## 当前原则

- 读写文件、执行命令、会话访问、LLM 调用、事件总线等都必须过 `pi.*`
- `crypto` 等纯计算不走 dispatcher，而走同步原生函数
- 加载期 `requiredPermissions` 当前默认放行；真正敏感的 hostcall 仍统一收口

更多细节请看 [`../plugin-system-overview_new.md`](../plugin-system-overview_new.md)。
