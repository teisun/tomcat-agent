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

### pi-mono 社区插件兼容（TASK-05 系列）

**总体验收口径**：**TASK-05e** 统一收口——在 TASK-05a→05b→05c→05d 全部落地后，按 [`extension_compat_matrix.md`](../docs/reports/extension_compat_matrix.md) 对 **10–15 个** pi-mono **社区**插件完成**端到端兼容验收**（可加载、各插件约定的核心路径可跑通）。各 Tier 子任务中的「1～3 个」等为**代表性回归样本**，用于门禁与自动化；**不可替代** TASK-05e 的全矩阵验收勾选。

**技术方案**：[pi-mono-compat-strategy.md](../openspec/specs/architecture/plugin-system/pi-mono-compat-strategy.md)
**开发计划**：[PLAN_TASK05_PI_MONO_COMPAT.md](./plan/PLAN_TASK05_PI_MONO_COMPAT.md)

**本地参考源码**（Tomcat 工作区根目录，与 `pi-rust-wasm/` 并列；**默认不纳入本仓库 Git 提交**，需本地自备克隆）：[pi-mono](../../pi-mono)（上游 TypeScript 生态、`ExtensionAPI` 与社区扩展形态）、[pi_agent_rust](../../pi_agent_rust)（SWC / 扩展加载等 Rust 侧参考实现）。

---

### TASK-05a | T1-P1-002a | pi-mono 插件兼容性 - Phase 0 技术验证与差距分析

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `DONE` |
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

**develop 集成**：已合并入 `develop` 并按 `INTEGRATION_MERGE_AND_ACCEPTANCE.md` 复跑通过（见 `docs/status/develop.md` 最新 status 块）。

---

### TASK-05b | T1-P1-002b | pi-mono 插件兼容性 - Tier 1 纯事件监听型扩展

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `DONE` |
| **负责人** | Jerry |
| **分支** | `feature/plugin-compat-tier1` |
| **阻塞点** | — |

**目标**：使纯事件监听 + notify 的 pi-mono 扩展能零修改运行（如 tps.ts）。

**技术方案**：[pi-mono-compat-strategy.md](../openspec/specs/architecture/plugin-system/pi-mono-compat-strategy.md)
**开发计划**：[PLAN_TASK05_PI_MONO_COMPAT.md](./plan/PLAN_TASK05_PI_MONO_COMPAT.md)

**子项**：
- [✓] b.1 改造扩展入口：支持 `export default function(pi)` 模式
- [✓] b.2 对齐 `pi.on(event, handler)` handler 签名（传 ctx 参数）
- [✓] b.3 实现最小 ctx 对象（hasUI、cwd、ui.notify）
- [✓] b.4 对齐事件类型名（agent_start/agent_end 等 pi-mono 映射）
- [✓] b.5 tps.ts 端到端测试（零修改加载 + 事件触发 + notify 回调）
- [✓] b.6 固化为自动化 E2E 测试

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
| **状态** | `DONE` |
| **负责人** | Jerry |
| **分支** | `feature/plugin-compat-tier2`（已并入 `develop`） |
| **阻塞点** | — |

**目标**：使含命令注册、exec 调用、基础 UI 交互的扩展能运行。

**技术方案**：[pi-mono-compat-strategy.md](../openspec/specs/architecture/plugin-system/pi-mono-compat-strategy.md)
**开发计划**：[PLAN_TASK05_PI_MONO_COMPAT.md](./plan/PLAN_TASK05_PI_MONO_COMPAT.md)

**子项**：
- [x] c.1 对齐 `pi.exec(cmd, args[], opts)` 签名
- [x] c.2 对齐 `pi.registerCommand(name, {description, handler})`
- [x] c.3 对齐 `pi.registerTool(toolDef)` TypeBox schema 兼容
- [x] c.4 扩展 ctx.ui：select、confirm、input、setStatus
- [x] c.5 对齐 `pi.sendMessage(msg, options)` 签名
- [x] c.6 2-3 个 Tier 2 社区扩展兼容性测试（选自扩展矩阵；累计达成 TASK-05 总体 **10–15** 个社区插件兼容测试的一部分）
- [x] c.7 固化为自动化 E2E 测试

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
| **状态** | `DONE` |
| **负责人** | Spike |
| **分支** | `feature/plugin-compat-tier3-4` |
| **阻塞点** | — |

**目标**：使含 TUI 自定义组件和深度会话 API 的扩展能运行（如 diff.ts、files.ts）。

**技术方案**：[pi-mono-compat-strategy.md](../openspec/specs/architecture/plugin-system/pi-mono-compat-strategy.md)
**开发计划**：[PLAN_TASK05_PI_MONO_COMPAT.md](./plan/PLAN_TASK05_PI_MONO_COMPAT.md)
**本地参考源码**（Tomcat 工作区根目录，与 `pi-rust-wasm/` 并列；**默认不纳入本仓库 Git 提交**，需本地自备克隆）：[pi-mono](../../pi-mono)（上游 TypeScript 生态、`ExtensionAPI` 与社区扩展形态）、[pi_agent_rust](../../pi_agent_rust)（SWC / 扩展加载等 Rust 侧参考实现）。

**子项**：
- [x] d.0 npm 包 import 基础设施（SWC import 重写 + globalThis shim 注入）
- [x] d.1 实现 `ctx.ui.custom()` + TUI 组件兼容层（Container/SelectList/Text）
- [x] d.2 实现高级 UI：setWidget、setFooter、setHeader、editor
- [x] d.3 实现 `ctx.sessionManager` 只读接口（getBranch 等）
- [x] d.4 实现 `ctx.model` / `ctx.modelRegistry`
- [x] d.5 diff.ts 端到端测试
- [x] d.6 files.ts 端到端测试
- [x] d.7 固化为自动化 E2E 测试

**依赖**：TASK-05c

**被依赖**：TASK-05e

**协作接口**：
- 消费：`pi_bridge.js`、`HostApiDispatcher`、`SessionManager`、`ts_compiler.rs`
- 提供：npm 包 shim 层、TUI 渲染层、sessionManager/modelRegistry 兼容接口

**验收标准**：
- diff.ts、files.ts 可零修改运行（仅 SWC 编译）
- TUI 组件在终端中正确渲染
- sessionManager.getBranch() 等深度 API 可正常调用
- 自动化 E2E 测试覆盖

---

### TASK-05e | T1-P1-002e | pi-mono 社区插件矩阵端到端兼容验收

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `DONE` |
| **负责人** | Tom |
| **分支** | `feature/plugin-compat-matrix-e2e` |
| **阻塞点** | — |

**目标**：按 Phase 0 输出的扩展矩阵，对 **15 个** pi-mono **社区**插件逐一做端到端兼容验收（长生命周期 VM），**15/15 全部通过**，每个插件均有自动化 E2E 测试覆盖。

**技术方案**：[pi-mono-compat-strategy.md](../openspec/specs/architecture/plugin-system/pi-mono-compat-strategy.md)
**开发计划**：[PLAN_TASK05_PI_MONO_COMPAT.md](./plan/PLAN_TASK05_PI_MONO_COMPAT.md)
**本地参考源码**（Tomcat 工作区根目录，与 `pi-rust-wasm/` 并列；**默认不纳入本仓库 Git 提交**，需本地自备克隆）：[pi-mono](../../pi-mono)（上游 TypeScript 生态、`ExtensionAPI` 与社区扩展形态）、[pi_agent_rust](../../pi_agent_rust)（SWC / 扩展加载等 Rust 侧参考实现）。

**硬性约束**：
- **长生命周期 VM**：所有插件 E2E 验证**必须在长生命周期 VM 模式下执行**（即 WasmEdge VM 实例启动后持续存活、复用，跨多次事件/命令调用），以确保覆盖 Promise drain、ctx 状态隔离、事件累积等长生命周期特有场景。
- **插件来源**：本批次插件清单**必须全部来自 pi-mono 仓库社区扩展或官方示例**（路径见 `extension_compat_matrix.md`），不得使用本项目自建的 mock/stub 插件充数；插件源码须以**零修改**（仅允许 SWC 编译转换）方式加载验证。

**子项**：
- [x] e.1 对照 [`extension_compat_matrix.md`](../docs/reports/extension_compat_matrix.md) 锁定本批次 **15** 个社区插件清单（名称、来源、Tier、核心验证路径）
- [x] e.2 为每个插件写明「通过」判定（如：SWC 编译 → `load_plugin` → 触发约定事件/命令/工具路径）
- [x] e.3 逐插件执行验证并记录结果 → [`plugin_community_e2e_acceptance.md`](../docs/reports/plugin_community_e2e_acceptance.md)（15/15 PASS）
- [x] e.4 所有 15 个插件均在 `tests/wasmedge_e2e_tests.rs` 中有自动化 E2E 测试函数，`cargo test -j 1 --all -- --test-threads=1` 全量通过
- [x] e.5 自检完成：15/15 PASS，全量测试 0 failed，标记 `PENDING_INTEGRATION`

**依赖**：TASK-05d

**被依赖**：—

**协作接口**：
- 消费：`extension_compat_matrix.md`、TASK-05b/c/d 已对齐的加载与 API、`ts_compiler`、Wasm 插件路径
- 提供：社区兼容验收记录（文档 + 可选测试/脚本）

**验收标准**：
- **15/15** 个社区插件全部通过端到端兼容验证，不接受 PARTIAL 或 BLOCKED
- 每个插件在 `tests/wasmedge_e2e_tests.rs` 中有对应的自动化 E2E 测试函数
- 遇到缺失的 shim/API 需当场补齐，而非标记跳过
- `cargo test -j 1 --all -- --test-threads=1` 全量通过

---

### TASK-06 | T1-P1-003 | 初始化体验与资源内嵌

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `DONE` |
| **负责人** | Tom |
| **分支** | `feature/init-experience` |
| **阻塞点** | — |

**目标**：将用户首次安装从 9 步手工操作简化为「下载二进制 + `pi init`」两步完成。内嵌运行时资源；可选 `--features standalone` 打包 WasmEdge；默认开发构建使用系统库；重写 `pi init` 交互式向导。

**技术方案**：[init-experience-and-embedded-assets.md](../docs/reports/init-experience-and-embedded-assets.md)

**子项**：

**Phase 1：资源内嵌与自动释放**
- [x] 6.1 `instance_wasmedge.rs`：`include_str!("../../assets/js/pi_bridge.js")` 嵌入桥接脚本，修改 `resolve_bridge_path` 消除磁盘 / 环境变量依赖
- [x] 6.2 `config.rs`：`include_bytes!` 嵌入 `wasmedge_quickjs.wasm`（~3.3MB），修改 `resolve_quickjs_path` 增加自动释放逻辑
- [x] 6.3 `Cargo.toml` 添加 `include_dir` 依赖；`instance_wasmedge.rs`：用 `include_dir!` 嵌入 `assets/modules/`（~1.0MB, 80+ 文件），修改 `resolve_quickjs_modules_dir` 优先 `{work_dir}/assets/modules/`
- [x] 6.4 `config.rs` 新增 `ensure_embedded_assets`（统一释放入口），`cli.rs` 中 `ensure_work_dir_structure` 之后调用
- [x] 6.5 `build.rs` 新增：编译期 SHA-256 摘要生成；`ensure_embedded_assets` 实现版本比对（摘要不一致则覆盖），写入 `assets/.versions.json`
- [x] 6.6 `Cargo.toml` 添加 `fs2` 依赖；释放前 `{work_dir}/assets/.lock` 文件锁 + 原子 rename 写入

**Phase 2：WasmEdge 可选 standalone + Release 优化**
- [x] 6.7 `Cargo.toml`：`standalone` feature gate（`--features standalone` 走 bundled；默认空 features 使用系统 WasmEdge）
- [x] 6.8 `Cargo.toml` 新增 `[profile.release]`：`opt-level = "z"` / `lto = true` / `codegen-units = 1` / `strip = "symbols"`（不使用 `panic = "abort"`，因 `vm_actor.rs` 和 `event_bus.rs` 依赖 `catch_unwind`）
- [x] 6.9 Release 体积在 strip+LTO 下已验证（目标区间随 standalone 与否变化）

**Phase 3：增强 `pi init` 交互式向导**
- [x] 6.10 重写 `run_init`：集成 `dialoguer` 两步向导（LLM 配置 + 资源检查），调用 `ensure_work_dir_structure` + `ensure_embedded_assets`
- [x] 6.11 `pi init` 幂等性：已有配置询问是否覆盖（默认 N）；已有 API Key 跳过输入
- [x] 6.12 旧目录迁移：检测 `~/.pi_wasm/` → 交互式迁移到 `~/.pi_/` → 旧目录重命名 `.bak`
- [x] 6.13 `.env` 处理：`Cargo.toml` 提升 `dotenvy` 为正式依赖；`run_cli` 入口加载 `{work_dir}/assets/.env`；创建时设置 `0600` 权限
- [x] 6.14 增强 `run_doctor`：修正现有路径 bug（`work_dir/wasm/` → `work_dir/assets/wasm/`）；新增 SHA-256 展示、`.env` 权限检查

**文档同步**
- [x] 6.15 更新 `docs/user-guide.md`（安装说明、目录结构、init 流程）
- [x] 6.16 同步 `openspec/specs/architecture/work-dir-and-data-layout.md` 与 `openspec/specs/architecture/directory-structure.md`

**依赖**：—（发布场景可显式 `cargo build --release --features standalone`）

**被依赖**：TASK-07（全平台兼容性测试）、TASK-10（项目文档编写）

**协作接口**：
- 消费：`AppConfig`、`ensure_work_dir_structure`、`resolve_quickjs_path`、`resolve_bridge_path`、`resolve_quickjs_modules_dir`、`dialoguer`
- 提供：单二进制分发能力、`ensure_embedded_assets` API、增强后的 `pi init` / `pi doctor`

**验收标准**：
- 预编译二进制可在干净 macOS / Linux 环境下通过 `pi init` 一步完成初始化，无需手动安装 WasmEdge、复制 wasm 文件或设置环境变量
- `pi init` 幂等，二次运行不破坏已有配置
- `pi doctor` 全部检查项通过（配置、WasmEdge、QuickJS WASM、Node 模块、.env 权限、API Key）
- 内嵌资源版本升级后，重启自动覆盖旧文件（SHA-256 比对）
- Release 二进制体积 30-40MB
- `cargo test -j 1 --all -- --test-threads=1` 全部通过

---

### TASK-18 | stream-delta-to-eventbus | on_stream_delta 回调改为 EventBus 事件订阅

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `DONE` |
| **负责人** | Tom |
| **分支** | `develop`（本任务直接在 develop 落地） |
| **阻塞点** | — |

**目标**：移除 `AgentLoop` 上的 `on_stream_delta` 回调字段，改由 `chat.rs` 通过 `EventBus` 订阅已有的 `message_update` 事件驱动 Markdown 渲染与 stdout 输出，对齐 pi-mono 的 `emit("message_update")` 事件分发模式。消除 `AgentLoop` 对 CLI 层的直接耦合，为后续多消费者（Web UI / LSP）铺路。

**子项**：
- [x] 18.1 `chat.rs`：替换调用方 — `event_bus.on("message_update", ...)` 订阅 + import `EventContext` + `off()` 在 match 前
- [x] 18.2 `agent_loop.rs`：移除 `OnStreamDelta` 类型、`on_stream_delta` 字段、`set_on_stream_delta` 方法及回调调用点
- [x] 18.3 更新 `src/core/README.md` 中 `on_stream_delta` 相关描述
- [x] 18.4 `cargo build` + `cargo clippy` + `cargo test` 全量验证

**依赖**：TASK-14（Agent Loop，DONE）

**被依赖**：TASK-08（CLI 交互体验优化，可并行）

**协作接口**：
- 消费：`EventBus::on`/`off`（`src/infra/event_bus.rs`）、`AgentEvent::MessageUpdate`（`src/infra/events.rs`）
- 移除：`OnStreamDelta` 类型、`set_on_stream_delta` 公开 API

**验收标准**：
- `cargo build` 编译通过，无 `on_stream_delta` 残留引用
- `cargo clippy --all-targets -- -D warnings` 无新增 warning
- `cargo test --lib` + `cargo test --test cli_tests` 全量通过
- `pi chat` 流式输出行为与改动前一致

**开发计划**：Cursor 计划 `stream_delta_改为事件_b97eb771.plan.md`

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

### TASK-09 | chat-path-env | Chat / 路径 / .env 整改

> **计划全文**（流程、PLAN_SPEC 六维、风险与 E2E 要点）：Cursor 计划 `chat_路径_wasmedge_整改_d21d6d2a.plan.md`（默认路径 `~/.cursor/plans/`；本表为执行摘要）。

| 字段 | 内容 |
|------|------|
| **优先级** | P1（内含 **P0** 子项须优先交付：transcript 400、`pi chat` 遇错不整段退出） |
| **状态** | `DONE` |
| **负责人** | Jerry |
| **分支** | `feature/chat-path-env`（已合并 `develop`，`ad3409f`） |
| **阻塞点** | — |

**目标**：消除 transcript 截断导致的 OpenAI 400；单次 API/Agent 错误不结束整段 `pi chat`；`pi.config.toml` 全局 `[workspace] extra_roots` 与 `DefaultPrimitiveExecutor` 路径策略一致；`.env` 代理模板、确认框 `{agentId} · host|pluginId`、system prompt 防目录幻觉。

**子项**（与计划内 P0-a / P0-b / P1 / P2 对齐）：
- [x] **P0-a** `build_context_messages`：尾部窗口向上追溯到首条 `user`，注入 system 后为 `[system, user, …]`（[`session/manager.rs`](../src/core/session/manager.rs)）
- [x] **P0-b** `chat_loop`：`agent_loop.run` 失败时终端打印清晰错误并 `continue`；**不把 API 错误写入送给 LLM 的 transcript**（可选独立审计日志）
- [x] **P1** 从 `pi.config.toml` 加载全局 `workspace.extra_roots` 并入允许路径；测试 + [`user-guide.md`](../docs/user-guide.md)（已废弃 per-agent `ext_workspaces.json`）
- [x] **P2** `pi init` `.env` 代理注释模板或条件写入；确认框来源格式；[`build_system_prompt`](../src/core/system_prompt.rs) 约束

**依赖**：—

**被依赖**：—

**协作接口**：
- 消费：`SessionManager`、`ChatContext`、`DefaultPrimitiveExecutor`、`run_init`/`run_workspace`、OpenAI 消息组装
- 提供：合规 LLM 消息序列、可恢复的 CLI 对话、与 workspace 注册一致的四原语路径策略

**验收标准**（须按 [INTEGRATION_MERGE_AND_ACCEPTANCE.md](./INTEGRATION_MERGE_AND_ACCEPTANCE.md) 在本分支完成集成/E2E，并更新 [E2E_SCENARIO_LIBRARY.md](../openspec/specs/guides/testing/E2E_SCENARIO_LIBRARY.md) 相关条目）：
- 多轮 tool 后不因截断触发 400；模拟 API 错误后仍停留在 `u>`
- `pi workspace add` 后外部根下 `read_file`/`list_dir` 在策略内成功
- `.env`/确认框/system prompt 符合计划验收描述
- `rustfmt` / `clippy` / 相关 `cargo test` 通过；不弱化断言

**实施流程**：已按 [Dispatcher.md](./Dispatcher.md) 在 `feature/chat-path-env` 交付并由集成流程合并入 `develop`（`ad3409f`）；看板按 [Nibbles.md](./Nibbles.md) §6 收口为 `DONE`。

**说明**：openspec 中 [T1-P2-002 插件安全扫描](../openspec/changes/001-mvp/tasks_details.md) 原对应本 TASK-09；**现 TASK-09 已改为本整改**。插件安全扫描若仍要做，须在 TASK_BOARD **另起新 TASK** 引用 `T1-P2-002`，勿与本条混用。

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

---

### TASK-12 | user-guide-remediation | user-guide.md 整改与 bug 修复

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `DONE` |
| **负责人** | Tom |
| **分支** | `feature/user-guide-remediation` |
| **阻塞点** | — |

**目标**：修正 user-guide.md 与实际代码的 14 项不一致，修复 2 个运行时 bug（流式输出 flush、工具调用未持久化），移除 4 项冗余功能（--config 标志、export/import、环境变量覆盖、session --cwd），新增 2 项能力（workspace 管理、插件注册表）。

**子项**：
- [x] 12.1 修复流式输出 flush（chat.rs print! 后加 flush）
- [x] 12.2 提示符改名（AI> → pi.{agentId}>，你> → u>）
- [x] 12.3 审计日志文档修正（默认开启措辞）
- [x] 12.4 移除 --config 标志 + 固定 DEFAULT_CONFIG_PATH + 测试隔离
- [x] 12.5 pi init 完成后输出 PATH 提示
- [x] 12.6 移除 config export/import 子命令 + 测试 + E2E
- [x] 12.7 移除 WASMEDGE_QUICKJS_PATH 死代码 + 环境变量文档
- [x] 12.8 修复工具调用未写入会话记录 bug
- [x] 12.9 workspace add/list/remove（持久化 `pi.config.toml` `[workspace] extra_roots`；原 ext_workspaces.json 已废弃）
- [x] 12.10 新增 plugins/registry.json 全局插件注册表

**依赖**：TASK-06（DONE）

**被依赖**：—

**协作接口**：
- 消费：`ChatContext`、`AgentLoop`、`AgentRunResult`、`SessionManager`、`PluginManager`、`AppConfig`
- 提供：workspace 管理 API、插件注册表、修正后的 CLI 行为

**验收标准**：
- `cargo build --release` 编译通过
- `cargo test --lib` + `cargo test --test cli_tests` 全量通过
- `cargo clippy` 无新增 warning
- user-guide.md 中每条操作示例与实际 CLI 行为一致
- E2E_SCENARIO_LIBRARY.md 与 tests/ 同步

---

### TASK-16 | init-3step-workspace-cwd | pi init 三步重构与 workspace add --cwd

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `DONE` |
| **负责人** | Tom |
| **分支** | `feature/user-guide-remediation` |
| **阻塞点** | — |

**目标**：`pi init` 改为 [1/3] 环境初始化 / [2/3] 资源检查（复用 doctor，跳过 API Key）/ [3/3] API Key；配置已存在默认不覆盖；PATH 自动写入 shell 配置；`pi workspace add --cwd` 添加当前目录。

**子项**：
- [x] 16.1 提取 `run_doctor_checks`，init 与 doctor 共用
- [x] 16.2 重构 `run_init` 三步 + `auto_add_to_path`
- [x] 16.3 `workspace add --cwd`
- [x] 16.4 测试与 user-guide、E2E 场景库、全量验收

**依赖**：TASK-12（可并行收尾）

**被依赖**：—

**验收标准**：见 `INTEGRATION_MERGE_AND_ACCEPTANCE.md` §1–§4；`cargo clippy --all-targets -- -D warnings`；场景库与测试同步。

---

### TASK-17 | context-management | 上下文管理（token-aware 滑窗 + Compaction）

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `DONE` |
| **负责人** | Tom |
| **分支** | `feature/context-management`（已合并入 `develop`） |
| **阻塞点** | — |

**目标**：将上下文管理从「按条数裁剪」改为 **token-aware 滑窗 + 四层防护**（Layer 0 单条截断 → Layer 1 占位符替换 → Layer 2 LLM 循环 Compaction → Layer 3 极端兜底），消除 context overflow，保留语义完整性。同时移除 `max_tool_rounds` 硬限（留 TODO 待 tool-loop-detection 方案）。

**技术方案**：[上下文管理技术方案](../openspec/specs/architecture/context-management.md)
**研究报告**：[context-management-deep-dive.md](../docs/reports/context-management-deep-dive.md)
**状态文件**：[feature-context-management.md](../docs/status/feature-context-management.md)

**子项**：

**Phase 1：基础设施与配置**
- [x] 17.1 `src/infra/config.rs`：新增 `[context]` 配置节（`context_window=400K`、`max_output_tokens=128K`、`compaction_turns=10`、`keep_recent_turns=3`、`single_tool_result_max_chars=400K`、`compaction_model="gpt-5.2"`），`PrimitiveConfig` 加载 + `pi.config.toml` 覆盖
- [x] 17.2 `src/core/session/manager.rs`：定义 `UserTurn`、`SummaryTurn`（含 `timestamp`）、`ContextState` 结构体；实现 `init_context_state`（三阶漏斗：`compute_fold_start` 预扫描 → `fold_entries_to_turns` 折叠 → `filter_turns_by_day` 当天优先 + 不足 10 向前补齐）；删除遗留 `build_context_messages` / `context_cap`
- [x] 17.3 `src/core/session/manager.rs`：`estimateContextChars` 动态维护（含 system prompt）；`on_message_appended` / `on_new_user_turn` 增量更新

**Phase 2：四层防护算法**
- [x] 17.4 `src/core/compaction.rs`（**新建**）：Layer 0 — `truncate_tool_result_if_needed`（单条 tool result 超 400K chars 截断，70%~100% 区间找换行切断）
- [x] 17.5 `src/core/compaction.rs`：Layer 1 — `compact_tool_results`（compactable zone 内从旧到新替换 role=tool 为 PLACEHOLDER，减够即停）
- [x] 17.6 `src/core/compaction.rs`：Layer 2 — `run_compaction_loop`（每批 ≤10 turns 调 LLM 生成结构化摘要，UPDATE 模式合并旧 summary，循环至回预算或 compactable zone 耗尽；失败重试 1 次 + 跳过 + 降级）
- [x] 17.7 `src/core/compaction.rs`：Layer 3 — 强制删除最旧 summary/turn 兜底（几乎不可达）
- [x] 17.8 `src/core/compaction.rs`：SUMMARIZATION_PROMPT + UPDATE_SUMMARIZATION_PROMPT 模板（Goal/Constraints/Progress/Key Decisions/Critical Context）

**Phase 3：集成与串联**
- [x] 17.9 `src/core/agent_loop.rs`：reasoning loop 工具返回后调用 Layer 0 + `on_message_appended`；移除 `max_tool_rounds` 硬限（保留配置项但默认不限制，留 TODO 注释）
- [x] 17.10 `src/core/session/manager.rs` / `src/api/chat.rs`：`build_context_messages` 改为从 `userTurnsList.flatten()` 产出，② 进入前触发 Layer 1~3 预算检查；④ 结束后打包当前 turn 追加 `userTurnsList`
- [x] 17.11 Transcript 写入：Compaction 发生时追加 `SessionEntry::Compaction` entry（append-only，原始消息不删），记录摘要正文与覆盖 turn range
- [x] 17.12 事件发布：`auto_compaction_start` / `auto_compaction_end` / `compaction_error` / `tool_result_truncated`

**Phase 4：测试**
- [x] 17.13 单元测试：`truncate_tool_result_if_needed`（正常/超限/无换行边界）、`compact_tool_results`（减够即停/全部替换仍不够）、`run_compaction_loop`（单批/多批/UPDATE 模式/失败降级）
- [x] 17.14 集成测试：大文件 read_file → Layer 0 截断不溢出；多轮对话 → Layer 1+2 自动触发；session 重载识别 Compaction entry
- [x] 17.15 `contextBudgetChars` 预算计算验证（GPT-5.2 400K → 816,000 chars）

**依赖**：TASK-14（Agent Loop DONE）、TASK-09（chat-path-env，DONE，已合入 `develop`）

**被依赖**：—

**协作接口**：
- 消费：`LlmProvider`（Compaction 摘要调用）、`SessionManager`（transcript 读写）、`EventBus`（事件发布）、`PrimitiveConfig`（配置加载）
- 提供：`ContextState`（内存上下文视图）、`compaction.rs` 四层防护 API、token-aware `build_context_messages`

**涉及文件**：

| 文件 | 改动 |
|------|------|
| `src/infra/config.rs` | 新增 `[context]` 配置节 |
| `src/core/session/manager.rs` | `UserTurn`/`ContextState` 定义；`build_context_messages` 改为 token-aware；初始化 + 动态维护 |
| `src/core/agent_loop.rs` | reasoning loop 集成 Layer 0 + 估算更新；移除 `max_tool_rounds` 硬限 |
| `src/core/compaction.rs`（**新建**） | 四层防护算法 + Compaction prompt 模板 |

**验收标准**：
- `estimateContextChars` 超 `contextBudgetChars` 时自动触发 Layer 1 → 2 → 3，不出现 context overflow 400 错误
- 单条 tool result > 400K chars 被 Layer 0 截断
- Compaction 摘要结构化（Goal/Progress/Critical Context），UPDATE 模式合并旧 summary
- 最近 3 个 user turns 不参与任何压缩/替换（protected zone）
- Session 重载正确识别 Compaction entry，不重复压缩
- transcript JSONL 仅追加，原始消息不删除
- `cargo clippy --all-targets -- -D warnings` 无新增 warning
- 单测覆盖率 ≥ 80%

---

### TASK-19 | context-management-v2 | 上下文管理重构（精确计数 + 级联降压 + 落盘）

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `DONE` |
| **负责人** | Jerry |
| **分支** | `feature/context-management-v2` |
| **阻塞点** | — |

**目标**：基于 [Claude Code 上下文管理对比分析](../docs/reports/context-management-refactoring-proposal.md)，对 TASK-17 落地的四层防护做渐进式升级——从被动单点触发改为多级 ratio 水位线主动降压，从字符估算改为 API Usage 精确计数，Layer 0 从截断丢弃改为落盘 + preview 占位符，新增 Circuit Breaker、PTL 重试、ContextMetrics 可观测性。

**技术方案**：[上下文管理技术方案 v2](../openspec/specs/architecture/context-management.md)
**重构建议报告**：[context-management-refactoring-proposal.md](../docs/reports/context-management-refactoring-proposal.md)

**子项**：

**Phase 1：精确 Token 计数（P0）**
- [ ] 19.1 `src/core/session/manager.rs`：`ContextState` 增加 `context_budget_tokens`、`last_api_usage: Option<ApiUsage>`、`post_usage_appended_chars`、`compaction_consecutive_failures` 字段；新增 `estimated_token_count()`、`usage_ratio()`、`update_api_usage()`、`invalidate_api_usage()` 方法
- [ ] 19.2 `src/core/agent_loop.rs`：reasoning loop 中捕获 `StreamEvent::Usage`，调用 `update_api_usage` 存入 `ContextState`
- [ ] 19.3 `src/core/compaction.rs`：`is_over_budget()` 改为基于 token 维度判断（有 usage 时用 token，否则 fallback 字符估算）

**Phase 2：多级水位线与级联降压（P1）**
- [ ] 19.4 `src/infra/config.rs`：`[context]` 配置节新增 `layer0_single_result_max_chars`（默认 30000）、`layer0_turn_aggregate_max_chars`（默认 150000）
- [ ] 19.5 `src/core/compaction.rs`：实现 ratio 水位线触发逻辑（0.70/0.85/0.92/0.98 → 对应 m=5/3/2/1）
- [ ] 19.6 `src/core/compaction.rs`：实现 cascade 流程编排——Layer 0 → 重算 ratio → Layer 1（若需） → 重算 ratio → Layer 2（若需） → 重算 ratio → Layer 3（若需）；每层跑完判断是否已降压成功
- [ ] 19.7 `src/core/agent_loop.rs`：每轮 LLM 回复完毕后触发 cascade（替换旧的仅 `is_over_budget` 预检）；ratio >= 0.98 时标记阻止新工具调用

**Phase 3：Layer 0 落盘 + Layer 1 改造（P1）**
- [ ] 19.8 `src/core/compaction.rs`：Layer 0 从截断改为落盘 + preview 占位符（单条 >= `layer0_single_result_max_chars` 默认 30K / 单 turn 合计 >= `layer0_turn_aggregate_max_chars` 默认 150K，均可配）；落盘路径 `{work_dir}/agents/{id}/tool-results/{tool_call_id}.txt`；preview 前 500 chars
- [ ] 19.9 `src/core/compaction.rs`：Layer 1 改为 cascade 内触发（旧 turn 0..N-m 中 tool_result > 20K chars 做占位符替换，不落盘、不每轮独立执行）
- [ ] 19.10 `src/core/compaction.rs`：Layer 2 从 `run_compaction_loop` 循环 batch 改为按 m 值一次性调用；Layer 3 目标 ratio < 0.50

**Phase 4：防御增强（P0/P1）**
- [ ] 19.11 `src/core/compaction.rs`：Circuit Breaker——`compaction_consecutive_failures` 连续 >= 3 次跳过 Layer 2，发布 `CompactionCircuitBreakerTriggered` 事件
- [ ] 19.12 `src/core/compaction.rs`：PTL 重试——摘要请求返回 context/token 错误时**取较新半段** `[mid..compactable_end)` 重试（最多 2 次），优先保留近期上下文的摘要；未覆盖的旧半段留给后续 cascade 或 Layer 3
- [ ] 19.13 `src/core/system_prompt.rs`：新增分页读取引导 section（「已落盘的工具结果可通过 offset/limit 按需读取」）

**Phase 5：可观测性与模块化（P2）**
- [ ] 19.14 `src/core/context_metrics.rs`（**新建**）：`ContextMetrics` 结构体（`input_tokens_used`、`context_utilization_ratio`、`compaction_count`、`compaction_tokens_freed`、`total_tool_result_bytes_persisted`）
- [ ] 19.15 `src/infra/events.rs`：新增 `ContextMetricsUpdate`、`CompactionCircuitBreakerTriggered`、`ToolResultPersisted` 事件类型
- [ ] 19.16 `src/core/system_prompt.rs`：模块化改造——`SystemPromptSection` trait + 注册机制 + 内置 Section（CoreIdentity / ToolInstructions / WorkspaceContext / ProjectRules）
- [x] 19.17 `src/core/session/manager.rs`：`init_context_state` 增加 compact boundary 处理（遇到 `is_boundary=true` 的 Compaction entry 时丢弃其前所有暂存 entry）— 已在 17.2 整改中实现
- [ ] 19.18 [session-storage.md](../openspec/specs/architecture/session-storage.md)：`CompactionEntry` 增加 `is_boundary: bool` 字段文档

**Phase 6：测试**
- [ ] 19.19 单元测试：`estimated_token_count`（有 usage / 无 usage / compact 后 fallback）、`usage_ratio` 计算、cascade 流程（各 ratio 档位触发正确层级 + m 值）
- [ ] 19.20 单元测试：Layer 0 落盘（单条 >= 30K 默认 / 单 turn 合计 >= 150K 默认 / preview 生成 / 阈值可配）、Layer 1 cascade 触发（20K 阈值 / 不独立执行）
- [ ] 19.21 单元测试：Circuit Breaker（连续 3 次失败跳过 / 成功清零）、PTL 重试（范围减半 / 最多 2 次）
- [ ] 19.22 集成测试：大文件 tool_result → Layer 0 落盘不丢失 + 可按需读回；多轮对话 ratio 升高 → cascade 自动触发 Layer 1+2；Session 重载识别 compact boundary 无重复
- [ ] 19.23 集成测试：buffer 安全网在小窗口模型（64K context）下正确触发；ratio >= 0.98 时阻止新工具调用

**依赖**：TASK-17（context-management，DONE）、TASK-14（Agent Loop，DONE）

**被依赖**：TASK-20（上下文管理现行方案：异步预热 + 延迟应用，见下文）

**协作接口**：
- 消费：`LlmProvider`（Compaction 摘要调用 + `StreamEvent::Usage`）、`SessionManager`（transcript 读写 + tool-result 文件 I/O）、`EventBus`（事件发布）、`PrimitiveConfig`（配置加载）
- 提供：`ContextState`（增强版：token 维度 + ratio）、cascade 流程编排、`ContextMetrics`、`SystemPromptSection` trait

**涉及文件**：

| 文件 | 改动 |
|------|------|
| `src/core/session/manager.rs` | `ContextState` 扩展（token 字段 + usage + ratio + circuit breaker）；compact boundary 处理 |
| `src/core/agent_loop.rs` | 捕获 Usage + 触发 cascade + ratio >= 0.98 阻止工具调用 |
| `src/infra/config.rs` | 新增 `layer0_single_result_max_chars`、`layer0_turn_aggregate_max_chars` 配置项 |
| `src/core/compaction.rs` | Layer 0 落盘 + Layer 1 cascade 改造 + Layer 2 单次调用 + Layer 3 目标 0.50 + cascade 编排 + Circuit Breaker + PTL 重试 |
| `src/core/system_prompt.rs` | 模块化 `SystemPromptSection` trait + 分页读取引导 |
| `src/infra/events.rs` | 新增事件类型 |
| `src/core/context_metrics.rs`（**新建**） | `ContextMetrics` 指标结构体 |

**验收标准**：
- `StreamEvent::Usage` 被正确捕获，`estimated_token_count` 有 usage 时基于真实值、无 usage 时 fallback 字符估算
- ratio 达到各档位时触发对应层级压缩（0.70→L2 m=5 / 0.85→L2 m=3 / 0.92→L2 m=2 / 0.98→L2 m=1 + 阻止工具 / 1.0→L3 目标<0.50）
- cascade 逐层执行、每层后重算 ratio、降压成功即停
- 单条 tool_result >= `layer0_single_result_max_chars`（默认 30K）被 Layer 0 落盘 + preview 占位符，原始内容可通过路径读回；阈值可配
- Layer 1 仅在 cascade 内触发，不独立每轮执行
- Circuit Breaker 连续失败 3 次跳过 Layer 2 降级到 Layer 3
- PTL 重试取较新半段最多 2 次，优先保留近期上下文
- compact boundary 使 session 重载与运行时状态一致、无重复
- `ContextMetrics` 每轮更新并通过 EventBus 推送
- `cargo clippy --all-targets -- -D warnings` 无新增 warning
- 新增单测覆盖率 ≥ 80%

---

### TASK-20 | context-management-async-boundary | 上下文管理 — 异步预热与 Boundary 延迟应用（对齐现行方案）

| 字段 | 内容 |
|------|------|
| **优先级** | P1 |
| **状态** | `PENDING_INTEGRATION` |
| **负责人** | Jerry |
| **分支** | `feature/context-async-compaction` |
| **阻塞点** | — |

**目标**：按 [上下文管理技术方案](../openspec/specs/architecture/context-management.md) **现行** §1–§5，将 LLM 摘要从「同步阻塞 / 每轮级联」收敛为 **Layer 1 异步预热 + Layer 2 延迟应用（Boundary 切换）**，保证 **reasoning loop 最终 assistant 回复后（时机 ⑤）主线程零等待**；仅在 **下一 user turn 发起 LLM 请求前（时机 ②）**、且 `ratio >= 0.98` 而摘要未完成时，才允许阻塞等待推理启动。统一水位线与触发表（0.50 / 0.70 / 0.85 / 0.98、buffer 安全网、Layer 3 仅 API Context Overflow 后兜底）。

**技术方案**：[context-management.md](../openspec/specs/architecture/context-management.md)（§3 时序图、§4.2 水位线、§5.2 `CompactionSummary`、§5.5 compact boundary）

**子项**：

**Phase A：状态与单例任务**
- [ ] 20.1 `ContextState`（或等价模块）：`compaction_summary: Option<CompactionSummary>` — 含后台 `JoinHandle`、覆盖区间 `covered_start_id`/`covered_end_id`、完成结果；**同一时间仅允许一个**预热任务；Session 退出时对未完成 task `abort()`（§5.2）
- [ ] 20.2 Boundary 切换后调用 `invalidate_api_usage`，并重算 `estimate_context_chars` / token 相关字段与 reasoning 消息起始索引（`start_idx`，§5.3、文档图二）

**Phase B：时机 ⑤ / ② 挂载**
- [ ] 20.3 **时机 ⑤**（user turn 结束：最终无 `tool_calls` 的 assistant 回复后）：同步跑 Layer 0（50K 落盘 + preview、compactable zone 内 10K 占位符，§4.4）；重算 `ratio`；`ratio >= 0.50` 且无进行中任务则 **spawn** Layer 1 预热；`ratio >= 0.85` 时对已完成摘要 **非阻塞** Boundary 切换，并可按需启动新一轮预热（§4.2）
- [ ] 20.4 **时机 ②**（下一 user turn 构建 `messages` 并发 LLM 前）：`ratio >= 0.70` 时非阻塞检查并完成则切换；`ratio >= 0.98` 时已完成则切换，**未完成则 `block_on` 等待**后再切换（§4.2）；与 buffer 安全网（§4.3）对齐

**Phase C：Transcript 与 Layer 1/2/3 语义**
- [ ] 20.5 预热完成：追加 `Compaction` entry，`is_boundary=false`（备用，加载时跳过）；应用切换：追加 `is_boundary=true`，`init_context_state` 折叠与运行时一致（§5.5）
- [ ] 20.6 Layer 1：克隆 `user_turns_list` 调 `compaction_model`，摘要覆盖**整表**并记录首尾 id；与既有 UPDATE 摘要 prompt 兼容（§3 图四、文档 1.1）
- [ ] 20.7 Layer 3：**仅**在 API 明确 Context Overflow 后物理截断至 `ratio < 0.50`（§4.2）；与 TASK-19 中「ratio=1.0 触发 L3」等旧叙述脱钩，以现行文档为准

**Phase D：可观测与配置对齐**
- [ ] 20.8 `ContextMetrics`（若尚未完整落地则本任务收口）：压缩次数、释放量、利用率等；经 EventBus 推送；CLI/TUI **状态栏反馈压缩进度**（§1.1 设计目标 8）
- [ ] 20.9 `[context]` 默认值与文档一致：`layer0_single_result_max_chars=50000`、`layer0_placeholder_threshold_chars=10000` 等（§4.4）；`compute_context_budget_chars` 改为 `input_budget * 4`（去掉 `* 0.75`，对齐 §4.6）
- [ ] 20.14 `system_prompt.rs` 新增落盘文件分页读取引导（§6.5 防振荡），告知 LLM 可通过 `read_file` offset/limit 按需读取

**Phase E：清理遗留同步路径**
- [ ] 20.10 **彻底删除**同步 cascade 旧路径：`run_compaction_cascade_v2` / `run_compaction_cascade` / `determine_cascade_params` / `CascadeParams` / `CascadeResult` / `force_drop_oldest` / `run_compaction` / `run_compaction_loop`；**移除** `compaction_consecutive_failures` 字段；L3 唯一入口收敛为 `is_context_overflow_error` + `force_drop_oldest_to_target`（**不保留 deprecated 过渡**）

**Phase F：Session 生命周期管理**
- [ ] 20.11 Session 退出时（Ctrl+D / `/exit` / error）调用 `abort_preheat()` 清理后台预热 task，防止资源泄漏

**Phase G：测试**
- [ ] 20.12 单元测试：单例预热、⑤ 不阻塞、② 下 0.98 同步等待路径、boundary 加载跳过 `false`、task abort
- [ ] 20.13 集成测试：高 `ratio` 下对话与流式输出不卡顿；session 重载后内存视图与 transcript 一致

**依赖**：TASK-17（DONE）、TASK-19（DONE）；以当前 `develop` 实现为基线，**规格以 `context-management.md` 现行正文为准**（TASK-19 表格若与文档不一致时以文档收口）。

**被依赖**：—

**协作接口**：
- 消费：`LlmProvider`（`StreamEvent::Usage`、compaction 调用）、`SessionManager`（transcript、tool-result 落盘路径）、`EventBus`、`PrimitiveConfig`
- 提供：异步 `CompactionSummary` 生命周期 API、Boundary 切换与 `build_context_messages` 一致行为

**验收标准**：
- ⑤ 路径上无 `await` Compaction LLM；UI 在整轮 reasoning 结束后不因压缩卡住
- ② 路径上仅在 `ratio >= 0.98` 且摘要未完成时阻塞，且阻塞发生在用户已看到完整回复之后
- `ratio` 档位与 §4.2 总表一致；Layer 3 仅在 Context Overflow 错误后触发
- Transcript：`is_boundary=false` / `true` 语义与 §5.5 一致；重载无重复摘要、无丢失 boundary 后状态
- `cargo clippy --all-targets -- -D warnings`；新增/相关单测覆盖率 ≥ 80%
