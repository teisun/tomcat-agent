# pi-rust-wasm 整体技术架构

## 设计原则

1.  **pi 生态全兼容**：以 pi-mono 为兼容性契约，API、事件机制、插件规范与社区插件零修改运行为目标。
2.  **安全隔离优先**：所有插件代码运行在 WasmEdge 独立沙箱内，宿主可信逻辑与插件不可信逻辑完全分离，仅通过显式注册的 API 通信。
3.  **极简分层**：严格遵循单向依赖、无循环依赖原则，核心层仅保留可信基础能力，所有扩展能力均通过插件实现。
4.  **原生性能**：Rust 宿主层负责核心调度与可信逻辑，WasmEdge 负责沙箱内 JS/TS 代码执行，兼顾生态兼容性与原生性能。
5.  **可插拔设计**：所有非核心能力均通过插件化实现，不耦合主程序核心逻辑，支持按需启用/禁用。

### pi 生态参考原则（双仓对照）

所有影响「兼容 pi 生态」的技术设计，**必须同时参考 [pi-mono](../../../pi-mono/) 与 [pi-agent-rust](../../../pi_agent_rust/) 两个仓库**，并遵循以下分工：

- **pi-mono**：作为**兼容性契约与行为基准**。扩展作者面向的是 TypeScript/JS 的 API、事件名、会话与 RPC 协议；「与 pi 生态兼容」的最终标准是**与 pi-mono 的对外行为与接口一致**。事件名、API 形态、payload 结构、协议语义等以 **pi-mono 为权威**。
- **pi-agent-rust**：作为 **Rust 侧的主要实现参考**。事件拆分（AgentEvent / ExtensionEvent）、hostcall 设计、扩展加载与 QuickJS 集成、会话/工具/权限等实现可优先参考 pi-agent-rust；其已与 pi-mono 对齐的部分可直接沿用。
- **二者不一致时**：以 **pi-mono 的语义为准**，在 pi-rust-wasm 中按 pi-mono 实现，再在 pi-rust-wasm 里用 Rust 实现出来, 不把 pi-agent-rust 的当前行为当作最终标准（pi-agent-rust 的 drop-in 认证当前为 NOT_CERTIFIED，存在已知差距）。

## 整体分层架构

从宿主可信层到沙箱插件层，单向依赖、边界清晰，架构层级从下到上依次为：
**基础设施层 → 宿主核心能力层 → 宿主API层 → WasmEdge运行时层 → 沙箱执行层 → 交互层**

## 各层核心模块详细设计

### 1. 基础设施层

项目的底层可信基础能力，所有上层模块均依赖该层，无任何业务逻辑，保证跨平台通用，完全基于 Rust 安全实现；包含统一错误处理、配置管理、日志与审计、跨平台适配、事件总线。

详见 [1. 基础设施层（详细）](architecture/infrastructure-layer.md)。

### 2. 宿主核心能力层

项目的可信核心引擎，所有业务逻辑的底层支撑，仅在宿主层运行，不向插件开放直接访问权限；包含会话管理（含 Transcript 约定）、LLM 接入、4 原语执行引擎、工具注册中心、插件生命周期管理、权限管控。

详见 [2. 宿主核心能力层（详细）](architecture/host-core-layer.md)。

### 3. 宿主API层

宿主向插件开放的唯一可信接口，完全对齐 pi-mono ExtensionAPI 规范；包含核心 Agent API 表、Node.js 兼容层、统一 Hostcall 通信协议（含高并发分发、异步 Hostcall、细粒度锁定及 AI 实现指导）。**Hostcall 请求/响应 JSON 协议**（HostRequest、HostResponse、module/method 与 params 约定）见子文档。

详见 [3. 宿主API层（详细）](architecture/host-api-layer.md)。Hostcall 与 Guest 的 JSON 协议以 [Hostcall JSON 协议（子文档）](architecture/host-call-protocol.md) 为准，实现须与其中请求/响应格式及 module/method/params 约定一致。pi-mono 兼容桥接层（`pi_bridge.js`、定制 `wasmedge_quickjs.wasm`、事件分发与 ctx 代理）见 [JS 桥接层架构](architecture/js-bridge-layer.md)。

### 4. WasmEdge运行时层

项目的沙箱隔离核心，基于 WasmEdge 官方构建，全局单例 Engine，每个插件对应独立的 Store/Instance；包含核心组件、插件加载执行流程、内存安全与数据交换、并发调度模型、资源与内存模式（MemoryProfile 表、零拷贝与流式、十一期动态切换）。

详见 [4. WasmEdge运行时层（详细）](architecture/wasmedge-runtime-layer.md)。

#### 4.5 资源与内存模式 (Resource & Memory Profile)

资源上限依 MemoryProfile 派生（Low/Standard/High/Auto），参数表与运行时动态切换见详细文档。详见 [4.5 资源与内存模式（详细）](architecture/wasmedge-runtime-layer.md#45-资源与内存模式-resource--memory-profile)。

### 5. 沙箱执行层

插件代码的实际运行环境，完全隔离于宿主系统，仅能通过显式注册的宿主 API 与外界交互；包含执行上下文、权限边界、资源限制、错误隔离、模块加载。

详见 [5. 沙箱执行层（详细）](architecture/sandbox-layer.md)。

### 6. 交互层

用户与引擎交互的入口，优先实现 CLI 工具，后续扩展 Web/移动端界面；包含 CLI 交互层、IPC 接口层、前端交互层（预留）。

详见 [6. 交互层（详细）](architecture/interaction-layer.md)。

### 7. 安全设计核心原则

最小权限、完全隔离、唯一通道、用户知情权、错误隔离、全链路审计、代码安全校验、资源硬配额（内存隔离、执行时限、API 限流）；敏感数据加密为后续 TODO。

详见 [7. 安全设计核心原则（详细）](architecture/security.md)。

## 8. 事件系统设计（替代原钩子设计，完全对齐pi-agent-rust）

基于发布-订阅的全局事件总线，支持同步/异步监听；事件分为 AgentEvent（流式/UI）与 ExtensionEvent（扩展钩子），与 pi-mono / pi_agent_rust 一致；扩展通过字符串事件名注册钩子，宿主在关键节点发布事件，单次回调错误不影响主流程。

详见 [事件系统设计（详细）](architecture/events.md)。

## 9. 会话存储数据结构设计

会话采用元数据 store（sessions.json，sessionKey → SessionEntry）与对话 transcript（pi 系 JSONL）两层；列表与路由由 sessions.json 提供，transcript 按需流式读取、最近 N 条、零拷贝解析；SessionEntry、SessionHeader、EntryBase 及会话路径与 sessionKey/sessionId 约定见详细文档。

详见 [会话存储数据结构设计（详细）](architecture/session-storage.md)。

### 10. 工作目录与数据布局

默认工作根目录为可执行文件目录下的 `.pi_wasm`，可配置；多 agent 子目录（sessions、plugins、tmp、logs）及全局 wasm、全局 plugins 目录约定、启动时创建、与现有 storage/plugins 配置的兼容见详细文档。

详见 [工作目录与数据布局（详细）](architecture/work-dir-and-data-layout.md)。

### 11. 异步 Hostcall 与事件循环设计

针对 LLM 调用、命令执行等耗时 Hostcall 的异步非阻塞方案。MVP 采用复用 `__pi_host_call` 的 submit/poll 模式，利用 wasmedge_quickjs 内置事件循环自动驱动 Promise 解析，无需修改 wasmedge_quickjs.wasm。宿主侧在 `HostApiDispatcher` 中新增异步任务管理，通过 `callId` 和 `__async.poll` 路由实现请求提交与结果轮询。

详见 [11. 异步 Hostcall 与事件循环设计（详细）](architecture/async-hostcall-event-loop.md)。

### 12. JS API 与 pi-mono 对齐设计

`pi_bridge.js` 的 `globalThis.pi` 接口与 pi-mono `ExtensionAPI` 的对齐方案。核心改动：`exec`/`createChatCompletion` 等耗时 API 从同步改为返回 Promise（依赖第 11 节异步 Hostcall），修复 `off`/`emit` 重复定义 bug，补齐 `once`/`setModel`/`getModel`/`complete` 等缺失 API。

详见 [12. JS API 与 pi-mono 对齐设计（详细）](architecture/js-api-alignment.md)。

### 13. Agent Loop 设计

Agent 的核心运行循环，编排 LLM 调用、工具执行、用户中断（Steering/FollowUp/Abort）、容错重试（Compaction/Backoff）的完整生命周期。采用三层嵌套循环：对话管理循环（管理用户输入与持久化）→ 容错重试循环（处理 ContextOverflow、RateLimit 等可恢复错误）→ 思考-行动循环（LLM 流式调用 + 工具执行 + Steering 检查）。Loop 是事件系统（第 8 节）的最大发布者，所有 AgentEvent / ExtensionEvent 的发布时机均在本节定义。

详见 [13. Agent Loop 设计（详细）](architecture/agent-loop.md)。

### 14. 多 Agent 架构设计

系统支持两个维度的多 Agent 能力：**多会话并发**（不同 session 各对应一个独立 AgentLoop 实例，共享基础设施、上下文完全隔离，通过 AgentRegistry 管理全局实例）与**主-子 Agent 编排**（主 Agent 通过 `dispatch_agent` 工具调用创建子 AgentLoop，子任务独立执行后将最终回答回注为 ToolResult，支持嵌套深度限制与级联 Abort）。设计综合参考了 openclaw（SubagentRegistry + spawnDepth）、claude-code（强上下文隔离 + 两层硬限制）、AutoGen（CancellationToken 级联取消）、LangGraph（recursion_limit 软限）的最优实践，与第 8、9、10、13 节共同构成完整的多 Agent 运行基础。

详见 [14. 多 Agent 架构设计（详细）](architecture/multi-agent.md)。

---

## 详细设计索引

| 文档 | 说明 |
|------|------|
| [architecture/infrastructure-layer.md](architecture/infrastructure-layer.md) | 基础设施层 |
| [architecture/host-core-layer.md](architecture/host-core-layer.md) | 宿主核心能力层 |
| [architecture/host-api-layer.md](architecture/host-api-layer.md) | 宿主API层 |
| [architecture/wasmedge-runtime-layer.md](architecture/wasmedge-runtime-layer.md) | WasmEdge运行时层 |
| [architecture/sandbox-layer.md](architecture/sandbox-layer.md) | 沙箱执行层 |
| [architecture/interaction-layer.md](architecture/interaction-layer.md) | 交互层 |
| [architecture/security.md](architecture/security.md) | 安全设计核心原则 |
| [architecture/events.md](architecture/events.md) | 事件系统设计 |
| [architecture/session-storage.md](architecture/session-storage.md) | 会话存储数据结构设计 |
| [architecture/work-dir-and-data-layout.md](architecture/work-dir-and-data-layout.md) | 工作目录与数据布局 |
| [architecture/host-call-protocol.md](architecture/host-call-protocol.md) | Hostcall JSON 协议（请求/响应与 module/method 约定） |
| [architecture/async-hostcall-event-loop.md](architecture/async-hostcall-event-loop.md) | 异步 Hostcall 与事件循环设计 |
| [architecture/phase2-long-lived-vm.md](architecture/phase2-long-lived-vm.md) | Phase 2 长生命周期 VM 方案设计（方案 A/B 对比） |
| [architecture/js-api-alignment.md](architecture/js-api-alignment.md) | JS API 与 pi-mono 对齐设计 |
| [architecture/agent-loop.md](architecture/agent-loop.md) | Agent Loop 设计 |
| [architecture/multi-agent.md](architecture/multi-agent.md) | 多 Agent 架构设计 |
