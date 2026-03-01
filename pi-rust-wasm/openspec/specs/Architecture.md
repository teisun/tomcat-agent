# pi-rust-awsm 整体技术架构

## 设计原则

1.  **pi 生态全兼容**：以 pi-mono 为兼容性契约，API、事件机制、插件规范与社区插件零修改运行为目标。
2.  **安全隔离优先**：所有插件代码运行在 WasmEdge 独立沙箱内，宿主可信逻辑与插件不可信逻辑完全分离，仅通过显式注册的 API 通信。
3.  **极简分层**：严格遵循单向依赖、无循环依赖原则，核心层仅保留可信基础能力，所有扩展能力均通过插件实现。
4.  **原生性能**：Rust 宿主层负责核心调度与可信逻辑，WasmEdge 负责沙箱内 JS/TS 代码执行，兼顾生态兼容性与原生性能。
5.  **可插拔设计**：所有非核心能力均通过插件化实现，不耦合主程序核心逻辑，支持按需启用/禁用。

### pi 生态参考原则（双仓对照）

所有影响「兼容 pi 生态」的技术设计，**必须同时参考 pi-mono 与 pi-agent-rust 两个仓库**，并遵循以下分工：

- **pi-mono**：作为**兼容性契约与行为基准**。扩展作者面向的是 TypeScript/JS 的 API、事件名、会话与 RPC 协议；「与 pi 生态兼容」的最终标准是**与 pi-mono 的对外行为与接口一致**。事件名、API 形态、payload 结构、协议语义等以 **pi-mono 为权威**。
- **pi-agent-rust**：作为 **Rust 侧的主要实现参考**。事件拆分（AgentEvent / ExtensionEvent）、hostcall 设计、扩展加载与 QuickJS 集成、会话/工具/权限等实现可优先参考 pi-agent-rust；其已与 pi-mono 对齐的部分可直接沿用。
- **二者不一致时**：以 **pi-mono 的语义为准**，在 pi-rust-wasm 中按 pi-mono 实现，再在 pi-rust-wasm 里用 Rust 实现出来, 不把 pi-agent-rust 的当前行为当作最终标准（pi-agent-rust 的 drop-in 认证当前为 NOT_CERTIFIED，存在已知差距）。

## 整体分层架构
从宿主可信层到沙箱插件层，单向依赖、边界清晰，架构层级从下到上依次为：
**基础设施层 → 宿主核心能力层 → 宿主API层 → WasmEdge运行时层 → 沙箱执行层 → 交互层**

## 各层核心模块详细设计
### 1. 基础设施层
项目的底层可信基础能力，所有上层模块均依赖该层，无任何业务逻辑，保证跨平台通用，完全基于Rust安全实现。
- 统一错误处理体系：基于thiserror+anyhow实现统一错误枚举，所有错误包含完整上下文，禁止裸panic与滥用unwrap()
- 配置管理模块：基于config-rs实现多源配置合并，支持配置热更新
- 日志与审计系统：基于tracing实现分级日志，独立审计日志模块，全链路记录4原语调用、插件执行、高危操作
- 跨平台基础适配：封装通用文件操作、进程管理、系统信息获取接口，抹平Windows/macOS/Linux平台差异
- 事件总线：参考pi-agent-rust设计，实现全局同步/异步事件总线，是宿主与插件、插件与插件通信的核心通道，替代原钩子设计

### 2. 宿主核心能力层
项目的可信核心引擎，所有业务逻辑的底层支撑，仅在宿主层运行，不向插件开放直接访问权限。
#### 2.1 会话管理模块

负责会话全生命周期管理、对话上下文组装、消息持久化与会话关联追溯；支持会话级插件、权限、LLM 配置隔离。设计面向多 Agent、多 channel。

- **存储约束**：会话内容不使用 SQLite，仅使用 **pi 系 JSONL transcript**；索引与路由由 **sessions.json**（元数据 store）提供。
- **两层**：元数据 store（sessions.json，`sessionKey -> SessionEntry`）+ 对话 transcript（pi 系 JSONL，与 pi-mono 格式兼容）。
- **约定**：列表与「当前会话」由 sessions.json 提供；transcript 内容以 JSONL 为准，sessions.json 为元数据与路由的权威。

会话路径、sessionKey/sessionId 约定及 SessionEntry、transcript 格式等见文末 **会话存储数据结构设计**。

#### 2.2 LLM接入模块
基于适配器模式实现统一LLM Provider Trait，兼容所有OpenAI格式大模型，支持流式响应、限流重试、Token统计、会话级模型配置，是插件调用LLM能力的唯一可信入口。

#### 2.3 4原语执行引擎
宿主可信核心，完全对齐pi-mono的4原语规范，是插件访问系统资源的唯一通道，所有操作必须经过权限校验、用户确认、审计日志记录。
- **Read原语**：文件读取、目录列表、元数据获取，路径白名单校验，大文件分块读取
- **Write原语**：文件写入、目录创建、路径删除，操作前备份、用户二次确认、权限校验
- **Edit原语**：基于diff的精确行编辑、内容替换，编辑前diff预览、原子化操作、失败自动回滚
- **Bash原语**：shell命令执行，分级管控（白名单/审批/禁止）、实时流式输出、资源限制、超时控制、完整审计

#### 2.4 工具注册中心
参考pi-agent-rust设计，实现全局工具注册与管理，宿主内置工具、插件注册的自定义工具统一管理，支持工具的注册/注销、权限校验、调用统计，是插件扩展能力的核心入口。

#### 2.5 插件生命周期管理
负责插件的加载、初始化、启动、停止、卸载全流程管理，每个插件对应独立的WasmEdge实例，完全隔离，执行完成后完全释放资源，无内存泄漏。

#### 2.6 权限管控模块
插件级细粒度权限管控，默认最小权限原则，支持4原语权限、网络权限、LLM调用权限、工具访问权限的精细化配置，所有跨宿主调用必须经过权限校验。

### 3. 宿主API层
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

### 4. WasmEdge运行时层
项目的沙箱隔离核心，基于WasmEdge官方构建，全局单例Engine，每个插件对应独立的Store/Instance，完全隔离的线性内存与执行环境，是JS/TS插件的执行载体。
#### 4.1 核心组件
- 全局WasmEdge Engine：全局唯一初始化，负责Wasm模块编译、内存管理、实例调度，仅一次初始化开销
- 插件独立Wasm实例：每个插件对应一个独立的WasmEdge实例，专属线性内存、调用栈、QuickJS上下文，故障不扩散、数据不串扰
- QuickJS运行时：WasmEdge官方优化版QuickJS引擎，内置到Wasm实例中，原生支持JS/TS代码执行，无需手动打包嵌入
- 宿主导入绑定：将宿主API显式注册到Wasm实例的导入表，映射为QuickJS全局对象，插件可直接调用，调用请求通过WasmEdge通道转发到宿主层执行
- WASI标准支持：原生支持WASI Preview2、WASI-Socket、WASI-HTTP，网络、文件IO能力受宿主权限管控

#### 4.2 插件加载执行流程
1.  宿主读取插件JS/TS代码，校验插件清单与权限声明
2.  创建独立的WasmEdge实例，初始化QuickJS运行时与Node.js兼容层
3.  向实例导入表注册该插件授权的宿主API，未授权API不注册
4.  将插件代码注入QuickJS运行时，执行插件初始化逻辑，注册工具、监听事件
5.  插件运行时，调用宿主API时，通过WasmEdge通道转发到宿主层，经过权限校验后执行，结果原路返回
6.  插件卸载时，销毁WasmEdge实例，完全释放所有内存与资源，无残留

### 5. 沙箱执行层
插件代码的实际运行环境，完全隔离于宿主系统，仅能通过显式注册的宿主API与外界交互。
- 插件执行上下文：每个插件独立的QuickJS上下文，全局作用域隔离，插件间无法直接互相访问内存
- 权限边界：仅能使用宿主授权的API，未授权的系统调用、网络访问、文件操作直接拦截
- 资源限制：每个插件实例可配置CPU、内存、执行超时硬限制，避免资源耗尽
- 错误隔离：插件执行错误完全捕获，不传递到宿主主程序，不会导致宿主崩溃
- 模块加载：支持插件内npm包加载、相对路径模块导入，完全兼容pi-mono插件的模块规范

### 6. 交互层
用户与引擎交互的入口，优先实现CLI工具，后续扩展Web/移动端界面。
- CLI交互层：基于clap+tokio实现，支持会话管理、对话交互、插件管理、配置管理、审计日志查看
- IPC接口层：统一的前后端接口规范，为后续Web/移动端界面预留，与CLI复用同一套核心服务
- 前端交互层（预留）：基于Tauri+React实现，全平台可视化界面，核心能力与CLI完全对齐

### 7. 安全设计核心原则

- **最小权限原则**：插件默认最小权限，仅授予完成任务所需的宿主 API，禁止过度授权
- **完全隔离原则**：每个插件在独立 WasmEdge 沙箱中，内存与执行环境隔离，故障不扩散
- **唯一通道原则**：插件仅能通过显式注册的宿主 API 与宿主交互，禁止绕过 API 的系统访问
- **用户知情权原则**：4 原语与高危操作须告知用户并获二次确认，禁止静默执行
- **错误完全隔离原则**：插件与事件回调的错误独立捕获，不导致宿主崩溃
- **全链路审计原则**：4 原语、工具调用、插件生命周期、高危操作留存完整审计日志，可追溯
- **代码安全校验原则**：插件加载前须安全扫描，禁止恶意或越权代码加载

**TODO**：敏感数据加密（如 LLM API 密钥）后续再考虑。

## 事件系统设计（替代原钩子设计，完全对齐pi-agent-rust）
### 核心设计原则
基于发布-订阅模式，全局事件总线，支持同步/异步事件监听，是宿主与插件、插件与插件之间通信的唯一方式，完全对齐pi-mono的事件规范。

### 事件分类（对齐 pi_agent_rust）

事件分为两类：**AgentEvent** 供流式/UI 订阅；**ExtensionEvent** 供扩展通过 `agent.on(event_name, ...)` 注册钩子。扩展侧使用**字符串事件名**（snake_case，如 `"tool_call"`、`"session_before_switch"`、`"input"`），与 pi-mono / pi_agent_rust 一致。序列化时 `type` 为 snake_case，payload 字段为 camelCase。

#### AgentEvent（流式 / UI）

用于 TUI、JSON 模式等，携带完整上下文；与 pi_agent_rust `agent.rs` 对齐。

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    AgentStart { #[serde(rename = "sessionId")] session_id: Arc<str> },
    AgentEnd { #[serde(rename = "sessionId")] session_id: Arc<str>, messages: Vec<Message>, error: Option<String> },
    TurnStart { #[serde(rename = "sessionId")] session_id: Arc<str>, #[serde(rename = "turnIndex")] turn_index: usize, timestamp: i64 },
    TurnEnd { #[serde(rename = "sessionId")] session_id: Arc<str>, #[serde(rename = "turnIndex")] turn_index: usize, message: Message, #[serde(rename = "toolResults")] tool_results: Vec<Message> },
    MessageStart { message: Message },
    MessageUpdate { message: Message, #[serde(rename = "assistantMessageEvent")] assistant_message_event: AssistantMessageEvent },
    MessageEnd { message: Message },
    ToolExecutionStart { #[serde(rename = "toolCallId")] tool_call_id: String, #[serde(rename = "toolName")] tool_name: String, args: Value },
    ToolExecutionUpdate { #[serde(rename = "toolCallId")] tool_call_id: String, #[serde(rename = "toolName")] tool_name: String, args: Value, #[serde(rename = "partialResult")] partial_result: ToolOutput },
    ToolExecutionEnd { #[serde(rename = "toolCallId")] tool_call_id: String, #[serde(rename = "toolName")] tool_name: String, result: ToolOutput, #[serde(rename = "isError")] is_error: bool },
    AutoCompactionStart { reason: String },
    AutoCompactionEnd { result: Option<Value>, aborted: bool, #[serde(rename = "willRetry")] will_retry: bool, #[serde(rename = "errorMessage")] error_message: Option<String> },
    AutoRetryStart { attempt: u32, #[serde(rename = "maxAttempts")] max_attempts: u32, #[serde(rename = "delayMs")] delay_ms: u64, #[serde(rename = "errorMessage")] error_message: String },
    AutoRetryEnd { success: bool, attempt: u32, #[serde(rename = "finalError")] final_error: Option<String> },
    ExtensionError { #[serde(rename = "extensionId")] extension_id: Option<String>, event: String, error: String },
}
```

#### ExtensionEvent（扩展钩子）

与 pi_agent_rust `extension_events.rs` 一致的事件名与 payload；保留会话/插件/系统等扩展事件时同样使用 snake_case + camelCase。

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExtensionEvent {
    #[serde(rename_all = "camelCase")]
    Startup { version: String, session_file: Option<String> },
    #[serde(rename_all = "camelCase")]
    AgentStart { session_id: String },
    #[serde(rename_all = "camelCase")]
    AgentEnd { session_id: String, messages: Vec<Message>, error: Option<String> },
    #[serde(rename_all = "camelCase")]
    TurnStart { session_id: String, turn_index: usize },
    #[serde(rename_all = "camelCase")]
    TurnEnd { session_id: String, turn_index: usize, message: AssistantMessage, tool_results: Vec<ToolResultMessage> },
    #[serde(rename_all = "camelCase")]
    ToolCall { tool_name: String, tool_call_id: String, input: Value },
    #[serde(rename_all = "camelCase")]
    ToolResult { tool_name: String, tool_call_id: String, input: Value, content: Vec<ContentBlock>, details: Option<Value>, is_error: bool },
    #[serde(rename_all = "camelCase")]
    SessionBeforeSwitch { current_session: Option<String>, target_session: String },
    #[serde(rename_all = "camelCase")]
    SessionBeforeFork { current_session: Option<String>, fork_entry_id: String },
    #[serde(rename_all = "camelCase")]
    Input { #[serde(rename = "text")] content: String, #[serde(rename = "images")] attachments: Vec<ImageContent> },
    // 保留：会话/插件/系统/4原语等扩展事件，命名同上
    // SessionCreate, SessionDestroy, SessionSwitch, PluginLoad/Unload/Enable/Disable, ToolRegister/Unregister, ToolCallError, SystemReady, SystemShutdown, ConfigChange, Custom(String) 等
}
```

### 事件执行机制

- 宿主在关键节点发布 **AgentEvent**（流式/UI）与 **ExtensionEvent**（扩展钩子），携带完整上下文
- 扩展通过 `agent.on("tool_call", ...)` 等**字符串事件名**监听 ExtensionEvent，与 pi-mono 一致
- 按注册顺序执行回调，支持同步/异步；单次回调错误不影响其他回调与主流程
- 扩展通过 `agent.emit()` 发布自定义事件（如 Custom 前缀），实现插件间通信
- 插件卸载时自动注销该插件所有监听，无泄漏


---

## 会话存储数据结构设计

### 元数据 store（sessions.json）

单文件 JSON：`sessionKey -> SessionEntry`。列表与路由由此提供，不另建 SQLite 索引。

```rust
/// 会话根目录：~/.pi/agent/sessions/ 或 ~/.pi/agent/sessions/<agentId>/
/// sessionKey 格式：agent:<agentId>:<channelKey>，MVP 单入口用 agent:default:main
pub type SessionStore = std::collections::HashMap<String, SessionEntry>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    pub session_id: String,           // 当前 transcript 文件 id，对应 <sessionId>.jsonl 或 pi 系 <timestamp>_<uuid>.jsonl
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>, // 可选显式 transcript 路径
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction_count: Option<u32>,
    // 预留：channel/agent 相关字段供三期多 channel 使用
}
```

### 对话 transcript（pi 系 JSONL）

每会话一个 `.jsonl` 文件：**每行一个 JSON 对象**（非管道分隔）；首行 session header，后续每行一条 entry，树形 id/parentId。内存中为结构化类型（pi-mono 为 `SessionEntry` 联合类型），落盘时每行 `JSON.stringify(entry)`。与 pi-mono 格式兼容。

```rust
/// 首行：session header
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeader {
    pub r#type: String, // "session"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>, // 3
    pub id: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// 后续每行：一条 SessionEntry。内存中为 enum 联合类型，落盘时每行序列化一个变体。
/// JSON 通过 type 字段区分（snake_case），与 pi-mono / pi_agent_rust 一致。
/// 参考：[session-pi-mono-format.jsonl](examples/session-pi-mono-format.jsonl)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEntry {
    Message(MessageEntry),
    ModelChange(ModelChangeEntry),
    ThinkingLevelChange(ThinkingLevelChangeEntry),
    Compaction(CompactionEntry),
    BranchSummary(BranchSummaryEntry),
    Label(LabelEntry),
    SessionInfo(SessionInfoEntry),
    Custom(CustomEntry),
}

/// 各 entry 变体均包含或 flatten 公共基座：id、parent_id、timestamp，组成树形结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryBase {
    pub id: Option<String>,
    pub parent_id: Option<String>,
    pub timestamp: String,
}
```

**会话路径与会话标识**
- **会话根目录** '~/.pi/agent/sessions/'; 按Agent分子目录预留多Agent设计 (如'~/.pi/agent/sessions/<agentId>/'), mvp先单agentId或固定default。
- **sessionKey** (路由键，预留多channel)：'agent:<agentId>:<channelKey>', MVP可用'agent:default:main' 后续channnelKey可扩展如: 'agent:mybot:telegram:group:123'
- **sessionId** 当前对话对应的 transcript 唯一 id(sessionId=<timestamp>_<uuid>)，对应文件名'<sessionId>.jsonl'; SessionEntry中'sessionId'指向改文件

**Source of truth**：transcript 内容以 JSONL 文件为准；sessions.json 为元数据与路由的权威，写入时覆盖该文件。

---
