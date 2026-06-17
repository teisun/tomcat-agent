# JS Bridge 与宿主 API 边界

本文为 [Architecture](../../openspec/specs/Architecture.md) 中「4. 插件系统（统一入口）」的专题页，补充 [`../plugin-system-overview.md`](../plugin-system-overview.md) 的“插件 JS 如何接入宿主”这条链路。

## 这份文档回答什么

- 插件里为什么只有 `pi.*`，没有直接暴露 `fs` / `child_process` / `os`
- `globalThis.pi`、`__pi_host_call`、`HostApiDispatcher` 分别负责什么
- 宿主如何主动向 JS 推送事件，而不是只接受 JS 过来调 hostcall
- 异步 hostcall 为什么走 submit / poll，而不是把 Rust future 直接暴露给 VM

## 文首导读：先分清两个方向

这页讨论的是两条相反方向的链路：

1. **guest -> host**：插件 JS 通过 `pi.*` / `__pi_host_call` 调宿主。
2. **host -> guest**：宿主把事件投给长生命周期 VM，桥接层在 JS 里构造 `ctx` 再调 handler。

> 说人话：不是只有“插件找宿主办事”，还有“宿主把下一条事件塞给插件继续干活”。前者是请求链，后者是投递链。

## 运行时注入顺序

`PluginVmInstance::build_combined_script()` 会按固定顺序拼接宿主脚本与用户脚本：

1. `pi_runtime_prelude.js`
2. `pi_crypto_shim.js`
3. `pi_bridge.js`
4. `pi_node_shim.js`
5. 轻量兼容 shim（如 `typebox` / `ms`）
6. 用户 `main.ts` / `main.js`
7. `pi_main_loop.js`（仅 session VM 且需要事件循环时）

Rust 侧还会额外注入：

- `__pi_host_call`
- `__pi_sleep`
- `__pi_wait_for_event`（每次返回后宿主刷新 `ExecutionGuardState`，空闲 `__tick` 不计入 `call_timeout_ms`）
- `__pi_budget_reset`
- `__pi_interrupt_reason`
- `__pi_crypto_*_native`

## 结构示意：bridge 在中间到底做什么

```text
plugin code
   │
   ▼
globalThis.pi.*
   │
   ├─ hostCall(...)              -> __pi_host_call(request_json)
   ├─ registerTool(...)          -> __pi_tools[name] = impl
   ├─ registerFunction(...)      -> __pi_functions[name] = impl
   └─ on(event, handler)         -> __pi_hooks[eventName].push(handler)
   │
   ▼
pi_bridge.js
   │
   ├─ guest -> host request path
   ├─ host -> guest event dispatch path
   └─ VM-local tool/function binding path
   │
   ▼
HostApiDispatcher
   ├─ fs.*
   ├─ tools.*
   ├─ llm.*
   ├─ events.*
   ├─ session.*
   ├─ context.*
   └─ agent.log
```

## 宿主 API 分层

边界规则很简单：

- **能留在 VM 内的纯计算尽量留在 VM 内**。
- **同步原生 crypto 不经 dispatcher**，直接走 `__pi_crypto_*_native`。
- **真正敏感的能力**，例如文件、命令、LLM、会话、事件总线，一律走 `pi.*` + hostcall。
- **`node:fs` / `node:child_process` / `node:os` 默认 fail-closed**，不把 Node 宿主能力直接塞给插件。

## `pi.*` 与 hostcall 的职责划分

| 层级 | 负责什么 | 不负责什么 | 说人话 |
|------|----------|------------|--------|
| `pi.*` | 暴露给插件作者的稳定 API 面 | 不决定宿主内部路由实现 | 这是插件作者眼里的“公共门面”。 |
| `pi_bridge.js` | 把 JS 调用包装成统一 JSON hostcall，并管理 VM 内工具 / 函数绑定与事件分发 | 不直接执行敏感宿主动作 | 它像翻译层兼接线员。 |
| `invoke_host_func_with()` | Rust 侧 hostcall 桥接入口，负责序列化 / 反序列化 | 不承载业务策略 | 它负责把请求送到真正的 dispatcher。 |
| `HostApiDispatcher` | 把请求分发到 4 原语、LLM、事件、会话等宿主能力 | 不暴露 VM 内部状态给外部直接修改 | 它才是安检口后面的办事大厅。 |

## host -> guest：事件如何推回 JS

宿主要触发插件事件时，不是让 JS 主动 poll 一条“有没有事件”，而是：

1. 宿主拿到当前事件与上下文快照；
2. 把事件投到目标 `(session_id, plugin_id)` 的 VM；
3. 桥接层在 JS 里执行 `__pi_dispatch_event(...)`；
4. 桥接层按事件名取 handler，并构造 `ctx` 代理对象；
5. handler(event, ctx) 在当前 VM 内执行。

```text
Host event happens
  -> VmActor receives event
  -> __pi_dispatch_event(envelope)
  -> build ctx proxy
  -> handler(event, ctx)
  -> optional ctx hostcalls back into dispatcher
```

## `ctx` 不是一坨快照，而是“静态 + 动态”混合代理

旧文档里最容易丢的一点，就是 `ctx` 不是简单 JSON 对象，而是**静态快照 + 动态 hostcall 代理**的混合体。

| `ctx` 内容 | 来自哪里 | 说人话 |
|------------|----------|--------|
| `cwd` / `model` / `hasUI` 等静态字段 | 事件 envelope 里的 context snapshot | 这些值在投递当下就能拍一张快照传过去。 |
| `isIdle()` / `abort()` / `getContextUsage()` | handler 内部再走 hostcall | 这些必须问宿主“此刻现在是什么状态”。 |

这也是为什么桥接层要亲自构造 `ctx`，而不是把一整个 Rust 对象直接暴露给 JS。

## guest -> host：异步 hostcall 的 submit / poll

对耗时操作，当前模型不是“把 Rust future 直接借给 QuickJS”，而是统一走两段式协议：

1. JS 侧提交请求，带 `callId`
2. 宿主立即返回 `pending`
3. JS 侧通过 `__async.poll(callId)` 轮询结果
4. dispatcher 在后台把 `Pending -> Done/Error` 收口

```text
await pi.exec("ls")
  -> hostCallAsync("fs","executeBash",{...}, callId)
  -> __pi_host_call(submit)
  -> HostResponse{pending:true}
  -> setTimeout(poll, 0/1)
  -> __pi_host_call(__async.poll)
  -> ready:false ? continue : resolve/reject
```

这样做的原因是：

- VM 边界清晰，不把宿主异步执行模型泄漏成 JS 里的隐式共享状态；
- submit / poll / timeout / interrupt 都能被统一审计；
- 对工具调用、宿主函数调用、事件循环都能复用同一套生命周期语义。

## 长生命周期 VM 的空闲 event loop 与执行预算

session VM 在 `pi_main_loop.js` / `pi_bridge.js` 里通过 `__pi_start_event_loop` 常驻运行：

```715:738:tomcat/assets/js/pi_bridge.js
  globalThis.__pi_start_event_loop = async function () {
    for (;;) {
      var raw;
      try {
        raw = await __pi_wait_for_event(50);
      } catch (hostErr) {
        // ...
        return;
      }
      // ...
      if (res.data && res.data.type === '__tick') {
        continue;
      }
```

要点：

| 路径 | 是否刷新执行预算 | 说明 |
|------|------------------|------|
| `await __pi_wait_for_event(...)` 返回 | **是**（Rust 绑定内 `guard.reset()`） | 含 `__tick`、真实事件、`__shutdown` |
| `__tick` 分支 | 否（仅 `continue`） | 依赖宿主在 wait 返回时已 reset |
| 异步 hostcall 的 poll 循环 | 是（JS 调 `__pi_budget_reset()`） | 长耗时 `pi.fetch` 等不会误杀 |
| 纯同步死循环 | 否 | 仍由 `call_timeout_ms` / `interrupt_budget` 拦截 |

> 说人话：`call_timeout_ms` 量的是「两次找宿主办事之间，JS 自己闷头跑多久」，不是「VM 开了多久」或「空闲等了多久」。详见 [`runtime-and-sandbox.md`](./runtime-and-sandbox.md) 中「call_timeout_ms 到底量什么」。

## `__pi_execute_tool` 与 `__pi_execute_function`

桥接层还承担一件很重要的事：把**静态已可见的能力**和**当前 VM 内真正可执行的实现**接起来。

| 路径 | 当前 VM 内的本地表 | 消费方 | 说人话 |
|------|------------------|--------|--------|
| `__pi_execute_tool` | `__pi_tools[name]` | LLM tool calling | 给模型看的工具，最后回到 VM 内按名字执行。 |
| `__pi_execute_function` | `__pi_functions[name]` | 宿主扩展点调用 | 给宿主自己的内部能力，不会进 LLM 工具表。 |

这再次说明：**manifest 决定“谁可见”，bridge 决定“当前 VM 里怎么找到实现”。**

## 与其他文档的关系

- 发现、scope 和何时真正起 VM：[`plugin-source-scan-register-load.md`](./plugin-source-scan-register-load.md)
- `HostRequest` / `HostResponse`、manifest 与 `tools[]` / `functions[]` 契约：[`host-call-protocol.md`](./host-call-protocol.md)
- `VmActor`、隔离、session VM 生命周期：[`runtime-and-sandbox.md`](./runtime-and-sandbox.md)
- 事件名字、AgentEvent / ExtensionEvent 线格式：[`events.md`](./events.md)
