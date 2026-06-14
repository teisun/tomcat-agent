# Hostcall 协议（现行摘要）

插件 VM 与 Rust 宿主之间的 JSON 协议在迁移到 `rquickjs` 后**没有本质变化**。

## 入口

- JS 侧：`globalThis.pi.*`
- Bridge：`__pi_host_call(request_json)`
- Rust 侧：`invoke_host_func_with()` → `HostApiDispatcher::dispatch()`

## 响应形态

协议仍以 `HostRequest` / `HostResponse` 为中心：

- `HostRequest`：`module` / `method` / `params` / `callId`
- `HostResponse`：`ok` / `data` / `error` / `callId`

## 当前边界

- 纯同步 crypto 不经此协议
- 真正敏感或异步的文件/命令/会话/LLM/事件能力必须经此协议
- 运行时已不再是 Wasm guest，因此无需再讨论线性内存读写

详细上下文请看 [`../plugin-system-overview_new.md`](../plugin-system-overview_new.md) 与 `src/ext/host_binding.rs`。
