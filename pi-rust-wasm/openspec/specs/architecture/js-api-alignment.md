# 12. JS API 与 pi-mono 对齐设计

本文为 [Architecture](../Architecture.md) 中第 12 节的详细设计，总览见主文档。

---

## 12.1 对齐目标

pi-rust-wasm 的 `pi_bridge.js` 是插件与宿主之间的唯一 JS 接口层。为实现 **pi-mono 生态插件零修改运行**，需确保 `globalThis.pi` 上暴露的方法签名、返回值类型与 pi-mono `ExtensionAPI` 完全一致。

**关键差异**：pi-mono 的核心 API 多数返回 `Promise`，插件使用 `async/await` 消费；当前 pi_bridge.js 全部同步返回 JSON 对象。

---

## 12.2 当前 pi_bridge.js 与 pi-mono 对比

| API | pi-mono 类型 | 当前 pi_bridge.js | 差距 | 优先级 |
|-----|-------------|------------------|------|--------|
| `pi.exec(command, args?, options?)` | `Promise<ExecResult>` | sync `hostCall(...)` | **须改为返回 Promise** | P0 |
| `pi.readFile(path)` | `Promise<string>` | sync `hostCall(...)` | 可选改为 Promise | P1 |
| `pi.writeFile(path, content, options?)` | `Promise<WriteResult>` | sync `hostCall(...)` | 可选改为 Promise | P1 |
| `pi.editFile(path, edits)` | `Promise<EditResult>` | sync `hostCall(...)` | 可选改为 Promise | P1 |
| `pi.createChatCompletion(params)` | `Promise<CompletionResult>` | sync `hostCall(...)` | **须改为返回 Promise** | P0 |
| `pi.on(event, handler)` | sync | sync | 已对齐 | — |
| `pi.off(event, handler)` | sync | sync（重复定义，有 bug） | **修复重复定义** | P0 |
| `pi.emit(event, payload)` | sync | sync（重复定义） | **修复重复定义** | P0 |
| `pi.registerTool(toolDef)` | sync | sync | 已对齐 | — |
| `pi.registerCommand(name, options)` | sync | sync | 已对齐 | — |
| `pi.log(msg)` | sync | sync | 已对齐 | — |
| `pi.session.getCurrent()` | `Promise<Session>` | sync | 可选改为 Promise | P2 |
| `pi.session.getMessages(cap?)` | `Promise<Message[]>` | sync | 可选改为 Promise | P2 |
| `pi.sendMessage(msg, options?)` | `Promise<void>` | sync | 可选改为 Promise | P2 |
| `pi.getActiveTools()` | sync | sync | 已对齐 | — |
| `pi.setActiveTools(toolNames)` | sync | sync | 已对齐 | — |

### pi-mono 缺失 API（pi_bridge.js 尚未实现）

| API | pi-mono 类型 | 说明 | 优先级 |
|-----|-------------|------|--------|
| `pi.setModel(model)` | `Promise<void>` | 切换当前 LLM 模型 | P1 |
| `pi.getModel()` | `string` | 获取当前模型名 | P1 |
| `pi.complete(prompt, options?)` | `Promise<string>` | 简化版 LLM 调用（封装 createChatCompletion） | P1 |
| `pi.once(event, handler)` | sync | 单次事件监听 | P0 |
| `pi.unregisterTool(name)` | sync | 注销工具 | P1 |
| `ctx.setModel(model)` | 事件上下文方法 | 在事件处理器中切换模型 | P2 |

---

## 12.3 MVP 改动计划

### 12.3.1 P0 改动（异步 Hostcall 完成后立即执行）

1. **修复 `off` / `emit` 重复定义**

   当前 `pi_bridge.js` 中 `off` 和 `emit` 各定义了两次（第 34-41 行和第 47-62 行），后者覆盖前者。两个 `off` 签名不同（一个按 listenerId，一个按 handler 引用）。合并为按 handler 引用查找 + listenerId 注销。

2. **`exec` / `createChatCompletion` 改为返回 Promise**

   使用 `hostCallAsync`（见 [async-hostcall-event-loop.md](async-hostcall-event-loop.md) 11.4.4）包装：

   ```javascript
   exec: function (command, args, options) {
       return hostCallAsync('fs', 'executeBash', {
           command: command, args: args,
           cwd: options && options.cwd
       }).then(function (r) { return r.data; });
   },
   
   createChatCompletion: function (params) {
       return hostCallAsync('llm', 'createChatCompletion', params)
           .then(function (r) { return r.data; });
   },
   ```

3. **新增 `pi.once(event, handler)`**

   ```javascript
   once: function (eventName, handler) {
       var self = this;
       var wrapped = function (data, ctx) {
           self.off(eventName, wrapped);
           handler(data, ctx);
       };
       return self.on(eventName, wrapped);
   },
   ```

### 12.3.2 P1 改动（MVP 后续批次）

1. 文件操作 API（`readFile`/`writeFile`/`editFile`）改为返回 Promise（走 `hostCallAsync`，或保持同步但包装为 `Promise.resolve(result)`，与 pi-mono 签名兼容）
2. 新增 `pi.setModel` / `pi.getModel` / `pi.complete` / `pi.unregisterTool`
3. LLM 流式（`createChatCompletionStream`）的异步迭代器设计（需 Phase 2 长生命周期 VM 支持，MVP 可降级为非流式）

### 12.3.3 P2 改动（后续迭代）

1. `pi.session.*` / `pi.sendMessage` 等改为返回 Promise
2. 事件上下文 `ctx` 方法异步化
3. `ctx.setModel` 等高级上下文方法

---

## 12.4 返回值格式对齐

### ExecResult（pi-mono）

```typescript
interface ExecResult {
    stdout: string;
    stderr: string;
    exitCode: number;
}
```

当前 `hostCall('fs', 'executeBash', ...)` 返回 `HostResponse`（含 `ok`/`data`/`error`）。需在 `pi_bridge.js` 侧解包：

```javascript
exec: function (command, args, options) {
    return hostCallAsync('fs', 'executeBash', {
        command: command, args: args, cwd: options && options.cwd
    }).then(function (r) {
        if (!r.ok) throw new Error(r.error || 'exec failed');
        return r.data;  // { stdout, stderr, exitCode }
    });
},
```

### CompletionResult（pi-mono）

```typescript
interface CompletionResult {
    message: { role: string; content: string; };
    usage?: { promptTokens: number; completionTokens: number; totalTokens: number; };
}
```

同理，在 `pi_bridge.js` 侧将 `HostResponse.data` 解包为 `CompletionResult` 格式。

---

## 12.5 与异步 Hostcall 的关系

| 依赖项 | 说明 |
|--------|------|
| [async-hostcall-event-loop.md](async-hostcall-event-loop.md) | `hostCallAsync` 函数由异步 Hostcall 设计提供 |
| dispatcher.rs 异步任务管理 | 宿主侧需完成 submit/poll 机制 |
| wasmedge_quickjs 事件循环 | Promise 解析依赖 QuickJS 内置事件循环 |

**执行顺序**：异步 Hostcall 机制先行（提供 `hostCallAsync` 基础能力），然后 JS API 对齐逐步跟进。

---

## 12.6 已知的 pi_bridge.js Bug

1. **`off` 重复定义**：第 34 行（按 listenerId）和第 47 行（按 handler 引用），后者覆盖前者
2. **`emit` 重复定义**：第 43 行和第 60 行，内容相同但冗余
3. **`exec` 返回 HostResponse 而非 ExecResult**：插件需 `res.data.stdout` 而非 `res.stdout`，与 pi-mono 不兼容
