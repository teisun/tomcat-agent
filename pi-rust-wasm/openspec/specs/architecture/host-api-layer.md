本文为 [Architecture](../Architecture.md) 中「3. 宿主API层」的详细设计，总览见主文档。

## 3. 宿主API层

宿主向插件开放的唯一可信接口，完全对齐pi-mono ExtensionAPI规范，所有API通过WasmEdge导入表显式注册到沙箱实例，未注册的API插件完全无法访问。

#### 3.1 核心Agent API（pi-mono 100%兼容）
| API分类 | 核心接口 | 说明 |
|---------|----------|------|
| 4原语API | readFile/writeFile/editFile/executeBash | 完全对齐pi-mono的4原语接口，插件唯一系统访问通道 |
| LLM调用API | createChatCompletion/createChatCompletionStream | 插件调用大模型能力的统一接口，支持流式与非流式 |
| 工具注册API | registerTool/unregisterTool/getToolList | 插件注册自定义工具，可被其他插件、对话调用 |
| 事件系统API | on/emit/off/once | 完全对齐pi-mono的事件机制，宿主与插件、插件间通信 |
| 会话API | getCurrentSession/getMessages/sendMessage | 插件获取当前会话信息、发送消息、操作上下文 |
| 配置API | getConfig/setConfig | 插件获取/更新自身配置，隔离存储 |
| 日志API | log/info/warn/error | 插件日志输出，统一接入宿主日志系统 |

#### 3.2 Node.js兼容层API
基于WasmEdge官方原生实现，覆盖pi插件高频使用的Node.js核心模块，完全对齐Node.js API规范，插件无需修改即可使用。
- 全局对象：console、Buffer、process、setTimeout/setInterval、Promise
- 内置模块：fs/path、stream、events、http/https、url、querystring、os
- 模块系统：支持CommonJS的require/import，兼容npm包加载规范

#### 3.3 统一Hostcall 通信协议

为了保证宿主与沙箱间的高效通信并降低耦合，所有 API 调用遵循以下契约：
- **机制基础**：基于 Wasm 标准的 Import/Export ABI，不自定义二进制调用栈。
- **多路复用路由器**：宿主仅向 Wasm 注册极少数核心 Import 函数（如 `__pi_host_call`），通过 `module_id` 和 `method_id` 进行逻辑分发，避免 Import 表膨胀。
- **序列化协议**：统一使用 **JSON** 作为数据交换格式。
    - **理由**：兼容 `pi-mono` 的 JS 生态，QuickJS 对 JSON 解析有原生优化，且易于调试。
- **字段契约**：严格遵循 `camelCase` 命名规范，确保 Rust 侧 `serde` 定义与 JS 侧对象字段无缝对应。
- **同步与异步处理**：
    - 简单 IO 为同步阻塞调用。
    - 耗时操作（如 LLM、网络请求）采用“请求-回调”模式或利用 WasmEdge 的异步转译机制。

##### **3.3.1 高并发分发设计 (Parallel Dispatching)**
- **无状态路由器**：`HostApiDispatcher` 必须设计为 `Send + Sync`。分发过程不应持有全局互斥锁。
- **上下文隔离 (Contextual Call)**：
  - 理论：每个 Wasm 实例在调用宿主时，必须携带自己的 `InstanceContext`。
  - 实践：宿主通过 WasmEdge 提供的 `CallingFrame` 自动识别调用者身份，从而访问该 Agent 专属的私有资源，避免全局状态竞争。

##### **3.3.2 异步非阻塞调用 (Async Hostcalls)**
- **理论**：对于 LLM 调用、网络 IO、耗时原语，必须使用 **Async Hostcall**。
- **原方案（已排除）**：利用 WasmEdge 的异步转译机制（`async_host_function` API）挂起/唤醒 Wasm 实例。**实际验证发现此 API 仅限 Linux 平台**（`#[cfg(target_os = "linux")]` 门控），macOS/Windows 不可用，不满足跨平台要求。
- **MVP 方案（已采纳）**：复用已有 `__pi_host_call` Wasm 导入的 **submit/poll 模式**。插件通过带 `callId` 的请求提交异步任务，宿主 spawn Tokio 任务后立即返回 `{pending: true}`；JS 侧通过 `setTimeout` 轮询 `__async.poll` 路由获取结果。wasmedge_quickjs 内置事件循环自动驱动 Promise 与 setTimeout 回调。**零 Wasm 改动，三个文件集中修改**。
- 完整技术设计见 [异步 Hostcall 与事件循环设计](async-hostcall-event-loop.md)。

##### **3.3.3 细粒度锁定 (Fine-grained Locking)**
- **规范**：禁止使用全局 `Mutex<AppStatus>`。
- **方案**：
  - 状态分片：按 `session_id` 对会话状态进行分片。
  - 读写分离：使用 `RwLock` 允许并行读取配置，仅在修改时阻塞。
  - 原子操作：计数器、标志位等优先使用 `std::sync::atomic`。

**AI 实现指导**：
“在实现 `src/ext/wasm/dispatcher.rs` 时，请创建一个 `HostApiManager` 结构体。它负责持有所有 `Processor`（如 `FsProcessor`, `LlmProcessor`）。它对外只暴露一个符合 WasmEdge 签名要求的 `call` 函数。在这个函数内部，先解析 JSON 参数，再根据 `method` 字符串路由到对应的 `Processor`。这种‘单入口多路复用’模式能极大简化我们未来添加新 API 的工作量。”
