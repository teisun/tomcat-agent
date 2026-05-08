# tomcat 整体技术架构

## 设计原则

1. **pi-mono 生态全兼容**：以 pi-mono 为兼容性契约，API、事件机制、插件规范与社区插件零修改运行为目标。
2. **安全隔离优先**：所有插件代码运行在 WasmEdge 独立沙箱内，宿主可信逻辑与插件不可信逻辑完全分离，仅通过显式注册的 API 通信。
3. **极简分层**：严格遵循单向依赖、无循环依赖原则，核心层仅保留可信基础能力，所有扩展能力均通过插件实现。
4. **原生性能**：Rust 宿主层负责核心调度与可信逻辑，WasmEdge 负责沙箱内 JS/TS 代码执行，兼顾生态兼容性与原生性能。
5. **可插拔设计**：所有非核心能力均通过插件化实现，不耦合主程序核心逻辑，支持按需启用/禁用。

### pi-mono 生态参考原则（双仓对照）

所有影响「兼容 **pi-mono** 插件与协议生态」的技术设计，**必须同时参考 [pi-mono](../../../pi-mono/) 与 [pi-agent-rust](../../../pi_agent_rust/) 两个仓库**，并遵循以下分工：

- **pi-mono**：作为**兼容性契约与行为基准**。扩展作者面向的是 TypeScript/JS 的 API、事件名、会话与 RPC 协议；「与 pi-mono 生态兼容」的最终标准是**与 pi-mono 的对外行为与接口一致**。事件名、API 形态、payload 结构、协议语义等以 **pi-mono 为权威**。
- **pi-agent-rust**：作为 **Rust 侧的主要实现参考**。事件拆分（AgentEvent / ExtensionEvent）、hostcall 设计、扩展加载与 QuickJS 集成、会话/工具/权限等实现可优先参考 pi-agent-rust；其已与 pi-mono 对齐的部分可直接沿用。
- **二者不一致时**：以 **pi-mono 的语义为准**，在 tomcat 中按 pi-mono 实现，再在 tomcat 里用 Rust 实现出来, 不把 pi-agent-rust 的当前行为当作最终标准（pi-agent-rust 的 drop-in 认证当前为 NOT_CERTIFIED，存在已知差距）。

## 整体分层架构

从宿主可信层到沙箱插件层，单向依赖、边界清晰，架构层级从下到上依次为：
**基础设施层 → 宿主核心能力层 → 宿主API层 → WasmEdge运行时层 → 沙箱执行层 → 交互层**

## 项目全貌

从用户发起一次对话到获得回复的完整路径、各层职责与关键组件的抽象关系，以及一条典型请求的调用链，见总览文档。便于快速建立整体心智模型后再进入各层详细设计。

详见 [项目全貌（详细）](../../docs/architecture/project-overview-panorama.md)。

## 各层核心模块详细设计

### 1. 基础设施层

项目的底层可信基础能力，所有上层模块均依赖该层，无任何业务逻辑，保证跨平台通用，完全基于 Rust 安全实现；包含统一错误处理、配置管理、日志与审计、跨平台适配、事件总线。**审计日志**的存储形态、仅追加与不可篡改、目录与配置、写入/查询/导出/清理及与 CLI 对接见 [审计日志设计（详细）](../../docs/architecture/audit-log.md)。

详见 [1. 基础设施层（详细）](../../docs/architecture/infrastructure-layer.md)。

### 2. 宿主核心能力层

项目的可信核心引擎，所有业务逻辑的底层支撑，仅在宿主层运行，不向插件开放直接访问权限；包含会话管理（含 Transcript 约定）、LLM 接入、4 原语执行引擎、工具注册中心、插件生命周期管理、权限管控。

详见 [2. 宿主核心能力层（详细）](../../docs/architecture/host-core-layer.md)。

### 3. 宿主API层

宿主向插件开放的唯一可信接口，完全对齐 pi-mono ExtensionAPI 规范；包含核心 Agent API 表、Node.js 兼容层、统一 Hostcall 通信协议（含高并发分发、异步 Hostcall、细粒度锁定及 AI 实现指导）。

### 4. 插件系统（统一入口）

本章集中承载 `docs/architecture/plugin-system/` 目录下全部子文档的入口，统一覆盖插件边界、协议、桥接、运行时、异步事件与演进路线，避免链接分散在多个主章节中。

#### 4.1 边界与协议

定义宿主对插件的能力边界、Hostcall 请求/响应契约及 module/method 约定，是插件与宿主通信的基础约束。

详见 [宿主API层（详细）](../../docs/architecture/plugin-system/host-api-layer.md)、[Hostcall JSON 协议（详细）](../../docs/architecture/plugin-system/host-call-protocol.md)。

#### 4.2 桥接与运行时

覆盖从 JS 桥接脚本到 Host-Guest 边界，再到 WasmEdge 实例执行的完整运行时链路。运行时侧基于 WasmEdge 官方构建，全局单例 Engine，每个插件对应独立 Store/Instance，提供内存安全与数据交换、并发调度、资源与内存模式（MemoryProfile：Low/Standard/High/Auto）能力。

详见 [JS 桥接层架构](../../docs/architecture/plugin-system/js-bridge-layer.md)、[Host-Guest 层设计](../../docs/architecture/plugin-system/host-guest-layer.md)、[WasmEdge运行时层（详细）](../../docs/architecture/plugin-system/wasmedge-runtime-layer.md)。

#### 4.3 沙箱执行

插件代码运行在与宿主隔离的沙箱环境，仅能通过显式注册 API 访问外界；同时约束执行上下文、权限边界、资源上限与错误隔离策略，避免插件故障扩散。

详见 [沙箱执行层（详细）](../../docs/architecture/plugin-system/sandbox-layer.md)。

#### 4.4 异步与事件

针对 LLM 调用、命令执行等耗时 Hostcall，采用复用 `__pi_host_call` 的 submit/poll 非阻塞模型；依托 wasmedge_quickjs 内置事件循环驱动 Promise 解析，宿主通过 `callId` 与 `__async.poll` 路由管理请求提交与结果轮询。事件系统采用发布-订阅模型，区分 AgentEvent 与 ExtensionEvent，保证单次回调失败不阻断主流程。

详见 [异步 Hostcall 与事件循环设计（详细）](../../docs/architecture/plugin-system/async-hostcall-event-loop.md)、[事件系统设计（详细）](../../docs/architecture/plugin-system/events.md)。

#### 4.5 JS API 对齐

`pi_bridge.js` 暴露的 `globalThis.pi` 与 pi-mono `ExtensionAPI` 保持语义对齐；耗时 API 统一 Promise 化，修复历史接口重复定义并补齐缺失 API，确保生态插件兼容。

详见 [JS API 与 pi-mono 对齐设计（详细）](../../docs/architecture/plugin-system/js-api-alignment.md)。

#### 4.6 对齐与演进

描述与 pi-mono 的 JS API 对齐方案以及长生命周期 VM 演进方向，确保兼容性与后续扩展能力。

详见 [插件系统全貌（详细）](../../docs/architecture/plugin-system-overview.md)、[Phase 2 长生命周期 VM 方案设计（详细）](../../docs/architecture/plugin-system/phase2-long-lived-vm.md)。

### 5. 交互层

用户与引擎交互的入口，优先实现 CLI 工具，后续扩展 Web/移动端界面；包含 CLI 交互层、IPC 接口层、前端交互层（预留）。

详见 [5. 交互层（详细）](../../docs/architecture/interaction-layer.md)。

### 6. 安全设计核心原则

最小权限、完全隔离、唯一通道、用户知情权、错误隔离、全链路审计、代码安全校验、资源硬配额（内存隔离、执行时限、API 限流）；敏感数据加密为后续 TODO。

详见 [6. 安全设计核心原则（详细）](../../docs/architecture/security.md)；其中「最小权限」与「用户知情权」在工作目录权限分级（T2-P0-004）的具体落地见 [权限子系统（PermissionGate）设计](../../docs/architecture/permission-system.md)。

### 7. 会话存储数据结构设计

会话采用元数据 store（sessions.json，sessionKey → SessionEntry）与对话 transcript（**pi-mono 相容** JSONL）两层；列表与路由由 sessions.json 提供，transcript 按需流式读取、最近 N 条、零拷贝解析；SessionEntry、SessionHeader、EntryBase 及会话路径与 sessionKey/sessionId 约定见详细文档。

详见 [会话存储数据结构设计（详细）](../../docs/architecture/session-storage.md)。

### 8. 工作目录与数据布局

默认工作根目录为 `~/.tomcat/`，可配置；多 agent 子目录（agent、sessions、logs、audit）、根级工作区（workspace-{id}）、全局目录（memory、plugins、assets 等）的约定、启动时创建、`[agent]` 配置节的覆盖规则见详细文档。

详见 [工作目录与数据布局（详细）](../../docs/architecture/work-dir-and-data-layout.md)。

### 9. Agent Loop 设计

Agent 的核心运行循环，编排 LLM 调用、工具执行、用户中断（Steering/FollowUp/Abort）、容错重试（Compaction/Backoff）的完整生命周期。采用三层嵌套循环：对话管理循环（管理用户输入与持久化）→ 容错重试循环（处理 ContextOverflow、RateLimit 等可恢复错误）→ 思考-行动循环（LLM 流式调用 + 工具执行 + Steering 检查）。Loop 是第 4.4 小节事件机制的最大发布者，所有 AgentEvent / ExtensionEvent 的发布时机均在本节定义。

详见 [9. Agent Loop 设计（详细）](../../docs/architecture/agent-loop.md)。

> Agent Loop 容错重试循环中的 ContextOverflow 路径、以及 `build_context_messages` 前的上下文预算检查，由 **上下文管理模块** 负责处理，详见 [上下文管理技术方案](../../docs/architecture/context-management.md)。

### 10. 多 Agent 架构设计

系统支持两个维度的多 Agent 能力：**多会话并发**（不同 session 各对应一个独立 AgentLoop 实例，共享基础设施、上下文完全隔离，通过 AgentRegistry 管理全局实例）与**主-子 Agent 编排**（主 Agent 通过 `dispatch_agent` 工具调用创建子 AgentLoop，子任务独立执行后将最终回答回注为 ToolResult，支持嵌套深度限制与级联 Abort）。设计综合参考了 openclaw（SubagentRegistry + spawnDepth）、claude-code（强上下文隔离 + 两层硬限制）、AutoGen（CancellationToken 级联取消）、LangGraph（recursion_limit 软限）的最优实践，与第 4、7、8、9 节共同构成完整的多 Agent 运行基础。

详见 [10. 多 Agent 架构设计（详细）](../../docs/architecture/multi-agent.md)。

---

## 详细设计索引


| 文档                                                                                                                 | 说明                                        |
| ------------------------------------------------------------------------------------------------------------------ | ----------------------------------------- |
| [docs/architecture/project-overview-panorama.md](../../docs/architecture/project-overview-panorama.md)                             | 项目全貌                                      |
| [docs/architecture/infrastructure-layer.md](../../docs/architecture/infrastructure-layer.md)                                       | 基础设施层                                     |
| [docs/architecture/audit-log.md](../../docs/architecture/audit-log.md)                                                             | 审计日志设计                                    |
| [docs/architecture/host-core-layer.md](../../docs/architecture/host-core-layer.md)                                                 | 宿主核心能力层                                   |
| [docs/architecture/plugin-system-overview.md](../../docs/architecture/plugin-system-overview.md)                                   | 插件系统全貌                                    |
| [docs/architecture/plugin-system/plugin-source-scan-register-load.md](../../docs/architecture/plugin-system/plugin-source-scan-register-load.md) | 插件来源扫描、注册与加载技术方案                         |
| [docs/architecture/plugin-system/host-api-layer.md](../../docs/architecture/plugin-system/host-api-layer.md)                       | 宿主API层                                    |
| [docs/architecture/plugin-system/host-call-protocol.md](../../docs/architecture/plugin-system/host-call-protocol.md)               | Hostcall JSON 协议（请求/响应与 module/method 约定） |
| [docs/architecture/plugin-system/js-bridge-layer.md](../../docs/architecture/plugin-system/js-bridge-layer.md)                     | JS 桥接层架构                                  |
| [docs/architecture/plugin-system/host-guest-layer.md](../../docs/architecture/plugin-system/host-guest-layer.md)                   | Host-Guest 层设计                            |
| [docs/architecture/plugin-system/wasmedge-runtime-layer.md](../../docs/architecture/plugin-system/wasmedge-runtime-layer.md)       | WasmEdge运行时层                              |
| [docs/architecture/plugin-system/sandbox-layer.md](../../docs/architecture/plugin-system/sandbox-layer.md)                         | 沙箱执行层                                     |
| [docs/architecture/plugin-system/async-hostcall-event-loop.md](../../docs/architecture/plugin-system/async-hostcall-event-loop.md) | 异步 Hostcall 与事件循环设计                       |
| [docs/architecture/plugin-system/events.md](../../docs/architecture/plugin-system/events.md)                                       | 事件系统设计                                    |
| [docs/architecture/plugin-system/js-api-alignment.md](../../docs/architecture/plugin-system/js-api-alignment.md)                   | JS API 与 pi-mono 对齐设计                     |
| [docs/architecture/plugin-system/phase2-long-lived-vm.md](../../docs/architecture/plugin-system/phase2-long-lived-vm.md)           | Phase 2 长生命周期 VM 方案设计（方案 A/B 对比）          |
| [docs/architecture/interaction-layer.md](../../docs/architecture/interaction-layer.md)                                             | 交互层                                       |
| [docs/architecture/security.md](../../docs/architecture/security.md)                                                               | 安全设计核心原则                                  |
| [docs/architecture/permission-system.md](../../docs/architecture/permission-system.md)                                             | 权限子系统（PermissionGate）— T2-P0-004 工作区权限分级 |
| [docs/architecture/session-storage.md](../../docs/architecture/session-storage.md)                                                 | 会话存储数据结构设计                                |
| [docs/architecture/work-dir-and-data-layout.md](../../docs/architecture/work-dir-and-data-layout.md)                               | 工作目录与数据布局                                 |
| [docs/architecture/agent-loop.md](../../docs/architecture/agent-loop.md)                                                           | Agent Loop 设计                             |
| [docs/architecture/multi-agent.md](../../docs/architecture/multi-agent.md)                                                         | 多 Agent 架构设计                              |
| [docs/architecture/context-management.md](../../docs/architecture/context-management.md)                                           | 上下文管理技术方案                                 |
| [docs/architecture/llm-multiprovider-integration.md](../../docs/architecture/llm-multiprovider-integration.md)                   | 多 LLM / OpenAI 对接（`LlmProvider`、Completions/Responses 边界、配置与演进） |

> **新增技术方案文档须知**：任何新增到 `docs/architecture/` 的 `*.md` 均属"技术方案文档（Architecture Spec）"，必须遵循 [`guides/workflow/ARCHITECTURE_SPEC.md`](guides/workflow/ARCHITECTURE_SPEC.md) 的章节骨架；其中 **「文件职责总览图（One-Glance Map）」为 MUST**——必须有一张 ASCII 图把方案涉及的所有业务 `*.rs` 与独立 `tests.rs` 按调用层次串起来，每节点内要点说明该文件做了什么。标杆案例：**One-Glance** 见 [`docs/architecture/tools/search_files.md` §4](../../docs/architecture/tools/search_files.md)；设有 **§4 落地选型与实施** 时 **§4.1 + §4.2** 为 MUST，见 [`guides/workflow/ARCHITECTURE_SPEC.md`](guides/workflow/ARCHITECTURE_SPEC.md) §4.1 / §4.2 与 [`docs/architecture/tools/read.md` §4.1–§4.2](../../docs/architecture/tools/read.md)；**取消 / 生命周期** 见 [`docs/architecture/interrupt-and-cancellation.md` §9.0](../../docs/architecture/interrupt-and-cancellation.md)。


