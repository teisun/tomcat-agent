# JS 桥接层架构（pi_bridge.js）

本文为 [Architecture](../../Architecture.md) 中「3. 宿主API层」与「4. WasmEdge运行时层」之间的桥接子文档，描述 pi-mono 兼容桥接层的设计、实现与数据流。

---

## 1. 概述

tomcat 通过定制 `wasmedge_quickjs.wasm`（新增 `host_call.rs`）将宿主的 `env.__pi_host_call` 暴露为 JS 全局函数。在此基础上，`assets/js/pi_bridge.js` 构建 pi-mono 兼容的 `globalThis.pi` 对象，使扩展插件无需修改即可运行。

## 2. 数据流

```
JS 插件代码
    ↓ pi.readFile("/tmp/x")
globalThis.pi  (pi_bridge.js)
    ↓ hostCall("fs", "readFile", {path: "/tmp/x"})
globalThis.__pi_host_call(requestJson)  (wasmedge_quickjs host_call.rs)
    ↓ env.__pi_host_call(buf_ptr, req_len, buf_cap) -> resp_len
宿主 host_call_impl  (instance_wasmedge.rs)
    ↓ host_invoke(request_json)
HostApiDispatcher  (dispatcher.rs)
    ↓ 按 module/method 路由
具体 Processor (PrimitiveExecutor / LlmProvider / EventBus / ...)
```

## 3. 组件说明

### 3.1 定制 wasmedge_quickjs.wasm

源码位于 `Tomcat/wasmedge-quickjs/`（基于 second-state/wasmedge-quickjs fork）：

- `src/host_call.rs`：声明 `#[link(wasm_import_module = "env")] extern "C" { fn __pi_host_call(...) }`，实现 `PiHostCallFn`（JsFn），将 JS 字符串参数序列化到线性内存 buffer、调用宿主、读取响应。
- `src/main.rs`：在 `eval_buf` 前调用 `host_call::register_pi_host_call(ctx)`，将 `__pi_host_call` 注册为 JS 全局函数。

构建命令（无 TLS）：
```bash
scripts/build-custom-quickjs.sh
# 或手动：
cd wasmedge-quickjs && cargo +stable build --release --no-default-features --bin wasmedge_quickjs
cp target/wasm32-wasip1/release/wasmedge_quickjs.wasm ../tomcat/assets/wasm/
```

### 3.2 ABI：`__pi_host_call(buf_ptr, req_len, buf_cap) -> resp_len`

- 宿主从 `buf_ptr` 读取 `req_len` 字节请求 JSON
- 宿主将响应写回 `buf_ptr`（不超过 `buf_cap` 字节）
- 返回实际响应长度；若 `resp_len > buf_cap`，Guest 侧可以更大 buffer 重试

### 3.3 pi_bridge.js

位于 `assets/js/pi_bridge.js`，由 `run_script_file_impl` 自动预加载（拼接在用户脚本前）。路径解析顺序：
1. 环境变量 `PI_BRIDGE_JS_PATH`
2. `quickjs_path` 的兄弟目录 `../js/pi_bridge.js`

提供的全局对象与函数：

| 全局标识 | 用途 |
|---------|------|
| `globalThis.pi` | pi-mono 兼容 API 对象 |
| `globalThis.__pi_dispatch_event(eventJson)` | 宿主调用以分发事件到 JS handler |
| `globalThis.__pi_execute_tool(toolCallJson)` | 宿主调用以执行 JS 注册的工具 |

### 3.4 pi 对象 API 映射

| pi-mono API | pi_bridge.js 方法 | Dispatcher 路由 |
|------------|------------------|----------------|
| `pi.on(event, handler)` | `pi.on()` | `events.subscribe` |
| `pi.exec(cmd)` | `pi.exec()` | `fs.executeBash` |
| `pi.readFile(path)` | `pi.readFile()` | `fs.readFile` |
| `pi.writeFile(path, content)` | `pi.writeFile()` | `fs.writeFile` |
| `pi.editFile(path, edits)` | `pi.editFile()` | `fs.editFile` |
| `pi.registerTool(def)` | `pi.registerTool()` | `tools.registerTool` |
| `pi.registerCommand(name, opts)` | `pi.registerCommand()` | `tools.registerCommand` |
| `pi.createChatCompletion(params)` | `pi.createChatCompletion()` | `llm.createChatCompletion` |
| `pi.session.getCurrent()` | `pi.session.getCurrent()` | `session.getCurrentSession` |
| `pi.sendMessage(msg)` | `pi.sendMessage()` | `agent.sendMessage` |
| `pi.sendUserMessage(content)` | `pi.sendUserMessage()` | `agent.sendUserMessage` |
| `pi.log(msg)` | `pi.log()` | `agent.log` |

## 4. 事件分发机制

### 4.1 宿主 → JS 事件分发

`WasmInstance::dispatch_event(plugin_script, event_type, data, context)`:

1. 读取插件脚本代码
2. 构造事件 envelope JSON：`{ type, data, context }`
3. 拼接 `__pi_dispatch_event(envelope)` 调用
4. 通过 `run_script` 执行（自动预加载 pi_bridge.js）

由于 WasmEdge QuickJS VM 是短生命周期的（每次执行新建），每次事件分发会重新执行插件脚本（注册 handler）然后触发事件。

### 4.2 ctx 代理对象

`__pi_dispatch_event` 为每个 handler 构建 `ctx` 对象：

**静态属性**（来自 envelope.context 快照）：
- `cwd: string`
- `hasUI: boolean`
- `model: object | null`

**动态方法**（每次调用触发 hostCall）：
- `isIdle() -> boolean` — `context.isIdle`
- `abort()` — `context.abort`
- `hasPendingMessages() -> boolean` — `context.hasPendingMessages`
- `shutdown()` — `context.shutdown`
- `getSystemPrompt() -> string` — `context.getSystemPrompt`
- `getContextUsage() -> object` — `context.getContextUsage`
- `compact(options?)` — `context.compact`

**嵌套对象**：
- `ui.notify(msg, type)` — `context.uiNotify`
- `ui.select(title, options)` — `context.uiSelect`
- `ui.confirm(title, msg)` — `context.uiConfirm`
- `ui.input(title, placeholder)` — `context.uiInput`
- `sessionManager.getCurrent()` — `session.getCurrentSession`

## 5. 工具执行

`__pi_execute_tool(toolCallJson)` 允许宿主调用 JS 侧注册的工具：

```json
{ "toolName": "my_tool", "toolCallId": "id-1", "params": {} }
```

返回 `{ "ok": true, "data": <result> }` 或 `{ "ok": false, "error": "..." }`。

## 6. 与实现文件的对应关系

| 文件 | 职责 |
|------|------|
| `wasmedge-quickjs/src/host_call.rs` | Wasm 侧：env.__pi_host_call 导入 + JsFn 全局注册 |
| `wasmedge-quickjs/src/main.rs` | Wasm 侧：eval_buf 前注册 pi host call |
| `assets/js/pi_bridge.js` | JS 桥接层：pi 对象 + 事件分发 + 工具执行 |
| `src/ext/instance_wasmedge.rs` | 宿主侧：host_call_impl + bridge 预加载 + dispatch_event |
| `src/ext/dispatcher.rs` | 宿主侧：module/method 路由 + context 模块 |
| `scripts/build-custom-quickjs.sh` | 构建定制 wasm 的脚本 |

---

**导航**：返回 [插件系统全貌](../plugin-system-overview.md) | 上一节：[Hostcall JSON 协议](host-call-protocol.md) | 下一节：[Host-Guest 层](host-guest-layer.md)
