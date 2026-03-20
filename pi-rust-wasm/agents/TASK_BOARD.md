# 任务总看板

---

## 当前迭代上下文

> 换迭代时只需修改本区块。

| 字段 | 值 |
|------|----|
| 当前迭代 | `001-mvp` |
| specs 规格文档 | [../openspec/specs/](../openspec/specs/)（含 Architecture.md、Constitution.md 及子文档） |
| 需求设计文档 | [../openspec/changes/001-mvp/](../openspec/changes/001-mvp/)（含 task.md、tasks_details.md、design.md） |
| 任务原子明细 | [tasks_details.md](../openspec/changes/001-mvp/tasks_details.md) |
| 技术设计 | [Architecture.md](../openspec/specs/Architecture.md)[design.md](../openspec/changes/001-mvp/design.md) |
| 技术方案（插件异步 Hostcall / 长生命周期 VM） | [async-hostcall-event-loop.md](../openspec/specs/architecture/plugin-system/async-hostcall-event-loop.md)（submit/poll，DONE）、[phase2-long-lived-vm.md](../openspec/specs/architecture/plugin-system/phase2-long-lived-vm.md)（VM actor，TASK-15 DONE） |

---

## 已完成任务（波次 1-4）

以下任务已合并到 develop，仅作依赖参考：

| 任务 ID | 名称 | 状态 |
|---------|------|------|
| T1-P0-001 | 项目骨架搭建与基础设施层落地 | DONE |
| T1-P0-002 | 全局事件总线核心实现 | DONE |
| T1-P0-003 | 存储层与会话管理模块落地 | DONE |
| T1-P0-004 | LLM 统一接入模块落地 | DONE |
| T1-P0-005 | 4 原语执行引擎核心实现 | DONE |
| T1-P0-006 | 工具注册中心核心实现 | DONE |
| T1-P0-007 | WasmEdge 运行时与 QuickJS 集成 | DONE |
| T1-P0-008 | 宿主 API 层与 JS 绑定实现 | DONE |

---

## 任务状态说明

任务**状态**取值统一使用英文，含义与典型流转如下：

| 状态 | 含义 |
|------|------|
| **TODO** | 待认领 |
| **DOING** | 开发中（已认领） |
| **PENDING_INTEGRATION** | 等待集成测试（工程师须已在功能分支按 [INTEGRATION_MERGE_AND_ACCEPTANCE.md](./INTEGRATION_MERGE_AND_ACCEPTANCE.md) 完成集成与 E2E 全量验收并推送；等待 Nibbles 合并入 develop 并复核通过） |
| **BLOCKED** | 阻塞（需在「阻塞点」中说明原因） |
| **DONE** | 已完成（含集成测试通过） |

**典型流转**：`TODO → DOING → PENDING_INTEGRATION → DONE`。阻塞时可为 `DOING` / `PENDING_INTEGRATION` → `BLOCKED` → `DOING` / `PENDING_INTEGRATION`。仅状态为 `TODO` 且负责人为空的任务可被认领；`PENDING_INTEGRATION` 表示已交集成、不可认领。

---

## 待办任务

按推荐执行顺序排列。工程师按 [Dispatcher.md](./Dispatcher.md) 流程认领。

---

### TASK-01 | T1-P0-009-completion | 插件生命周期 — 补完加载流程

| 字段 | 内容 |
|------|------|
| **优先级** | P0 |
| **状态** | `DONE` |
| **负责人** | Tom |
| **分支** | `feature/plugin-lifecycle` |
| **阻塞点** | — |

**目标**：补完插件从磁盘加载到初始化运行的完整流程（9.2），使 PluginManager 能真正加载并运行一个 pi-mono 风格插件。

**子项**（参考 tasks_details.md T1-P0-009）：
- [✓] 9.1 PluginManifest/PluginInstance/PluginStatus 定义与清单解析校验
- [✓] 9.2 完整加载流程：读取清单与 main 入口代码 → 权限校验与用户确认 → 创建 Wasm 实例 → 注册授权 API → 注入并执行插件初始化代码 → 注册到 PluginManager
- [✓] 9.3 启用/禁用：状态切换，控制事件响应与工具调用
- [✓] 9.4 卸载：EventBus.remove_plugin_listeners、ToolRegistry.unregister_plugin_tools、销毁 Wasm 实例
- [✓] 9.5 单元测试

**依赖**：T1-P0-007 (DONE)、T1-P0-008 (DONE)

**被依赖**：TASK-02 (T1-P0-010-completion)、TASK-03 (T1-P0-011)、TASK-05 (T1-P1-002)

**协作接口**：
- 消费：`WasmEngine::create_instance`、`HostApiDispatcher`、`EventBus`、`ToolRegistry`
- 提供：`PluginManager::load_plugin(path)` — 完整加载 API，供 CLI plugin 子命令与 chat 调用

**验收标准**：
- `PluginManager::load_plugin` 可从磁盘路径加载插件清单、创建 Wasm 实例、注入初始化代码并运行
- 清单非法/权限不满足/Wasm 初始化失败时错误信息清晰，宿主不崩溃、可恢复
- 加载 → 启用 → 禁用 → 卸载全流程贯通
- 单测覆盖率 >= 80%

---

### TASK-02 | T1-P0-010-completion | CLI 子命令 — 补完占位部分

| 字段 | 内容 |
|------|------|
| **优先级** | P0 |
| **状态** | `DONE` |
| **负责人** | Jerry |
| **分支** | `feature/cli-commands` |
| **阻塞点** | — |

**目标**：将 CLI 中仍为占位的子命令补充为真实实现，使 `pi-wasm` 所有子命令可正常执行。

**子项**（参考 tasks_details.md T1-P0-010）：
- [✓] 10.1 CLI 骨架（clap 子命令结构）
- [✓] 10.2 `pi-wasm init`：引导 LLM 配置、生成配置文件
- [✓] 10.3 `pi-wasm doctor`：补全 WasmEdge/QuickJS 可用性检测
- [✓] 10.4 `pi-wasm config`：补全 get(key)/set/edit 子命令
- [✓] 10.5 `pi-wasm session`：list/new/switch/delete/archive/search
- [✓] 10.6 `pi-wasm plugin`：list/load/unload/enable/disable/info，对接 PluginManager
- [✓] 10.7 `pi-wasm audit`：list/show/export，读取 tracing 日志过滤审计记录
- [✓] 10.8 完善帮助文档与参数校验

**依赖**：TASK-01 (T1-P0-009-completion)

**被依赖**：TASK-03 (T1-P0-011)

**协作接口**：
- 消费：`PluginManager`（plugin 子命令）、`AppConfig`（config 子命令）、`WasmEngine`（doctor 检测）、审计日志模块（audit 子命令）
- 提供：完整 CLI 入口，供用户与对话模式使用

**验收标准**：
- `pi-wasm doctor` 能检测 WasmEdge/QuickJS 可用性并输出修复建议
- `pi-wasm config set/edit` 可修改配置
- `pi-wasm plugin list/load/unload/enable/disable/info` 可正常执行
- `pi-wasm audit list/show/export` 可读取审计日志（或合理占位）
- 所有子命令帮助文档完整、参数校验正确
- 首次运行无配置时的提示友好

---

### TASK-03 | T1-P0-011 | CLI 对话模式核心实现

| 字段 | 内容 |
|------|------|
| **优先级** | P0 |
| **状态** | `DONE` |
| **负责人** | Spike |
| **分支** | `feature/cli-chat` |
| **阻塞点** | — |

**目标**：实现 `pi-wasm chat`（或无参数默认进入）的交互式对话模式，支持流式渲染、多轮上下文、4 原语/工具调用与用户确认。

**子项**（参考 tasks_details.md T1-P0-011）：
- [✓] 11.1 对话主循环：读取用户输入、调用 LLM、输出响应；集成 SessionManager 与 LlmProvider
- [✓] 11.2 流式响应渲染（syntect），逐字或逐块输出
- [✓] 11.3 Markdown 与代码块高亮（syntect）
- [✓] 11.4 多轮对话上下文：从当前会话加载历史、组装消息列表、写入新消息到 JSONL
- [✓] 11.5 集成 4 原语与工具调用：LLM 返回 tool_calls 时展示并调用 require_user_confirmation/工具执行，结果回传 LLM
- [✓] 11.6 快捷键：Ctrl+C 中断生成、Ctrl+D 退出、上下箭头历史导航；`--resume` 行为对齐 pi-mono
- [✓] 11.7 边界验收：会话切换后会话级 LLM/插件配置正确隔离

**依赖**：T1-P0-002 (DONE)、T1-P0-003 (DONE)、T1-P0-004 (DONE)、T1-P0-005 (DONE)、T1-P0-006 (DONE)、TASK-01 (T1-P0-009-completion)、TASK-02 (T1-P0-010-completion)

**被依赖**：TASK-07 (T1-P1-004)、TASK-08 (T1-P2-001)

**协作接口**：
- 消费：`LlmProvider`（chat/chat_stream）、`SessionManager`（会话 CRUD/上下文组装）、`PrimitiveExecutor`（4 原语）、`ToolRegistry`（工具调用）、`PluginManager`（插件联动）、`EventBus`（事件通知）
- 提供：完整 CLI 对话体验，MVP 核心交互入口

**验收标准**：
- `pi-wasm chat` 或 `pi-wasm` 可进入对话模式
- 流式输出逐字/逐块渲染，Markdown 与代码高亮
- 多轮上下文从 JSONL 加载并正确组装
- LLM 返回 tool_calls 时触发用户确认、执行并回传结果
- 用户拒绝 4 原语确认时有提示与审计记录
- 快捷键 Ctrl+C/Ctrl+D/上下箭头正常工作
- 会话切换后 LLM/插件配置正确隔离

---

### TASK-12 | T1-P0-008-async | 异步 Hostcall 与 submit/poll 机制实现

| 字段 | 内容 |
|------|------|
| **优先级** | P0 |
| **状态** | `DONE` |
| **负责人** | Jerry |
| **分支** | `feature/async-hostcall` |
| **阻塞点** | — |

**目标**：实现异步 Hostcall 的 submit/poll 机制，使 exec/LLM 等耗时调用不阻塞 Wasm 实例，利用 wasmedge_quickjs 内置事件循环自动驱动 Promise 解析。

**技术方案**：[异步 Hostcall 与事件循环设计](../openspec/specs/architecture/plugin-system/async-hostcall-event-loop.md)

**子项**（参考 tasks_details.md T1-P0-008 的 8.4）：
- [✓] 8.4.1 `dispatcher.rs`：新增 `AsyncCallStatus` + `async_results: Arc<DashMap>`
- [✓] 8.4.2 `dispatcher.rs`：改造 `dispatch()` — callId 非空时 spawn Tokio 任务，立即返回 pending
- [✓] 8.4.3 `dispatcher.rs`：新增 `__async.poll` 路由
- [✓] 8.4.4 `instance_wasmedge.rs`：`dispatch()` 改用共享 Tokio Handle
- [✓] 8.4.5 异步任务超时控制（默认 30s）
- [✓] 8.4.6 实例销毁时清理 pending 异步任务
- [✓] 8.4.7 并发模型优化（Session 分片锁、LLM Semaphore）
- [✓] 8.4.8 单元测试 + 集成测试

**依赖**：T1-P0-008 (DONE)

**被依赖**：TASK-13 (JS API 对齐)

**涉及文件**：
- `src/ext/dispatcher.rs`：核心改动（异步任务管理 + poll 路由）
- `src/ext/instance_wasmedge.rs`：共享 Tokio Handle

**验收标准**：
- 带 `callId` 的 hostcall 请求立即返回 `{pending: true}`，后台 Tokio 任务异步执行
- `__async.poll` 路由可正确返回 `{ready: true/false}` 及结果
- 异步任务超时后返回错误
- 实例销毁时无 pending 任务泄漏
- 多 callId 并发场景稳定

---

### TASK-13 | T1-P0-008-jsapi | JS API 与 pi-mono 对齐

| 字段 | 内容 |
|------|------|
| **优先级** | P0 |
| **状态** | `DONE` |
| **负责人** | Tom |
| **分支** | `feature/js-api-alignment` |
| **阻塞点** | — |

**目标**：pi_bridge.js 的 `globalThis.pi` 接口对齐 pi-mono `ExtensionAPI`，核心 API 返回 Promise，修复已知 bug。

**技术方案**：[JS API 与 pi-mono 对齐设计](../openspec/specs/architecture/plugin-system/js-api-alignment.md)

**子项**（参考 tasks_details.md T1-P0-008 的 8.7）：
- [✓] 8.7.1 `pi_bridge.js`：新增 `hostCallAsync` 函数（submit/poll 包装，返回 Promise）
- [✓] 8.7.2 `exec` / `createChatCompletion` 改为调用 `hostCallAsync`，返回值解包为 pi-mono 格式
- [✓] 8.7.3 修复 `off` / `emit` 重复定义 bug
- [✓] 8.7.4 新增 `pi.once(event, handler)`
- [✓] 8.7.5 集成测试：`await pi.exec("echo hello")`
- [✓] 8.7.6 集成测试：`await pi.createChatCompletion({...})`
- [✓] 8.7.7 （P1）readFile/writeFile/editFile 返回 Promise
- [✓] 8.7.8 （P1）新增 setModel/getModel/complete/unregisterTool

**依赖**：TASK-12 (T1-P0-008-async)

**被依赖**：TASK-05 (pi-mono 插件兼容性测试)

**涉及文件**：
- `assets/js/pi_bridge.js`：核心改动

**验收标准**：
- `pi.exec("...")` 返回 Promise，`await` 可正确获取 `{stdout, stderr, exitCode}`
- `pi.createChatCompletion({...})` 返回 Promise，结果格式与 pi-mono 一致
- `off`/`emit` 无重复定义
- `pi.once` 可用
- pi-mono 风格插件代码 `const result = await pi.exec("ls")` 可正常运行

---

### TASK-14 | T1-P1-005 | Agent Loop 核心结构化实现

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `DONE` |
| **负责人** | Spike |
| **分支** | `feature/agent-loop` |
| **阻塞点** | — |

**目标**：将现有 chat_loop + do_chat_turn 重构为正式的三层 AgentLoop 结构体，实现 Steering/FollowUp/Abort 中断机制、AgentEvent 完整发布、错误分类与自动重试。

**技术方案**：[Agent Loop 设计](../openspec/specs/architecture/agent-loop.md)

**子项**（参考 tasks_details.md T1-P1-005）：
- [✓] 5.1 AgentMessage 枚举与 convert_to_llm_format() 转换边界
- [✓] 5.2 AgentLoop 结构体（src/core/agent_loop.rs），三层循环骨架
- [✓] 5.3 Steering 机制（steer()，每工具后检查）
- [✓] 5.4 FollowUp 机制（follow_up()，第一层尾部检查）
- [✓] 5.5 Abort 信号（abort()，AtomicBool，每工具前检查）
- [✓] 5.6 AgentEvent 全生命周期节点发布（agent_start/end, turn_*, message_*, tool_execution_*）
- [✓] 5.7 错误分类与 Retryable 指数退避重试
- [✓] 5.8 重构 src/api/chat.rs → AgentLoop::run()
- [✓] 5.9 单元测试（Loop 状态机、Steering 时序、事件顺序）

**依赖**：T1-P0-011（DONE，TASK-03）

**被依赖**：T1-P1-002（插件兼容性测试依赖完整 Loop）

**协作接口**：
- 消费：LlmProvider、SessionManager、PrimitiveExecutor、ToolRegistry、EventBus
- 提供：AgentLoop::run()、steer()、follow_up()、abort() 公开 API

**验收标准**：
- Steering：执行工具批次中途发送消息，当前工具完成后跳过剩余工具并继续
- FollowUp：Agent 回答后追加消息，在同一会话上下文无缝继续
- Abort：abort() 调用后当前工具完成即终止，发布 agent_end(interrupted)
- 事件：agent_start/end、turn_start/end、tool_execution_start/end 均正确发布
- 重试：RateLimit/Timeout 自动退避重试 ≤ MAX_ATTEMPTS 次；401 立即终止
- 工具错误：不终止 Loop，错误内容回注 LLM
- 覆盖率 ≥ 80%

---

### TASK-15 | T1-P1-006 | 长生命周期 VM 实现

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `DONE` |
| **负责人** | Tom |
| **分支** | `develop`（已合并 feature/long-lived-vm） |
| **阻塞点** | — |

**目标**：按 phase2-long-lived-vm.md 收敛定版实现 VM actor 模型与 session 维度管理，使插件状态跨事件保持，支持 pi-mono 核心状态插件。

**技术方案**：
- [Phase 2 长生命周期 VM 方案设计](../openspec/specs/architecture/plugin-system/phase2-long-lived-vm.md)
- [异步 Hostcall 与事件循环设计 11.7](../openspec/specs/architecture/plugin-system/async-hostcall-event-loop.md)

**子项**（参考 tasks_details.md T1-P1-006）：
- [x] 15.1 结构改造：长寿命运行单元，解耦启动与事件分发
- [x] 15.2 RuntimeManager：session_id + plugin_id 双键，lookup/lazy_init/remove
- [x] 15.3 PluginManager 升级为 session 维度实例管理
- [x] 15.4 VM actor 命令通道（Init/DispatchEvent/Shutdown）+ spawn_blocking 专属线程
- [x] 15.5 dispatcher.rs 新增 __session.waitForEvent 路由与有界 channel
- [x] 15.6 _start 常驻循环：lazy start + setTimeout(loop, 0) + Shutdown 退出
- [x] 15.7 废弃组合脚本 + __pi_dispatch_event 模式，改为 channel send
- [x] 15.8 队列上限/回压、超时、session_end 清理与 Error 恢复
- [x] 15.9 单元+集成测试

**依赖**：TASK-12 (DONE)、TASK-13 (DONE)

**被依赖**：TASK-05a（pi-mono 插件兼容性 Phase 0，跨事件状态保持为前置）

**验收标准**：
- 插件全局变量可跨事件保持
- 已注册 handler 在多次事件中持续有效
- setInterval 在会话期间稳定运行
- 多会话上下文隔离（状态不串会话）
- 关闭流程无悬挂线程、无 pending 泄漏

---

### TASK-04 | T1-P1-001 | 审计日志系统完整落地

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `DONE` |
| **负责人** | Tom |
| **分支** | `feature/audit-log` |
| **阻塞点** | — |

**目标**：实现独立审计日志模块，使所有高危操作（4 原语、工具调用、插件生命周期）可追溯、可查询、可导出。

**子项**（参考 tasks_details.md T1-P1-001）：
- [x] 1.1 独立审计日志模块：专用存储，仅追加、不可篡改；保留最近 N 天配置
- [x] 1.2 在 4 原语、工具调用、插件生命周期、高危操作等关键路径写入审计记录
- [x] 1.3 审计日志查询（按时间/类型/插件等）、导出、按策略清理
- [x] 1.4 `pi-wasm audit list/show/export` 子命令与审计模块对接
- [x] 1.5 （可选）文档说明加密存储为 TODO
- [x] 3.6.1 Architecture 增加审计日志技术方案子文档（architecture/audit-log.md + 索引）
- [x] 3.6.2 Nibbles 流程增加「合并后文档与场景库同步」步骤

**依赖**：T1-P0-005 (DONE)、T1-P0-006 (DONE)

**被依赖**：—

**协作接口**：
- 消费：`AppConfig`（审计配置）、现有审计日志占位接口
- 提供：`AuditLogger` 完整实现，供 PrimitiveExecutor/ToolRegistry/PluginManager/CLI audit 子命令使用

**验收标准**：
- 审计日志仅追加、不可篡改
- 4 原语/工具/插件操作均有完整审计记录（操作人、时间、内容、确认状态、结果）
- `pi-wasm audit` 子命令可查询、展示、导出审计日志
- 过期日志按配置自动清理

---

### TASK-05a | T1-P1-002a | pi-mono 插件兼容性 - Phase 0 技术验证与差距分析

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `PENDING_INTEGRATION` |
| **负责人** | Spike |
| **分支** | `feature/plugin-compat-phase0` |
| **阻塞点** | — |

**目标**：完成技术可行性验证和完整差距分析，为后续分层实现提供依据。

**技术方案**：[pi-mono-compat-strategy.md](../openspec/specs/architecture/plugin-system/pi-mono-compat-strategy.md)
**开发计划**：[PLAN_TASK05_PI_MONO_COMPAT.md](./plan/PLAN_TASK05_PI_MONO_COMPAT.md)

**子项**：
- [✓] a.1 恢复 pi-mono 完整工作树（确保能 npm install + tsc 编译）
- [✓] a.2 挂载 wasmedge-quickjs modules/ 目录（启用 18 个已有 Node.js 模块）
- [✓] a.3 SWC crate 集成验证（TS→JS 转译 POC）
- [✓] a.4 tps.ts 打包+加载 POC（在 wasmedge_quickjs 中执行编译后的 JS）
- [✓] a.5 ExtensionAPI 差距分析文档输出
- [✓] a.6 采样 10-15 个 pi-mono 社区扩展，输出兼容性评估矩阵

**依赖**：TASK-15 (DONE)

**被依赖**：TASK-05b

**协作接口**：
- 消费：`WasmEngine`、`instance_wasmedge.rs`、wasmedge-quickjs `modules/`
- 提供：`ts_compiler.rs` 模块、差距分析文档、扩展评估矩阵

**验收标准**：
- tps.ts 的 SWC 编译产物可在 wasmedge_quickjs 中加载（即使 API 调用失败）
- 输出完整差距分析文档（docs/reports/extension_api_gap_analysis.md）
- 输出 10+ 扩展兼容性评估矩阵

**分支侧集成/E2E**：`./scripts/run-integration-tests.sh all` 已通过（含 wasmedge E2E）；等待 Nibbles 合并入 `develop` 后按 `INTEGRATION_MERGE_AND_ACCEPTANCE.md` 复跑。

---

### TASK-05b | T1-P1-002b | pi-mono 插件兼容性 - Tier 1 纯事件监听型扩展

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `TODO` |
| **负责人** | — |
| **分支** | `feature/plugin-compat-tier1` |
| **阻塞点** | — |

**目标**：使纯事件监听 + notify 的 pi-mono 扩展能零修改运行（如 tps.ts）。

**子项**：
- [ ] b.1 改造扩展入口：支持 `export default function(pi)` 模式
- [ ] b.2 对齐 `pi.on(event, handler)` handler 签名（传 ctx 参数）
- [ ] b.3 实现最小 ctx 对象（hasUI、cwd、ui.notify）
- [ ] b.4 对齐事件类型名（agent_start/agent_end 等 pi-mono 映射）
- [ ] b.5 tps.ts 端到端测试（零修改加载 + 事件触发 + notify 回调）
- [ ] b.6 固化为自动化 E2E 测试

**依赖**：TASK-05a

**被依赖**：TASK-05c

**协作接口**：
- 消费：`ts_compiler.rs`、`pi_bridge.js`、`HostApiDispatcher`
- 提供：ctx 对象构建、事件名映射、扩展入口加载器

**验收标准**：
- tps.ts 零修改（仅 SWC 编译）在 pi-rust-wasm 上运行
- agent_start/agent_end 事件正确触发，ctx.ui.notify() 正确回调宿主
- 自动化 E2E 测试覆盖

---

### TASK-05c | T1-P1-002c | pi-mono 插件兼容性 - Tier 2 命令+exec+基础 UI

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `TODO` |
| **负责人** | — |
| **分支** | `feature/plugin-compat-tier2` |
| **阻塞点** | — |

**目标**：使含命令注册、exec 调用、基础 UI 交互的扩展能运行。

**子项**：
- [ ] c.1 对齐 `pi.exec(cmd, args[], opts)` 签名
- [ ] c.2 对齐 `pi.registerCommand(name, {description, handler})`
- [ ] c.3 对齐 `pi.registerTool(toolDef)` TypeBox schema 兼容
- [ ] c.4 扩展 ctx.ui：select、confirm、input、setStatus
- [ ] c.5 对齐 `pi.sendMessage(msg, options)` 签名
- [ ] c.6 2-3 个 Tier 2 社区扩展兼容性测试
- [ ] c.7 固化为自动化 E2E 测试

**依赖**：TASK-05b

**被依赖**：TASK-05d

**协作接口**：
- 消费：`pi_bridge.js`、`HostApiDispatcher`、`ToolRegistry`
- 提供：对齐后的 exec/registerTool/registerCommand API

**验收标准**：
- 至少 2 个含 registerCommand + exec 的扩展可零修改运行
- ctx.ui 基础四件套（select/confirm/input/notify）功能正常
- 自动化 E2E 测试覆盖

---

### TASK-05d | T1-P1-002d | pi-mono 插件兼容性 - Tier 3-4 TUI 组件+深度会话 API

| 字段 | 内容 |
|------|------|
| **优先级** | P2 |
| **状态** | `TODO` |
| **负责人** | — |
| **分支** | `feature/plugin-compat-tier3-4` |
| **阻塞点** | 需要 TUI 渲染框架和 SessionManager 只读接口 |

**目标**：使含 TUI 自定义组件和深度会话 API 的扩展能运行（如 diff.ts、files.ts）。

**子项**：
- [ ] d.1 实现 `ctx.ui.custom()` + TUI 组件兼容层（Container/SelectList/Text）
- [ ] d.2 实现高级 UI：setWidget、setFooter、setHeader、editor
- [ ] d.3 实现 `ctx.sessionManager` 只读接口（getBranch 等）
- [ ] d.4 实现 `ctx.model` / `ctx.modelRegistry`
- [ ] d.5 diff.ts 端到端测试
- [ ] d.6 files.ts 端到端测试
- [ ] d.7 固化为自动化 E2E 测试

**依赖**：TASK-05c

**被依赖**：—

**协作接口**：
- 消费：`pi_bridge.js`、`HostApiDispatcher`、`SessionManager`
- 提供：TUI 渲染层、sessionManager/modelRegistry 兼容接口

**验收标准**：
- diff.ts、files.ts 可零修改运行（仅 SWC 编译）
- TUI 组件在终端中正确渲染
- sessionManager.getBranch() 等深度 API 可正常调用
- 自动化 E2E 测试覆盖

---

### TASK-06 | T1-P1-003 | 核心模块单元测试全覆盖

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `TODO` |
| **负责人** | — |
| **分支** | `feature/test-coverage` |
| **阻塞点** | — |

**目标**：补充单元测试使核心模块覆盖率 >= 80%、核心路径 100%。

**子项**（参考 tasks_details.md T1-P1-003）：
- [ ] 3.1 对基础设施层、宿主核心能力层、宿主 API 层、WasmEdge 层、CLI 层补充单测
- [ ] 3.2 确保全部测试用例通过；跨平台编译与测试

**依赖**：—（可随时进行）

**被依赖**：—

**协作接口**：
- 消费：所有模块的 pub API
- 提供：全量单测用例

**验收标准**：
- 核心模块覆盖率 >= 80%，核心路径 100%
- `cargo test` 全部通过
- 跨平台编译通过（至少 CI 或本地三平台各一次）

---

### TASK-07 | T1-P1-004 | 全平台兼容性测试与 bug 修复

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `TODO` |
| **负责人** | — |
| **分支** | `feature/cross-platform` |
| **阻塞点** | — |

**目标**：确保在 Windows/macOS/Linux 上全量功能正常。

**子项**（参考 tasks_details.md T1-P1-004）：
- [ ] 4.1 在三平台执行全量功能测试
- [ ] 4.2 修复平台专属 bug（路径、换行、依赖库等）
- [ ] 4.3 验证跨平台安装包构建；优化 doctor 的自动适配建议

**依赖**：TASK-03 (T1-P0-011)（建议 011 完成后再全量测试）

**被依赖**：—

**协作接口**：
- 消费：所有模块
- 提供：平台 bug 修复、doctor 适配建议

**验收标准**：
- Windows/macOS/Linux 全量功能测试通过
- 平台专属 bug 已修复
- `pi-wasm doctor` 可准确检测各平台环境并给出建议

---

### TASK-08 | T1-P2-001 | CLI 交互体验优化

| 字段 | 内容 |
|------|------|
| **优先级** | P2 |
| **状态** | `TODO` |
| **负责人** | — |
| **分支** | `feature/cli-ux` |
| **阻塞点** | — |

**目标**：优化 CLI 交互体验，提升流畅度与可用性。

**子项**（参考 tasks_details.md T1-P2-001）：
- [ ] 1.1 优化流式渲染流畅度（节律、刷新率等）
- [ ] 1.2 优化 diff 预览与用户确认交互
- [ ] 1.3 新增子命令/参数自动补全（shell completion）
- [ ] 1.4 统一优化错误提示文案，给出可操作的修复建议
- [ ] 1.5 为耗时操作新增加载状态与进度提示

**依赖**：TASK-03 (T1-P0-011)

**被依赖**：—

**协作接口**：
- 消费：CLI 模块、LlmProvider（流式）、PrimitiveExecutor（diff 预览）
- 提供：优化后的 CLI 交互体验

**验收标准**：
- 流式渲染更流畅，无明显卡顿
- diff 预览清晰可读，确认流程便捷
- shell completion 可用
- 错误提示含可操作的修复建议

---

### TASK-09 | T1-P2-002 | 插件安全扫描基础能力

| 字段 | 内容 |
|------|------|
| **优先级** | P2 |
| **状态** | `TODO` |
| **负责人** | — |
| **分支** | `feature/plugin-security` |
| **阻塞点** | — |

**目标**：在插件加载前增加安全扫描，拦截风险插件。

**子项**（参考 tasks_details.md T1-P2-002）：
- [ ] 2.1 静态检查恶意模式、越权 API 使用、敏感信息泄露风险（规则可配置）
- [ ] 2.2 风险插件拦截并提示用户，不静默加载；可选"强制加载"二次确认

**依赖**：TASK-01 (T1-P0-009-completion)

**被依赖**：—

**协作接口**：
- 消费：`PluginManifest`、插件入口代码、`SecurityConfig`
- 提供：安全扫描接口，嵌入 `PluginManager::load_plugin` 流程

**验收标准**：
- 加载插件前自动执行安全扫描
- 风险插件被拦截并给出明确提示
- 提供"强制加载"二次确认选项
- 扫描规则可配置

---

### TASK-10 | T1-P3-001 | 项目文档编写

| 字段 | 内容 |
|------|------|
| **优先级** | P3 |
| **状态** | `TODO` |
| **负责人** | — |
| **分支** | `feature/docs` |
| **阻塞点** | — |

**目标**：编写项目 README、用户使用文档、插件开发文档、API 文档、部署指南。

**子项**（参考 tasks_details.md T1-P3-001）：
- [ ] 1.1 README.md：简介、快速开始、构建与运行、目录结构
- [ ] 1.2 用户使用文档：安装、配置、各子命令使用说明
- [ ] 1.3 插件开发文档：清单格式、宿主 API、事件、工具注册、4 原语使用示例
- [ ] 1.4 API 文档（或指向 design/Architecture 中 Trait 与结构说明）
- [ ] 1.5 部署与安装指南：各平台依赖、安装包使用、环境变量与配置路径

**依赖**：—（可随时进行，但建议主要功能稳定后）

**被依赖**：—

**协作接口**：—

**验收标准**：
- README 简洁完整，新用户可按指引快速上手
- 各子命令使用文档齐全
- 插件开发文档含完整示例

---

### TASK-11 | T1-P3-002 | 示例插件开发

| 字段 | 内容 |
|------|------|
| **优先级** | P3 |
| **状态** | `TODO` |
| **负责人** | — |
| **分支** | `feature/example-plugins` |
| **阻塞点** | — |

**目标**：开发示例插件，覆盖工具注册、事件监听、4 原语调用，作为兼容性测试与开发者参考。

**子项**（参考 tasks_details.md T1-P3-002）：
- [ ] 2.1 至少 3 个示例插件：工具注册与调用、事件监听、4 原语调用
- [ ] 2.2 为示例插件补充注释与 README

**依赖**：TASK-01 (T1-P0-009-completion)

**被依赖**：—

**协作接口**：
- 消费：宿主 API（pi 全局对象）、PluginManifest 格式
- 提供：示例插件代码与文档

**验收标准**：
- 至少 3 个示例插件可正常加载并运行
- 分别覆盖工具注册、事件监听、4 原语调用
- 每个插件含注释与 README
