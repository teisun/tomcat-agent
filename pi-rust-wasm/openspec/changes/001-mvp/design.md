# 设计文档：一期 MVP 核心引擎与插件系统落地

## 参考与原则

### 参考文件

- **pi 生态兼容性对齐检查**：（原 `archive/pi-ecosystem-alignment-check.md` 已归档移除）用于核对事件、API、工具定义与 pi_agent_rust / pi-mono 的差异及对齐结论；设计或实现变更影响扩展/事件/宿主 API 时，应据 Architecture.md 和 host-call-protocol.md 做一次对齐检查。

### pi 生态参考原则（与 Architecture.md 一致）

所有影响「兼容 pi 生态」的技术设计，**必须同时参考 pi-mono 与 pi-agent-rust 两个仓库**：

- **pi-mono**：**兼容性契约与行为基准**。事件名、API 形态、payload 结构、协议语义等以 pi-mono 为权威；「与 pi 生态兼容」的最终标准是与 pi-mono 的对外行为与接口一致。
- **pi-agent-rust**：**Rust 侧的主要实现参考**。事件拆分、hostcall、扩展加载与 QuickJS 集成、会话/工具/权限等实现可优先参考 pi-agent-rust；其已与 pi-mono 对齐的部分可直接沿用。
- **二者不一致时**：以 **pi-mono 的语义为准**，在 pi-rust-wasm 中按 pi-mono 实现；不把 pi-agent-rust 的当前行为当作最终标准（pi-agent-rust 的 drop-in 认证当前为 NOT_CERTIFIED）。

---

## 整体架构设计

本期落地项目最小可行分层架构，以 pi-mono 为兼容性契约、pi-agent-rust 为 Rust 实现参考，严格遵循安全隔离优先、单向依赖、无循环依赖的设计原则，为后续迭代预留完整扩展能力。

架构层级从下到上依次为：
基础设施层 → 宿主核心能力层 → 宿主API层 → WasmEdge运行时层 → 沙箱执行层 → CLI交互层

## 核心模块详细设计
### 0. 开发与代码组织规范
- **分层目录结构**：
  - `src/infra/`: 错误处理、配置、日志、事件总线、平台适配。
  - `src/core/`: 会话管理、LLM 适配、4原语逻辑、工具注册、权限管控。
  - `src/ext/`: WasmEdge 运行时、QuickJS 绑定、Node.js 兼容层。
  - `src/api/`: CLI 子命令实现、交互逻辑。
- **可见性原则**：内部逻辑优先使用 `pub(crate)`，仅通过 `mod.rs` 重新导出（Re-export）必要 API。
- **异步优先**：核心链路（LLM、IO、Hostcall）必须支持 `async/await`，基于 `tokio` 调度。

### 1. 基础设施层
#### 1.1 统一错误处理体系
设计思路：基于thiserror定义项目统一错误枚举，基于anyhow做上层错误包装，所有错误必须包含清晰的上下文信息，禁止裸panic、禁止滥用unwrap()/expect()，所有错误可被完整捕获，不传递到主程序。

核心错误枚举见 [CODE_BLOCK_P1_001]

#### 1.2 配置管理模块
设计思路：基于config-rs实现多源配置合并，支持配置文件、环境变量，支持配置热更新，启动时自动校验配置合法性，默认配置遵循最小权限原则。

核心结构见 [CODE_BLOCK_P1_002]

AppConfig 预留 **memory 相关扩展**（如 `memory_profile`、`MemorySettings`），具体结构与默认值见 **Architecture.md「4.5 资源与内存模式」**；一期 MVP 不实现内存模式代码，仅预留设计。

核心能力：配置加载与合并、热更新、配置合法性校验、配置文件生成与修复。

#### 1.3 日志与审计系统
设计思路：基于tracing实现分级日志，分为trace/debug/info/warn/error五个级别，支持控制台/文件双输出，按大小滚动归档；独立审计日志模块，专门记录4原语调用、工具调用、插件生命周期、高危操作，不可篡改、可追溯。（审计日志加密存储 TODO 后续考虑）

核心配置：生产环境默认关闭debug/trace级别日志，所有日志禁止打印敏感信息；审计日志单独存储，保留最近90天记录，支持导出与查询。

#### 1.4 全局事件总线
设计思路：参考pi-agent-rust与pi-mono的事件机制，基于发布-订阅模式实现全局同步/异步事件总线，是宿主与插件、插件与插件通信的唯一方式，替代原钩子设计，完全对齐pi-mono的事件API规范。

核心特性：
- 支持同步/异步两种事件回调模式，同步回调可阻塞事件流程，异步回调不阻塞
- 事件按注册顺序执行，单个回调错误完整捕获，不影响其他回调
- 支持事件优先级，系统级事件优先级高于插件自定义事件
- 支持事件上下文传递，回调函数可获取完整的事件触发上下文
- 支持事件监听的自动注销，插件卸载时自动清理该插件注册的所有监听
- 完全对齐 pi-mono / pi-agent-rust 的事件 API：**扩展侧使用字符串事件名**（如 `"tool_call"`、`"session_before_switch"`、`"input"`），snake_case，与 pi 生态一致

事件分类与枚举（AgentEvent 流式/UI、ExtensionEvent 扩展钩子）、payload 约定（camelCase）见 **Architecture.md「事件系统设计」**，此处不重复列出。

核心事件总线 Trait 定义见 [CODE_BLOCK_P1_004]

#### 1.5 跨平台基础适配
设计思路：封装通用文件操作、进程管理、系统信息获取、路径处理接口，基于Rust条件编译抹平Windows/macOS/Linux平台差异，核心业务逻辑100%跨平台复用，仅做平台专属的适配与裁剪。

### 2. 宿主核心能力层
#### 2.1 会话管理模块

设计思路：负责会话全生命周期管理、对话持久化、上下文组装与会话级配置隔离；**设计面向多 Agent、多 channel**（参考 openclaw），**兼容并复用 pi 系 transcript 格式**；多 Agent / 多 channel 实现放三期，MVP 仅落地单 Agent、单入口。**会话内容不使用 SQLite**，仅使用 pi 系 JSONL；索引与路由由 **sessions.json** 提供。

- 两层：元数据 store（sessions.json，`sessionKey -> SessionEntry`）+ 对话 transcript（pi 系 JSONL，与 pi-mono 一致）。
- 核心能力：
    - 会话CRUD：创建、查询、更新、归档、删除、搜索
    - 消息管理：有限支持对话记录的增删改查（通过SessionManager写入 JSONL 不落 SQLite）
    - 上下文组装：根据会话历史自动组装LLM所需的上下文消息，支持会话级上下文窗口配置
    - 会话级配置隔离：每个会话可独立配置使用的LLM模型、启用的插件
    - 会话关联追溯：支持会话来源记录，上下文完整追溯，会话列表与「当前会话」由 sessions.json 提供。
- **一致性保障**：
  - **Append-only**：对话 Transcript (JSONL) 仅允许追加，禁止修改历史行，确保审计线索完整。
  - **Atomic Store**：`sessions.json` 的更新必须采用“写临时文件 -> Rename”模式，确保在断电或崩溃时元数据不损坏。

> 会话路径、sessionKey/sessionId、SessionEntry 字段及 transcript 格式、其他细节见本文末尾 **会话管理数据结构设计**。

#### 2.2 LLM接入模块
设计思路：采用适配器模式，定义统一的LLM Provider Trait，不同大模型实现对应的适配器，主程序仅与统一Trait交互，与具体模型解耦，完全对齐pi-mono的LLM调用API规范，支持流式与非流式调用。

统一Trait定义见 [CODE_BLOCK_P1_005]

核心能力：
- 多模型统一接入，兼容所有OpenAI API格式的大模型
- 流式响应支持，基于tokio-stream实现异步流式输出，完全对齐pi-mono的流式API
- 请求限流与指数退避重试，并发控制
- Token消耗统计与记录，会话级Token消耗汇总
- 模型配置热更新，无需重启程序
- 会话级模型配置隔离，不同会话可使用不同模型与参数

#### 2.3 4原语执行引擎
设计思路：宿主可信核心，完全对齐pi-mono的4原语API规范，是插件访问系统资源的唯一通道，所有操作必须经过「权限校验→用户确认→执行→审计日志记录」全流程，保证安全可控。

核心结构与API定义见 [CODE_BLOCK_P1_006]

核心能力与安全机制：
1.  **Read原语**
    - 核心能力：读取文件内容、列出目录结构、获取文件元数据
    - 安全机制：路径白名单校验，禁止访问未授权目录；大文件分块读取，避免内存溢出；完整审计日志记录
2.  **Write原语**
    - 核心能力：创建文件、写入内容、创建目录、删除路径
    - 安全机制：路径白名单校验；覆盖/删除操作前自动备份原文件，支持回滚；操作前显示内容预览，用户二次确认；原子化写入，避免文件损坏；完整审计日志
3.  **Edit原语**
    - 核心能力：基于行号的替换/插入/删除、基于内容匹配的精确替换
    - 安全机制：路径白名单校验；编辑前自动备份原文件；编辑前生成diff预览，用户二次确认；原子化操作，失败自动回滚；完整审计日志
4.  **Bash原语**
    - 核心能力：shell命令执行，实时流式输出，工作目录与环境变量配置，超时控制
    - 安全机制：三级命令管控（白名单/审批/禁止）；执行前显示命令内容与风险提示，用户二次确认；严格的资源限制（CPU/内存/超时）；工作目录限制在白名单内；禁止sudo/root权限执行；完整审计日志，包含stdout/stderr全量输出

#### 2.4 工具注册中心
设计思路：参考pi-agent-rust设计，实现全局工具注册与管理，宿主内置工具与插件注册的自定义工具统一管理，支持LLM函数调用规范，是插件扩展Agent能力的核心入口。

核心结构与Trait定义见 [CODE_BLOCK_P1_007]

核心能力：
- 工具注册/注销：插件可通过宿主API注册/注销自定义工具，插件卸载时自动注销
- 工具检索：支持按名称、标签、描述检索工具，LLM调用时自动匹配
- 工具调用：统一的工具调用入口，经过权限校验、参数校验、审计日志记录，执行结果统一封装
- 函数调用规范：完全对齐OpenAI函数调用规范，支持JSON Schema输入定义，LLM可自动解析调用
- 工具权限管控：插件级工具访问权限配置，未授权插件无法调用受限工具

#### 2.5 插件生命周期管理模块
设计思路：负责插件的全生命周期管理，基于WasmEdge实现每个插件的独立沙箱实例，完全隔离，生命周期与宿主解耦，执行完成后完全释放资源。

核心流程：
1.  **插件加载**：读取插件代码与清单文件，校验插件合法性、权限声明，创建独立的WasmEdge实例
2.  **实例初始化**：初始化WasmEdge实例，内置QuickJS运行时与Node.js兼容层，注册该插件授权的宿主API到导入表
3.  **插件初始化**：注入插件代码到QuickJS运行时，执行插件初始化逻辑，注册工具、监听事件
4.  **启用/禁用**：控制插件是否可响应事件、工具是否可被调用，禁用状态下插件代码不执行
5.  **卸载**：销毁WasmEdge实例，注销插件注册的工具与事件监听，完全释放所有内存与资源，无残留

核心结构见 [CODE_BLOCK_P1_008]

### 3. 宿主API层
设计思路：完全对齐pi-mono ExtensionAPI规范，是宿主向插件开放的唯一可信接口，所有API通过WasmEdge导入表显式注册，未授权API插件无法访问。

核心API分类与对齐规范：
| API分类 | 核心接口 | pi-mono兼容性 | 说明 |
|---------|----------|---------------|------|
| 4原语API | 内置工具 read/write/edit/bash（与 pi-mono 一致） | 100%对齐 | 以工具形式暴露给 LLM；插件通过宿主工具调用链使用，与 pi_agent_rust tool.read/write/edit/bash 语义一致 |
| LLM API | createChatCompletion/createChatCompletionStream | 100%对齐 | 插件调用大模型的统一接口 |
| 工具API | registerTool/unregisterTool/getToolList/callTool | 100%对齐 | 插件注册/调用工具的核心接口 |
| 事件API | on/once/off/emit | 100%对齐 | 事件监听与发布，宿主与插件通信 |
| 会话API | getCurrentSession/getMessages/sendMessage/updateSessionConfig | 100%对齐 | 插件获取/操作当前会话 |
| 配置API | getPluginConfig/setPluginConfig/getGlobalConfig | 100%对齐 | 插件配置管理，隔离存储 |
| 日志API | log/info/warn/error/debug | 100%对齐 | 插件日志输出，统一接入宿主日志系统 |

API绑定实现逻辑：
1.  宿主层实现API的核心可信逻辑，经过权限校验、审计日志记录
2.  创建插件Wasm实例时，将授权的API函数注册到WasmEdge导入表
3.  WasmEdge将宿主函数映射为QuickJS运行时的全局`agent`对象，插件可直接访问
4.  插件调用API时，通过WasmEdge通道将调用请求与参数转发到宿主层
5.  宿主层执行核心逻辑，将结果按原路返回给插件，转换为JS可识别的类型

统一hostcall通信协议
**设计请参考[架构文档](../../specs/Architecture.md)中的3.3章节**

### 4. WasmEdge运行时层

#### 4.1 设计思路：
- 设计请参考[架构文档](../../specs/Architecture.md)中的 4. WasmEdge运行时层 章节
- 基于WasmEdge官方原生JS运行时扩展，全局单例Engine，每个插件对应独立的Wasm实例，内置优化版QuickJS引擎与Node.js兼容层，实现pi-mono插件的沙箱隔离执行，无需手动打包嵌入QuickJS引擎。

#### 4.2 核心组件：
1.  **全局WasmEdge Engine**：全局唯一初始化，负责Wasm模块编译、内存管理、实例调度，配置全局WasmEdge参数，开启WASI Preview2、WASI-Socket支持，仅一次初始化开销
2.  **插件独立Wasm实例**：每个插件对应一个独立的Store/Instance，专属线性内存、调用栈、QuickJS上下文，完全隔离，插件间无法互相访问，故障不扩散
3.  **QuickJS运行时**：WasmEdge官方优化版，内置到Wasm实例中，原生支持ES6+语法、JS/TS代码执行，无需手动编译嵌入
4.  **Node.js兼容层**：WasmEdge官方原生实现，覆盖Node.js核心模块与全局对象，API行为与Node.js完全对齐，支持CommonJS模块规范
5.  **宿主导入绑定层**：将宿主API显式注册到Wasm实例导入表，映射为QuickJS全局对象，处理Rust与JS类型转换、错误捕获、异步调用调度
6.  **WASI系统接口**：基于WASI Preview2实现，文件IO、网络请求、异步调度全部经过宿主权限校验，禁止未授权访问
#### 4.3 并发调度与异步 Hostcall
- **多路复用分发**：宿主侧实现统一入口路由器，通过 `(module, method)` 映射业务逻辑，减少 Wasm 导出表维护成本。
- **异步 Hostcall 机制（MVP）**：WasmEdge 的 `async_host_function` API 仅限 Linux，不满足跨平台要求。MVP 采用 **submit/poll 模式**：插件发起带 `callId` 的异步请求，宿主 spawn Tokio 任务后立即返回 `{pending: true}`，JS 侧通过 `setTimeout` 驱动的轮询循环调用 `__async.poll` 获取结果。wasmedge_quickjs 内置的 `EventLoop` + `run_loop_without_io()` 自动驱动 Promise 微任务和 setTimeout 回调，`_start` 在所有异步任务完成后自然退出。完整技术设计见 [异步 Hostcall 与事件循环设计](../../specs/architecture/plugin-system/async-hostcall-event-loop.md)。
- **JS API 对齐**：`pi_bridge.js` 中 `exec`/`createChatCompletion` 等耗时 API 改为返回 Promise，与 pi-mono `async/await` 编程模型兼容。详见 [JS API 与 pi-mono 对齐设计](../../specs/architecture/plugin-system/js-api-alignment.md)。
- **资源配额**：每个实例强制限制内存上限（如 128MB）与指令计数（Gas Limit），防止恶意插件耗尽系统资源。

核心执行流程见 [CODE_BLOCK_P1_009]

### 5. CLI交互层
设计思路：基于clap+tokio实现的极简命令行工具，优先保证核心功能可用，交互逻辑对齐pi-mono CLI，所有核心能力优先通过CLI落地。

技术选型：
- clap：命令行参数解析与子命令管理
- tokio：异步运行时，与宿主核心复用同一运行时
- crossterm：终端交互、快捷键支持、流式输出渲染
- bat：Markdown/代码块高亮渲染
- similar：diff预览生成
- dialoguer：用户确认、选择、输入交互

核心子命令设计：
1.  `pi-wasm init`：初始化配置，引导用户完成LLM配置、基础安全策略设置，生成配置文件
2.  `pi-wasm doctor`：检测运行环境、WasmEdge依赖、配置合法性，给出修复建议
3.  `pi-wasm chat`：启动对话模式，支持自然语言对话、流式响应、工具/4原语调用、会话管理
4.  `pi-wasm session`：会话管理子命令，包含list/new/switch/delete/archive/search子命令
5.  `pi-wasm plugin`：插件管理子命令，包含list/load/unload/enable/disable/info子命令
6.  `pi-wasm config`：配置管理子命令，包含get/set/edit/export/import子命令
7.  `pi-wasm audit`：审计日志查看子命令，包含list/show/export子命令

核心交互设计：
- 对话模式：流式逐字渲染，Markdown/代码高亮，4原语/工具调用实时展示，用户确认弹窗，快捷键支持（Ctrl+C中断生成、Ctrl+D退出、↑↓历史消息导航）
- 插件加载：加载插件时显示插件信息、权限声明，等待用户确认授权后加载
- 高危操作：所有4原语写入/编辑/bash操作，默认显示预览与风险提示，用户确认后执行
- 错误提示：所有错误信息友好可读，给出明确的修复建议，无晦涩的技术报错
- 状态反馈：所有操作有清晰的加载状态、成功/失败提示，无静默操作

**CLI 与 pi-mono / openclaw 对照**：子命令划分参考 openclaw（init/doctor/chat/session/plugin/config/audit），便于实现与脚本化；**交互行为**与 pi-mono 对齐（流式输出、会话恢复、4 原语确认、快捷键等）。无参数时 `pi-wasm` 默认等价于 `pi-wasm chat`，与 pi 的「直接进对话」一致。会话恢复通过 `pi-wasm chat --resume` 或 `session` 子命令实现，行为对齐 pi-mono 的 `--resume` / `--session`。

### 6. 安全设计
1.  **沙箱隔离**：每个插件运行在独立的WasmEdge实例中，内存、上下文、执行环境完全隔离，插件无法直接访问宿主系统内存与资源
2.  **最小权限原则**：插件默认仅拥有最小权限，仅开放插件清单中声明、用户确认授权的API，未授权API完全无法访问
3.  **唯一通道原则**：插件仅能通过显式注册的宿主API与宿主系统交互，禁止任何绕过API的直接系统调用，WASI接口全部经过权限校验
4.  **用户知情权保障**：所有4原语写入/编辑/bash高危操作，必须清晰展示操作内容、diff预览、风险提示，获得用户明确二次确认后方可执行，禁止任何形式的静默执行
5.  **错误完全隔离**：插件执行、事件回调、API调用的所有错误，全部独立捕获，不传递到宿主主程序，单个插件崩溃不会影响宿主与其他插件运行
6.  **全链路审计**：所有4原语调用、工具调用、插件生命周期、高危操作，全部留存完整审计日志，包含操作人、时间、内容、用户确认状态、执行结果、全量输入输出，可追溯、可审计
7.  **代码安全校验**：插件加载前自动进行安全扫描，检测恶意代码、越权操作、敏感信息泄露风险，风险插件禁止加载

---

## 资源伸缩性设计（引用）

内存模式（MemoryProfile）、参数表、运行时动态切换及零拷贝/流式、mimalloc 等**完整设计**见 **Architecture.md「4.5 资源与内存模式」**（及该节下子节）。一期 MVP 仅落文档与任务约束，不实现 MemoryProfile 等代码；会话与 transcript 实现须满足 Architecture 中「Transcript 的存储与读取约定」与 4.5 的约定。

---

## 一期代码块
### [CODE_BLOCK_P1_001] 核心错误枚举
```rust
// MVP 会话与审计均不使用 SQLite，故不包含 Db 变体；若后续引入再扩展。
#[derive(Debug, Error)]
pub enum AppError {
    #[error("IO错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("LLM调用错误: {0}")]
    Llm(String),
    #[error("插件错误: {0}")]
    Plugin(String),
    #[error("4原语执行错误: {0}")]
    Primitive(String),
    #[error("事件执行错误: {0}")]
    Event(String),
    #[error("配置错误: {0}")]
    Config(String),
    #[error("权限错误: {0}")]
    Permission(String),
    #[error("工具调用错误: {0}")]
    Tool(String),
    #[error("序列化错误: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("Wasm运行时错误: {0}")]
    WasmEdge(String),
    #[error("JS执行错误: {0}")]
    QuickJS(String),
    #[error("审计日志错误: {0}")]
    Audit(String),
}

[CODE_BLOCK_P1_002] 核心配置结构

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppConfig {
    pub log: LogConfig,
    pub llm: LlmConfig,
    pub storage: StorageConfig,
    pub plugin: PluginConfig,
    pub security: SecurityConfig,
    pub primitive: PrimitiveConfig,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PrimitiveConfig {
    pub path_whitelist: Vec<String>,
    pub path_blacklist: Vec<String>,
    pub bash_whitelist: Vec<String>,
    pub bash_approval_required: Vec<String>,
    pub bash_forbidden: Vec<String>,
    pub auto_confirm: bool,
    pub auto_confirm_whitelist: Vec<String>,
    pub require_approval_for_all_write: bool,
    pub require_approval_for_all_bash: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SecurityConfig {
    pub default_plugin_permission_level: PermissionLevel,
    pub enable_audit_log: bool,
    pub audit_log_retention_days: u32,
    pub enable_plugin_safety_scan: bool,
    // TODO: 敏感数据加密后续考虑时再启用
    // pub sensitive_data_encryption_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum PermissionLevel {
    Restricted,
    Normal,
    Trusted,
}

[CODE_BLOCK_P1_003] 核心事件枚举（规范见 Architecture.md）
事件分为 AgentEvent（流式/UI）与 ExtensionEvent（扩展钩子），序列化 type 为 snake_case，payload 为 camelCase，与 pi_agent_rust 一致。完整枚举与 payload 定义见 Architecture.md「事件系统设计」。

[CODE_BLOCK_P1_004] 事件总线 Trait
扩展侧使用字符串事件名（与 pi-mono 一致），内部可映射到 ExtensionEvent 枚举。
#[async_trait]
pub trait EventBus: Send + Sync + 'static {
    /// 扩展通过字符串事件名注册，event_name 与 pi-mono 一致（如 "tool_call", "session_before_switch", "input"）
    fn on(&self, event_name: &str, callback: EventCallback) -> EventListenerId;
    fn once(&self, event_name: &str, callback: EventCallback) -> EventListenerId;
    fn off(&self, listener_id: EventListenerId);
    fn emit_sync(&self, event_name: &str, context: EventContext) -> Result<(), AppError>;
    async fn emit_async(&self, event_name: &str, context: EventContext) -> Result<(), AppError>;
    fn remove_plugin_listeners(&self, plugin_id: &str);
}

pub type EventCallback = Box<dyn FnMut(EventContext) -> Result<(), AppError> + Send + Sync>;

[CODE_BLOCK_P1_005] 统一 LLM Provider Trait
#[async_trait]
pub trait LlmProvider: Send + Sync + 'static {
    fn provider_name(&self) -> &str;
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, AppError>;
    async fn chat_stream(&self, request: ChatRequest) -> Result<tokio_stream::BoxStream<'static, Result<StreamEvent, AppError>>, AppError>;
    fn count_tokens(&self, messages: &[ChatMessage]) -> Result<u32, AppError>;
}

[CODE_BLOCK_P1_006] 4 原语核心结构与 API
#[async_trait]
pub trait PrimitiveExecutor: Send + Sync + 'static {
    async fn read_file(&self, path: &str, plugin_id: &str) -> Result<String, AppError>;
    async fn list_dir(&self, path: &str, plugin_id: &str) -> Result<Vec<DirEntry>, AppError>;
    async fn write_file(&self, path: &str, content: &str, overwrite: bool, plugin_id: &str) -> Result<WriteFileResult, AppError>;
    async fn edit_file(&self, path: &str, edits: Vec<EditOperation>, plugin_id: &str) -> Result<EditFileResult, AppError>;
    async fn execute_bash(&self, command: &str, cwd: Option<&str>, plugin_id: &str) -> Result<BashResult, AppError>;
    async fn require_user_confirmation(&self, operation: PrimitiveOperation, preview: &str, plugin_id: &str) -> Result<bool, AppError>;
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EditOperation {
    pub operation_type: EditOperationType,
    pub start_line: Option<u64>,
    pub end_line: Option<u64>,
    pub old_content: Option<String>,
    pub new_content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum EditOperationType {
    Replace,
    Insert,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum PrimitiveOperation {
    Read,
    Write,
    Edit,
    Bash,
}

[CODE_BLOCK_P1_007] 工具注册中心 Trait
与 pi-mono ToolDefinition 对齐：name、label、description、parameters（JSON Schema）、execute 语义；返回值形态与 AgentToolResult（content、details）一致。
#[async_trait]
pub trait ToolRegistry: Send + Sync + 'static {
    async fn register_tool(&self, tool: Tool, plugin_id: &str) -> Result<(), AppError>;
    async fn unregister_tool(&self, tool_name: &str, plugin_id: &str) -> Result<(), AppError>;
    async fn get_tool(&self, tool_name: &str) -> Result<Tool, AppError>;
    async fn list_tools(&self, plugin_id: Option<&str>) -> Result<Vec<Tool>, AppError>;
    async fn call_tool(&self, tool_name: &str, params: serde_json::Value, plugin_id: &str) -> Result<serde_json::Value, AppError>;
    fn unregister_plugin_tools(&self, plugin_id: &str);
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Tool {
    pub name: String,
    pub label: String,
    pub description: String,
    /// JSON Schema，与 pi-mono parameters 一致
    pub parameters: serde_json::Value,
    pub plugin_id: String,
    pub is_enabled: bool,
    pub created_at: i64,
}

[CODE_BLOCK_P1_008] 插件核心结构
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub main: String,
    pub required_permissions: Vec<String>,
    pub required_api_version: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PluginInstance {
    pub id: String,
    pub manifest: PluginManifest,
    pub wasm_instance: WasmEdgeInstance,
    pub js_context: QuickJSContext,
    pub status: PluginStatus,
    pub registered_tools: Vec<String>,
    pub event_listeners: Vec<EventListenerId>,
    pub config: serde_json::Value,
    pub created_at: i64,
    pub loaded_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginStatus {
    Unloaded,
    Loading,
    Loaded,
    Enabled,
    Disabled,
    Error,
}

[CODE_BLOCK_P1_009] 插件执行核心流程
// 插件加载执行核心流程伪代码
1.  解析插件清单与代码
    let manifest = parse_plugin_manifest(plugin_path)?;
    let plugin_code = read_plugin_code(manifest.main)?;

2.  权限校验与用户确认
    validate_permissions(manifest.required_permissions)?;
    let user_confirm = show_permission_dialog(&manifest)?;
    if !user_confirm {
        return Err(AppError::Permission("用户拒绝插件授权".to_string()));
    }

3.  创建WasmEdge实例
    let mut instance = wasm_edge_engine.create_instance()?;
    instance.register_wasi()?;
    instance.enable_quickjs_runtime()?;
    instance.enable_nodejs_compat_layer()?;

4.  注册宿主API到导入表
    let authorized_apis = get_authorized_apis(&manifest.required_permissions);
    for api in authorized_apis {
        instance.register_import(api.name, api.handler)?;
    }

5.  初始化QuickJS运行时，执行插件代码
    let js_context = instance.init_quickjs()?;
    js_context.inject_global_agent_object()?;
    js_context.execute(plugin_code)?;

6.  执行插件初始化，完成注册
    let plugin_instance = PluginInstance {
        id: manifest.id.clone(),
        manifest,
        wasm_instance: instance,
        js_context,
        status: PluginStatus::Loaded,
        ..Default::default()
    };
    plugin_manager.register_plugin(plugin_instance)?;

7.  触发PluginLoad事件，启用插件
    event_bus.emit_sync(AgentEvent::PluginLoad, event_context)?;
    plugin_manager.enable_plugin(plugin_id)?;

---

## 会话管理数据结构设计

- 会话路径、sessionKey/sessionId、元数据 store（sessions.json）、对话 transcript（pi 系 JSONL）及 SessionEntry/SessionHeader/EntryBase 等类型定义，**均以 [Architecture.md](../../specs/Architecture.md) 中「会话存储数据结构设计」为准**；MVP 实现直接引用该节，此处不再重复。配置与数据布局（工作根目录 work_dir、多 agent 目录）见 [工作目录与数据布局](../../specs/architecture/work-dir-and-data-layout.md)。

- 消息管理
    - 增（Create）：通过 SessionManager 的 appendMessage、appendThinkingLevelChange、appendModelChange、appendCompaction、appendSessionInfo、appendLabelChange 等，在内存里追加 entry 并持久化到 JSONL。
    - 查（Read）：getEntry(id)、getEntries()、getBranch()、getTree()、getChildren()、getLeafEntry() 等，都是对已加载的 session 做查询。
    - 改（Update）：没有“改某一条 entry”的 API。transcript 设计成 append-only，没有 updateEntry / editEntry。要“改”只能通过分支（branch / branchWithSummary）换一条新路径，或自己改文件。
    - 删（Delete）：只支持 整场会话 的删除（删掉该会话的 .jsonl 文件，或通过 /resume 里 Ctrl+D）。没有“删某一条 message/entry”的 API。


---