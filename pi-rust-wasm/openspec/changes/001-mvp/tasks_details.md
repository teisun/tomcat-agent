# 一期 MVP 原子任务明细（tasks_details）

## 说明

- 本文档由 [task.md](./task.md) 大任务拆解而来，用于直接执行与跟踪。
- 大任务 ID、优先级、依赖以 task.md 为准；此处仅列原子子任务。
- 子任务按实现顺序排列，单条可独立验收；带「边界/验收」的项覆盖计划校验中识别的边界场景。

---

## T1-P0-001 项目骨架搭建与基础设施层落地

- **1.1** 使用 `cargo new` 初始化 Rust 项目，配置 workspace（若有多 crate 规划）。
- **1.2** 在 Cargo.toml 中声明并锁定依赖：thiserror、anyhow、config、tracing、tracing-subscriber、serde、serde_json、跨平台所需 crates（如 dirs、path 等）。
- **1.3** 定义项目统一错误枚举 AppError（参考 design.md CODE_BLOCK_P1_001），实现 From 常见类型，禁止包含 Db(rusqlite)（MVP 不用 SQLite）。
- **1.4** 实现配置结构体（AppConfig、LogConfig、LlmConfig、StorageConfig、PluginConfig、SecurityConfig、PrimitiveConfig），与 design 一致。
- **1.5** 实现配置加载与合并（文件 + 环境变量）、默认配置生成、配置合法性校验入口。
- **1.6** 接入 tracing，实现分级日志（trace/debug/info/warn/error）、控制台与按大小滚动的文件输出。
- **1.7** 实现跨平台基础适配：路径规范化、通用文件读写封装、进程/系统信息接口；用条件编译区分 Windows/macOS/Linux。
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

- **3.1** 定义 SessionStore（sessions.json）、SessionEntry 等元数据结构，与 Architecture.md「会话存储数据结构设计」一致；实现 sessionKey→SessionEntry 的读写与持久化。
- **3.2** 定义 SessionHeader、SessionEntry（JSONL 行类型）及 EntryBase 等，与 pi 系 JSONL 格式兼容；实现单文件 JSONL 读写与追加。
- **3.3** 实现会话 CRUD：创建、按 sessionKey 查询/更新/归档/删除；会话列表与当前会话路由仅依赖 sessions.json，不建 SQLite。
- **3.4** 实现消息管理：appendMessage、appendThinkingLevelChange、appendModelChange、appendCompaction、appendSessionInfo、appendLabelChange 等写入 JSONL；getEntry、getEntries、getBranch、getTree、getChildren、getLeafEntry 等只读查询。
- **3.5** 实现上下文组装：根据会话历史组装 LLM 所需消息列表，支持会话级上下文窗口配置。
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

- **7.1** 集成 WasmEdge 库，实现全局 Engine 单例的初始化与生命周期管理（仅一次初始化）。
- **7.2** 实现单插件独立 Wasm 实例的创建与销毁（Store/Instance），保证实例间隔离。
- **7.3** 启用 WasmEdge 官方 QuickJS 运行时扩展，验证 JS 代码可执行。
- **7.4** 启用并配置 Node.js 兼容层（fs/path/process/console/http 等高频模块）。
- **7.5** 实现宿主导入绑定骨架：将宿主侧函数注册到 Wasm 实例导入表，并映射到 QuickJS 全局对象（具体 API 实现在 T1-P0-008）；实现 Rust↔JS 类型转换与错误传递的最小通道。
- **7.6** 跨平台编译与运行验证（Windows/macOS/Linux 至少各一次）。

---

## T1-P0-008 宿主API层与JS绑定实现

- **8.1** 按 design「核心API分类与对齐规范」列出全部宿主 API：4原语、LLM、工具、事件、会话、配置、日志。
- **8.2** 在 Rust 侧实现各 API 的核心逻辑，接入权限校验与审计日志（调用现有 PrimitiveExecutor、ToolRegistry、EventBus、SessionManager、LlmProvider 等）。
- **8.3** 将上述 API 注册到 WasmEdge 导入表，并在 QuickJS 中暴露为全局 agent 对象（或约定命名空间）。
- **8.4** 实现 Rust 与 JS 类型双向转换（参数与返回值）、异步调用在宿主侧的调度与回传。
- **8.5** 实现插件调用 API 时的错误捕获与透传到 JS 侧。
- **8.6** 单元测试：至少覆盖主要 API 的调用链与错误路径；覆盖率≥80%。

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
- **10.2** 实现 `pi-awsm init`：引导 LLM 配置、基础安全策略，生成配置文件。
- **10.3** 实现 `pi-awsm doctor`：检测运行环境、**WasmEdge 与 QuickJS 可用性**、配置文件存在与合法性，输出修复建议（边界：首次运行无配置时的提示）。
- **10.4** 实现 `pi-awsm config`：get/set/edit/export/import 子命令。
- **10.5** 实现 `pi-awsm session`：list/new/switch/delete/archive/search，依赖 SessionManager；**边界：空会话列表、无当前会话时的行为与提示**。
- **10.6** 实现 `pi-awsm plugin`：list/load/unload/enable/disable/info，依赖插件生命周期管理。
- **10.7** 实现 `pi-awsm audit`：list/show/export（P0 阶段可先读已有审计日志或占位；完整能力依赖 T1-P1-001）。
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
- **1.4** 实现 `pi-awsm audit list/show/export` 子命令，与审计模块对接。
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
