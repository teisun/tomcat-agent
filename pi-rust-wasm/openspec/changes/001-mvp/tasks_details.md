# 一期 MVP 原子任务明细（tasks_details）

## 说明

- 本文档由 [task.md](./task.md) 大任务拆解而来，用于直接执行与跟踪。
- 大任务 ID、优先级、依赖以 task.md 为准；此处仅列原子子任务。
- 子任务按实现顺序排列，单条可独立验收；带「边界/验收」的项覆盖计划校验中识别的边界场景。

---

## T1-P0-001 项目骨架搭建与基础设施层落地

- **1.1** 使用 `cargo new` 初始化 Rust 项目，创建 `infra/`, `core/`, `ext/`, `api/`, `common/` 文件夹及对应的 `mod.rs`，配置 workspace（若有多 crate 规划）。
- **1.2** 在 Cargo.toml 中声明并锁定依赖：thiserror、anyhow、config、tracing、tracing-subscriber、serde、serde_json、跨平台所需 crates（如 dirs、path 等）。
- **1.3** 定义项目统一错误枚举 AppError（参考 design.md CODE_BLOCK_P1_001），实现 From 常见类型，禁止包含 Db(rusqlite)（MVP 不用 SQLite）。
- **1.4** 实现配置结构体（AppConfig、LogConfig、LlmConfig、StorageConfig、PluginConfig、SecurityConfig、PrimitiveConfig），与 design 一致。
- **1.5** 实现配置加载与合并（文件 + 环境变量）、默认配置生成、配置合法性校验入口。
- **1.6** 接入 tracing，实现分级日志（trace/debug/info/warn/error）、控制台与按大小滚动的文件输出。
- **1.7** 实现跨平台基础适配：**重点包含 `write_file_atomic`（临时文件+重命名）与路径规范化**、通用文件读写封装、进程/系统信息接口；用条件编译区分 Windows/macOS/Linux。
- **1.8** 运行 clippy 全量规则并修复警告；为核心模块编写单元测试，覆盖率≥90%。

---

## T1-P0-002 全局事件总线核心实现

- **2.1** 定义 AgentEvent、ExtensionEvent 枚举及 payload 结构，与 Architecture.md「事件系统设计」完全对齐（type snake_case，payload camelCase）。
- **2.2** 定义 EventBus Trait（on/once/off/emit_sync/emit_async/remove_plugin_listeners），扩展侧使用字符串事件名。
- **2.3** 实现 EventBus：监听器注册与 ID 分配、同步 emit 按序执行回调。
- **2.4** 实现异步 emit、事件优先级、EventContext 传递。
- **2.5** 实现 remove_plugin_listeners：插件卸载时按 plugin_id 清理该插件所有监听。
- **2.6** 单回调错误捕获：任一听众抛错时仅记录/返回，不中断其他听众与主流程。
- **2.7** 单元测试：同步/异步、优先级、注销、**边界：单 listener 抛错时其余 listener 仍执行且主流程不崩溃**；覆盖率≥90%。

---

## T1-P0-003 存储层与会话管理模块落地

上述**禁止全量加载、BufReader 逐行、最近 N 条、零拷贝解析**等要求遵循 **Architecture.md「2.1 会话管理」下的 Transcript 的存储与读取约定**。

- **3.1** 定义 SessionStore（sessions.json）、SessionEntry 等元数据结构，与 Architecture.md「会话存储数据结构设计」一致；实现 sessionKey→SessionEntry 的读写与持久化。
- **3.2** 定义 SessionHeader、SessionEntry（JSONL 行类型）及 EntryBase 等，与 pi 系 JSONL 格式兼容；实现单文件 JSONL 读写与追加。
- **3.3** 实现会话 CRUD：创建、按 sessionKey 查询/更新/归档/删除；会话列表与当前会话路由仅依赖 sessions.json，不建 SQLite。**重点实现元数据 sessions.json 的原子性更新算法**，确保并发写入下的数据安全性(不允许锁文件，可以用队列)。
- **3.4** 实现消息管理：appendMessage、appendThinkingLevelChange、appendModelChange、appendCompaction、appendSessionInfo、appendLabelChange 等写入 JSONL；getEntry、getEntries、getBranch、getTree、getChildren、getLeafEntry 等只读查询。**约束**：禁止全量加载 transcript（禁止一次性 `from_str` 整文件）；使用 BufReader 逐行或流式读取；上下文组装仅保留最近 N 条（N 可配置，MVP 可用固定值如 10）。
- **3.5** 实现上下文组装：根据会话历史组装 LLM 所需消息列表，支持会话级上下文窗口配置（同上，仅保留最近 N 条，不全量加载）。
- **3.x** `SessionHeader` 读取必须使用流式解析（如 `serde_json::StreamDeserializer`），确保首行再大也不会撑爆内存。
- **零拷贝**：在生命周期允许的前提下，sessions.json、config.toml、单行 JSONL 解析优先使用 `from_slice` + 借用（`&'a str`），减少分配。
- **3.6** 实现会话级配置隔离：每会话可独立配置 LLM 模型、启用的插件等，并存于 SessionEntry/配置层。
- **3.7** 单元测试：空 store、无 sessions.json、无会话列表时的行为与路径约定；覆盖率≥80%。

---

## T1-P0-004 LLM统一接入模块落地

- **4.1** 定义 LlmProvider Trait（provider_name、chat、chat_stream、count_tokens），与 design CODE_BLOCK_P1_005 一致。
- **4.2** 定义 ChatRequest、ChatResponse、ChatMessage、StreamEvent 等请求/响应类型。
- **4.3** 实现 OpenAI 格式适配器：非流式 chat、流式 chat_stream（基于 tokio-stream）。
- **4.4** 实现请求限流、指数退避重试、并发控制。
- **4.5** 实现 Token 消耗统计与会话级汇总；会话级模型配置隔离（从 SessionEntry 或配置层读取）。
- **4.6** 单元测试：非流式/流式调用、限流与重试、**边界：流式中断/超时时的错误处理与资源释放**；覆盖率≥80%。

---

## T1-P0-005 4原语执行引擎核心实现

- **5.1** 定义 PrimitiveExecutor Trait 及 read_file、list_dir、write_file、edit_file、execute_bash、require_user_confirmation；定义 EditOperation、EditOperationType、PrimitiveOperation 等，与 design CODE_BLOCK_P1_006 一致。
- **5.2** 实现 read_file：路径白名单校验、大文件分块读取、审计日志记录。
- **5.3** 实现 write_file：路径白名单、覆盖前备份、用户确认接口调用、原子写入、审计。
- **5.4** 实现 edit_file：路径白名单、编辑前备份、diff 生成、用户确认接口调用、原子操作与回滚、审计。
- **5.5** 实现 execute_bash：三级命令管控（白名单/审批/禁止）、工作目录白名单、超时与资源限制、用户确认、stdout/stderr 审计。
- **5.6** 定义用户确认交互接口（Trait 或回调），供 CLI 等上层实现具体交互；实现权限与 PrimitiveConfig 的读取（path_whitelist、bash_whitelist 等）。
- **5.7** 单元测试：各原语成功路径、白名单拒绝、**边界：用户拒绝确认时的错误返回与审计记录**；覆盖率≥90%。

---

## T1-P0-006 工具注册中心核心实现

- **6.1** 定义 Tool 结构（name、label、description、parameters JSON Schema、plugin_id、is_enabled、created_at），与 pi-mono ToolDefinition 对齐。
- **6.2** 定义 ToolRegistry Trait（register_tool、unregister_tool、get_tool、list_tools、call_tool、unregister_plugin_tools），与 design CODE_BLOCK_P1_007 一致。
- **6.3** 实现工具注册/注销/按名称检索/按 plugin_id 列表；插件卸载时调用 unregister_plugin_tools。
- **6.4** 实现 call_tool：参数校验、权限校验、执行、审计、返回值封装（content、details 与 AgentToolResult 一致）。
- **6.5** 实现插件级工具访问权限（与 SecurityConfig/PluginConfig 联动）。
- **6.6** 单元测试：注册/注销/调用/权限拒绝/卸载后自动注销；覆盖率≥80%。

---

## T1-P0-007 WasmEdge运行时与QuickJS集成

- **7.1** 集成 WasmEdge 库，实现 `WasmEngine` 全局单例，**开启异步扩展（Async Options）与统计功能（Statistics）**。
- **7.2** 实现单插件独立 Wasm 实例的创建与销毁（Store/Instance），保证实例间隔离。
- **7.3** 启用 WasmEdge 官方 QuickJS 运行时扩展，验证 JS 代码可执行。
- **7.4** 启用并配置 Node.js 兼容层（fs/path/process/console/http 等高频模块）。
- **7.5** 
  - 实现宿主导入绑定骨架：将宿主侧函数注册到 Wasm 实例导入表，并映射到 QuickJS 全局对象（具体 API 实现在 T1-P0-008）；实现 Rust↔JS 类型转换与错误传递的最小通道。
  - 实现统一 Hostcall 路由器骨架，定义 `invoke_host_func` 入口。
- **7.6** 跨平台编译与运行验证（Windows/macOS/Linux 至少各一次）。
- **资源上限**：Wasm 实例与 QuickJS 创建时使用固定默认资源上限（与 Standard 模式一致），具体数值见 **Architecture.md「4.5 资源与内存模式」**；后续由内存模式扩展为按 profile 配置。
- **7.x** 预留 `set_memory_limit`（或等价）接口调用位，MVP 阶段可传固定值，但代码结构需支持从配置层动态传入上限。

---

## T1-P0-008 宿主API层与JS绑定实现 (Hostcall & JS Binding)

### 核心目标

构建宿主与 Wasm 沙箱间的高性能、异步、安全的通信桥梁。通过统一的 Hostcall 路由器分发请求，并完全对齐 `pi-mono` 的 ExtensionAPI 行为规范。

### 任务细分

- **8.1 协议定义与 DTO 设计 (Protocol & Schema)**
  - 基于 design 定义统一的 Hostcall 通信协议（JSON 序列化）。
  - 实现 Rust 侧与 JS 侧对齐的数据传输对象（DTO），强制使用 `#[serde(rename_all = "camelCase")]` 确保与 `pi-mono` 字段一致。
  - 定义 `HostRequest` 与 `HostResponse` 的标准包格式（包含 `module`, `method`, `params`, `call_id`）。
- **8.2 统一 Hostcall 多路复用分发器 (Dispatcher)**
  - 实现 `HostApiDispatcher` 模块，采用“单入口多路复用”模式，减少 Wasm 导入表开销。
  - 路由逻辑设计：支持根据 `module_id` 或字符串快速分发至对应的 Processor（Fs, Llm, Agent 等）。
  - 实现无状态路由器，确保 `Send + Sync`，支持多 Agent 并发调用。
- **8.3 核心 API 逻辑集成 (Core Logic Integration)**
  - **4原语集成**：接入 `PrimitiveExecutor`，确保所有文件/命令操作经过权限校验与用户二次确认。
  - **LLM/工具集成**：接入 `LlmProvider` 与 `ToolRegistry`，支持插件注册自定义工具。
  - **事件与会话**：接入 `EventBus` 实现跨沙箱事件分发；接入 `SessionManager` 实现上下文读写。
  - **审计挂钩**：在分发层统一触发 `AuditLogger`，记录每一个 Hostcall 的来源插件、参数及执行状态。
- **8.4 异步 Hostcall 与并发调度 (Async & Concurrency)**
  - 技术方案见 [异步 Hostcall 与事件循环设计](../../specs/architecture/plugin-system/async-hostcall-event-loop.md)。
  - **8.4.1** `dispatcher.rs`：新增 `AsyncCallStatus` 枚举（`Pending`/`Done(HostResponse)`/`Error(String)`）和 `async_results: Arc<DashMap<String, AsyncCallStatus>>` 字段到 `HostApiDispatcher`。
  - **8.4.2** `dispatcher.rs`：改造 `dispatch()` — 若 `request.call_id` 非空，spawn Tokio 任务到共享 `tokio_handle`，将实际 `dispatch_async` 结果写入 `async_results`，立即返回 `{ok: true, data: {pending: true}, callId: "..."}`。
  - **8.4.3** `dispatcher.rs`：新增 `__async.poll` 路由 — 当 `module == "__async" && method == "poll"` 时，从 `async_results` 查结果并返回 `{ready: true/false, result: ...}`。
  - **8.4.4** `instance_wasmedge.rs`：将 `dispatch()` 中的 `Runtime::new().block_on(...)` 改为使用宿主全局共享的 `tokio::runtime::Handle`，避免每次创建新 Runtime。
  - **8.4.5** `dispatcher.rs`：为异步任务添加超时控制（`tokio::time::timeout`，默认 30 秒，可配置）。
  - **8.4.6** 实例销毁时清理该实例的所有 pending 异步任务（在 `WasmInstance::drop` 中清除 `async_results` 相关条目）。
  - **8.4.7** 优化并发模型：多 Agent 同时调用时，Session 读写通过分片锁或 `Arc<RwLock>` 解决；LLM 并发通过 `Semaphore` 限制。
  - **8.4.8** 单元测试：异步提交→轮询→返回全链路；超时处理；多 callId 并发；边界：submit 后实例销毁时的清理。
- **8.7 JS API 与 pi-mono 对齐 (JS API Alignment)**
  - 技术方案见 [JS API 与 pi-mono 对齐设计](../../specs/architecture/plugin-system/js-api-alignment.md)。
  - **8.7.1** `pi_bridge.js`：新增 `hostCallAsync` 函数（submit/poll 包装，返回 Promise），含 callId 生成、指数退避轮询逻辑。
  - **8.7.2** `pi_bridge.js`：`exec` / `createChatCompletion` 改为调用 `hostCallAsync`，返回 Promise，返回值解包为 pi-mono 格式（`ExecResult` / `CompletionResult`）。
  - **8.7.3** `pi_bridge.js`：修复 `off` / `emit` 重复定义 bug，合并为单一定义。
  - **8.7.4** `pi_bridge.js`：新增 `pi.once(event, handler)` 方法。
  - **8.7.5** 集成测试：JS 插件调用 `await pi.exec("echo hello")`，验证 Promise 正确 resolve 且返回值为 `{stdout, stderr, exitCode}` 格式。
  - **8.7.6** 集成测试：JS 插件调用 `await pi.createChatCompletion({...})`，验证 Promise 正确 resolve。
  - **8.7.7** （P1）`readFile`/`writeFile`/`editFile` 改为返回 Promise（可先 `Promise.resolve` 包装同步结果）。
  - **8.7.8** （P1）新增 `pi.setModel` / `pi.getModel` / `pi.complete` / `pi.unregisterTool`。
- **8.5 内存安全边界与错误透传 (Safety & Error Handling)**
  - **内存校验**：实现 Wasm 线性内存边界检查，防止宿主侧在读取参数或写入结果时发生越界攻击。
  - **所有权管理**：明确 Hostcall 过程中的内存申请与释放责任，防止内存泄漏。
  - **错误映射**：将宿主侧的 `AppError` 精确映射为 JS 侧的异常（Exception），确保插件能捕获并处理权限拒绝、超时等错误。
- **8.6 验收与测试 (QA)**
  - **功能测试**：编写集成测试脚本（JS 侧），验证从插件发起调用到宿主执行并返回结果的全链路。
  - **边界测试**：模拟大并发调用（10+ Agents 同时请求 LLM）验证引擎稳定性。
  - **安全测试**：验证未授权 API 是否被物理拦截，验证非法内存指针是否被正确阻断。
  - **覆盖率**：宿主 API 层单测及集成测试覆盖率 ≥85%。

---

## T1-P0-009 插件生命周期管理模块落地

- **9.1** 定义 PluginManifest、PluginInstance、PluginStatus，与 design CODE_BLOCK_P1_008 一致；实现清单解析与校验（必填字段、required_api_version、required_permissions）。
- **9.2** 实现加载流程：读取清单与 main 入口代码 → 权限校验与用户确认（调用确认接口）→ 创建 Wasm 实例 → 注册授权 API → 注入并执行插件初始化代码 → 注册到插件管理器。
- **9.3** 实现启用/禁用：仅改变状态，控制插件是否响应事件与工具是否可被调用。
- **9.4** 实现卸载：调用 EventBus.remove_plugin_listeners、ToolRegistry.unregister_plugin_tools，销毁 Wasm 实例，释放所有资源。
- **9.5** 单元测试：加载→启用→禁用→卸载；**边界：清单非法、权限不满足、Wasm/QuickJS 初始化失败时错误信息清晰、宿主不崩溃、可恢复**；无内存泄漏（如有条件做简单检测）。

---

## T1-P0-010 CLI工具核心子命令实现

- **10.1** 使用 clap 搭建 CLI 骨架，定义子命令结构：init、doctor、chat、session、plugin、config、audit；无参数时默认等价于 chat。
- **10.2** 实现 `pi-wasm init`：引导 LLM 配置、基础安全策略，生成配置文件。
- **10.3** 实现 `pi-wasm doctor`：检测运行环境、**WasmEdge 与 QuickJS 可用性**、配置文件存在与合法性，输出修复建议（边界：首次运行无配置时的提示）。
- **10.4** 实现 `pi-wasm config`：get/set/edit/export/import 子命令。
- **10.5** 实现 `pi-wasm session`：list/new/switch/delete/archive/search，依赖 SessionManager；**边界：空会话列表、无当前会话时的行为与提示**。
- **10.6** 实现 `pi-wasm plugin`：list/load/unload/enable/disable/info，依赖插件生命周期管理。
- **10.7** 实现 `pi-wasm audit`：list/show/export（P0 阶段可先读已有审计日志或占位；完整能力依赖 T1-P1-001）。
- **10.8** 完善帮助文档与参数校验，所有子命令可正常执行。

---

## T1-P0-011 CLI对话模式核心实现

- **11.1** 实现对话主循环：读取用户输入、调用 LLM、输出响应；集成 SessionManager 与 LlmProvider。
- **11.2** 实现流式响应渲染（crossterm/bat 等），逐字或逐块输出。
- **11.3** 实现 Markdown 与代码块高亮（bat/similar 等）。
- **11.4** 实现多轮对话上下文：从当前会话加载历史、组装消息列表、写入新消息到 JSONL。
- **11.5** 集成 4 原语与工具调用：在 LLM 返回 tool_calls 时展示并调用 require_user_confirmation/工具执行，结果回传 LLM；**边界：用户拒绝 4 原语确认时的提示与审计**。
- **11.6** 实现快捷键：Ctrl+C 中断生成、Ctrl+D 退出、↑↓ 历史消息导航；会话切换与 `--resume` 行为对齐 pi-mono。
- **11.7** **边界/验收**：会话切换后会话级 LLM/插件配置正确隔离；可选：切换时若有进行中 tool call 的简单策略（等待或取消）。

---

## T1-P1-001 审计日志系统完整落地

- **1.1** 实现独立审计日志模块：专用存储（文件或按设计约定），仅追加、不可篡改；保留最近 N 天（如 90 天）配置。
- **1.2** 在 4 原语、工具调用、插件生命周期、高危操作等关键路径写入审计记录（操作人、时间、内容、用户确认状态、结果、必要输入输出）。
- **1.3** 实现审计日志查询（按时间/类型/插件等）、导出、按策略清理。
- **1.4** 实现 `pi-wasm audit list/show/export` 子命令，与审计模块对接。
- **1.5** （可选）文档说明加密存储为 TODO，当前明文或占位。

---

## T1-P1-002 pi-mono插件兼容性测试与适配

- **2.1** 挑选主流 pi-mono 社区插件（至少 3～5 个），列出依赖的 API 与 Node 模块。
- **2.2** 在本运行时上执行兼容性测试，记录无法加载或运行错误的插件及原因（API 缺失、Node 行为差异、事件名/参数不一致等）。
- **2.3** 修复宿主 API、Node 兼容层、事件 payload 等兼容问题，直至标准插件可零修改运行。
- **2.4** 将兼容性用例固化为自动化测试或文档用例集。

---

## T1-P1-003 核心模块单元测试全覆盖

- **3.1** 对基础设施层、宿主核心能力层、宿主API层、WasmEdge 层、CLI 层各模块补充单元测试，使核心模块覆盖率≥80%、核心路径 100%。
- **3.2** 确保所有测试用例通过；运行跨平台编译与测试（至少 CI 或本地三平台各跑一次）。

---

## T1-P1-004 全平台兼容性测试与bug修复

- **4.1** 在 Windows、macOS、Linux 上执行全量功能测试（init/doctor/chat/session/plugin/config/audit、对话与 4 原语/工具调用）。
- **4.2** 修复平台专属 bug（路径、换行、依赖库等）。
- **4.3** 验证跨平台安装包构建（若本期包含）；优化环境依赖检测与 doctor 的自动适配建议。

---

## T1-P2-001 CLI交互体验优化

- **1.1** 优化流式渲染流畅度（节律、刷新率等）。
- **1.2** 优化 diff 预览与用户确认交互（布局、可读性、确认/取消流程）。
- **1.3** 新增子命令或参数的自动补全（如 shell completion）。
- **1.4** 统一并优化错误提示文案，给出可操作的修复建议。
- **1.5** 为耗时操作新增加载状态与进度提示。

---

## T1-P2-002 插件安全扫描基础能力

- **2.1** 在插件加载前增加安全扫描步骤：静态检查恶意模式、越权 API 使用、敏感信息泄露风险等（规则可配置）。
- **2.2** 对风险插件拦截并提示用户，不静默加载；可选：提供“强制加载”的明确二次确认。

---

## T1-P3-001 项目文档编写

- **1.1** 编写项目 README.md：简介、快速开始、构建与运行、目录结构。
- **1.2** 编写用户使用文档：安装、配置、init/doctor/chat/session/plugin/config/audit 使用说明。
- **1.3** 编写插件开发文档：清单格式、宿主 API、事件、工具注册、4 原语使用示例。
- **1.4** 编写 API 文档（或指向 design/Architecture 中 Trait 与结构说明）。
- **1.5** 编写部署与安装指南：各平台依赖、安装包使用、环境变量与配置路径。

---

## T1-P3-002 示例插件开发

- **2.1** 开发至少 3 个示例插件，分别覆盖：工具注册与调用、事件监听（如 tool_call/input）、4 原语调用（read/write/edit/bash 至少各一例）。
- **2.2** 为示例插件补充注释与 README，作为兼容性测试与开发者参考用例。

---

## T1-P1-005 Agent Loop 核心结构化实现

- **5.1** 定义 AgentMessage 枚举：UserMessage、AssistantMessage、ToolResultMessage、SystemMessage、SteeringMessage；实现 convert_to_llm_format()（AgentMessage → LLM Message 的唯一转换边界，参考 agent-loop.md 13.4）。
- **5.2** 新增 src/core/agent_loop.rs，实现 AgentLoop 结构体；持有 steering_queue/follow_up_queue/abort_signal；实现三层循环骨架（Conversation Loop → Attempt Loop → Reasoning Loop），参考 agent-loop.md 13.3.2 伪代码。
- **5.3** 实现 Steering 机制：steer(msg) 写入 steering_queue；Reasoning Loop 每个工具执行完毕后检查队列，有则跳过剩余工具、注入消息、进入下一轮 LLM 调用。
- **5.4** 实现 FollowUp 机制：follow_up(msg) 写入 follow_up_queue；Conversation Loop 尾部检查队列，有则继续循环（one-at-a-time 默认模式）。
- **5.5** 实现 Abort 信号：abort() 设置 AtomicBool；Reasoning Loop 每个工具执行前检查，已设置则终止循环并发布 agent_end(interrupted)。
- **5.6** 在 Loop 各关键节点接入 EventBus 发布 AgentEvent，发布时序见 agent-loop.md 13.6（agent_start/end、turn_start/end、message_start/update/end、tool_execution_start/end、auto_retry_start/end）。
- **5.7** 实现错误分类（参考 agent-loop.md 13.10）：AppError 中识别 RateLimit/Timeout/5xx 为 Retryable，在 Attempt Loop 内指数退避重试（MAX_ATTEMPTS 可配置，默认 3）；401/400/ModelNotFound 为 Fatal；工具执行错误为 ToolError，回注 LLM。
- **5.8** 重构 src/api/chat.rs：移除直接的 chat_loop + do_chat_turn 实现，改为构造 AgentLoop 并调用 run()；Steering 注册到 CLI Ctrl+C 中断事件。
- **5.9** 单元测试：Loop 状态机（正常/RateLimit 重试/Fatal 终止/Abort 路径）、Steering 注入时序（当前工具完成后才跳过）、FollowUp 触发、AgentEvent 发布顺序；覆盖率 ≥ 80%。

---

## T1-P1-006 长生命周期 VM 实现（VM actor + session 维度 + waitForEvent）

技术方案：[Phase 2 长生命周期 VM 方案设计](../../specs/architecture/plugin-system/phase2-long-lived-vm.md)、[异步 Hostcall 与事件循环设计 11.7](../../specs/architecture/plugin-system/async-hostcall-event-loop.md)。

**第一步：结构改造（低风险先行）**

- **15.1** 将 `instance_wasmedge.rs` 中「每次执行新建 VM」改为「长寿命运行单元」：VM 在会话期间持有，`_start` 与事件分发解耦。
- **15.2** 定义 `WasmInstanceRuntimeKey`（`session_id + plugin_id`），实现 RuntimeManager（lookup / lazy_init / remove）。
- **15.3** 将 `PluginManager` 中 `plugin_id` 维度的实例管理升级为 `session_id + plugin_id` 双键维度。

**第二步：事件驱动（actor 化）**

- **15.4** 引入 VM actor 命令通道（`VmCommand::Init / DispatchEvent / Shutdown`），VM 封装在专属 `spawn_blocking` 线程。
- **15.5** 宿主侧 `dispatcher.rs` 新增 `__session.waitForEvent` 路由，通过有界 channel 向 VM 投递事件。
- **15.6** 实现 `_start` 常驻事件循环：lazy start（首次 `session_start` 时）、`blocking_recv()` 空闲挂起、收到 `Shutdown` 后退出。
- **15.7** 废弃 `dispatch_event` 中的「组合脚本 + `__pi_dispatch_event`」模式，改为 channel send。

**可靠性与收尾**

- **15.8** 实现有界 channel 队列上限与回压；事件处理超时策略；`session_end` 触发 `Shutdown` 并清理 pending。
- **15.9** 单元测试 + 集成测试：全局变量跨事件保持、handler 持续有效、`setInterval` 会话期间运行、多会话隔离、关闭无悬挂。

**验收标准**（来自 phase2-long-lived-vm.md）：插件全局变量可跨事件保持；已注册 handler 在多次事件中持续有效；`setInterval` 在会话期间稳定运行；多会话上下文隔离（状态不串会话）；关闭流程无悬挂线程、无 pending 泄漏。

