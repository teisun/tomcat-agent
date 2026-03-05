# 基础设施层说明 (Infrastructure Layer)

## 1. 概述 (Overview)

- **职责**：为上层模块（session_cli、llm、wasm_plugin、primitives_tools、chat）提供统一的错误处理、配置管理、分级日志、跨平台路径与文件操作，以及解耦的全局事件总线。
- **所在层级**：基础设施层（全项目起点，无上游依赖）。
- **核心文件**：
  - `src/lib.rs` — 门面，声明 `pub mod infra` 并 re-export 对外 API
  - `src/infra/mod.rs` — 基础设施层聚合，`pub(crate) mod` 子模块与选择性 `pub use`
  - `src/infra/error.rs` — 统一错误枚举 `AppError`
  - `src/infra/config.rs` — 配置结构体与加载/校验
  - `src/infra/logging.rs` — tracing 分级日志初始化
  - `src/infra/platform.rs` — 路径规范化、原子写入、系统信息
  - `src/infra/event_bus.rs` — 事件总线 Trait 与默认实现
  - `src/infra/events.rs` — `AgentEvent` / `ExtensionEvent` 枚举定义

设计原则：最小依赖、强类型约束、错误完整捕获不导致主流程崩溃。

---

## 2. 设计方案 (Design Details)

### 2.1 设计模式与关键权衡

- **错误处理**：采用 `thiserror` 枚举 + `anyhow` 可选包装；禁止裸 `panic`、慎用 `unwrap()`，所有错误可追溯。
- **配置**：多源合并（默认值 → 配置文件 → 环境变量），环境变量前缀 `PI_AWSM__`、分隔符 `__`，与 config-rs 约定一致。
- **事件总线**：发布-订阅，基于 `Arc` + `RwLock` 的 `HashMap` 存储监听器；单 listener 抛错或 panic 时通过 `catch_unwind` 捕获并打日志，其余 listener 照常执行，主流程不崩溃。
- **线程安全**：`EventBus`、`DefaultEventBus` 要求 `Send + Sync`；回调类型 `EventCallback` 为 `Box<dyn FnMut(EventContext) -> Result<(), AppError> + Send + Sync>`。

### 2.2 与 pi 生态对齐

- 扩展侧使用**字符串事件名**（如 `tool_call`、`session_before_switch`、`input`），snake_case，与 pi-mono 一致。
- 事件 payload 使用 camelCase（见 `events.rs` 中 `ExtensionEvent` 的 `#[serde(rename_all = "camelCase")]`）。

---

## 3. 核心 API 与数据结构 (API Definitions)

### 3.1 统一错误 (AppError)

```rust
// src/infra/error.rs
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
```

MVP 会话与审计均不使用 SQLite，故不包含 `Db` 变体。各层通过 `Result<T, AppError>` 或 `anyhow` 包装使用。

### 3.2 配置 (AppConfig 及子结构)

- **AppConfig**：顶层配置，包含 `log`、`llm`、`storage`、`plugin`、`security`、`primitive`。
- **LogConfig**：`level`（trace/debug/info/warn/error）、`file_enabled`、`file_path`、`file_roll_size_mb`。
- **LlmConfig**：`provider`、`api_base`、`api_key_env`、`default_model`。
- **StorageConfig**：`sessions_dir`。
- **PluginConfig**：`plugins_dir`、`auto_load`。
- **PrimitiveConfig**：路径/命令白名单与审批、`auto_confirm`、`require_approval_for_all_write` 等。
- **SecurityConfig**：`default_plugin_permission_level`、`enable_audit_log`、`audit_log_retention_days`、`enable_plugin_safety_scan`。

**加载与校验**：

- `load_config(config_path: Option<&Path>) -> Result<AppConfig, AppError>`：从可选配置文件和环境变量合并。
- `validate_config(cfg: &AppConfig) -> Result<(), AppError>`：校验日志级别、`audit_log_retention_days > 0` 等，启动时调用。

### 3.3 事件总线 (EventBus)

```rust
// src/infra/event_bus.rs
pub type EventCallback = Box<dyn FnMut(EventContext) -> Result<(), AppError> + Send + Sync>;

#[async_trait]
pub trait EventBus: Send + Sync + 'static {
    fn on(&self, event_name: &str, callback: EventCallback) -> EventListenerId;
    fn once(&self, event_name: &str, callback: EventCallback) -> EventListenerId;
    fn off(&self, listener_id: EventListenerId);
    fn emit_sync(&self, event_name: &str, context: EventContext) -> Result<(), AppError>;
    async fn emit_async(&self, event_name: &str, context: EventContext) -> Result<(), AppError>;
    fn remove_plugin_listeners(&self, plugin_id: &str);
}
```

- **EventContext**：`event_name`、`payload`（`serde_json::Value`）、`plugin_id`、`priority`；支持 `with_plugin_id` / `with_priority` 链式构造。
- **DefaultEventBus**：`add_listener(event_name, once, plugin_id, priority, callback)` 供插件注册时传入 `plugin_id`，便于卸载时 `remove_plugin_listeners(plugin_id)` 一键清理。

### 3.4 事件枚举 (AgentEvent / ExtensionEvent)

- **AgentEvent**：流式/UI 相关，如 `AgentStart`、`TurnStart`、`MessageUpdate`、`ToolExecutionEnd`、`ExtensionError` 等；`type` snake_case，payload 字段 camelCase。
- **ExtensionEvent**：扩展钩子，如 `Startup`、`ToolCall`、`ToolResult`、`SessionBeforeSwitch`、`Input` 等；与 Architecture.md 事件系统设计一致。

### 3.5 平台与日志

- **platform**：`normalize_path(path)`、`read_file_utf8(path)`、`write_file_atomic(path, content)`、`current_dir()`、`system_info()`（`SystemInfo { os, arch }`）。
- **logging**：`init_logging(cfg: &LogConfig) -> Result<(), AppError>`，基于 tracing，控制台 + 可选按天滚动文件，禁止在日志中打印敏感信息。

---

## 4. 配置项 (Configuration)

| 环境变量 / 配置路径 | 说明 | 默认值 |
|--------------------|------|--------|
| `PI_AWSM__*`（`__` 为嵌套分隔） | 覆盖对应配置项 | - |
| 配置文件 | TOML，由 `load_config(Some(path))` 指定 | - |
| `log.level` | trace / debug / info / warn / error | info |
| `log.file_enabled` | 是否写文件 | false |
| `log.file_path` | 日志文件路径 | pi_awsm.log |
| `log.file_roll_size_mb` | 滚动大小（MB） | 10 |
| `storage.sessions_dir` | 会话目录 | ~/.pi/agent/sessions |
| `plugin.plugins_dir` | 插件目录 | ~/.pi/agent/plugins |
| `security.enable_audit_log` | 是否启用审计日志 | true |
| `security.audit_log_retention_days` | 审计保留天数 | 90 |
| **预留** `memory.profile` | low / standard / high / auto，见 [Architecture 4.5 资源与内存模式](../openspec/specs/Architecture.md#45-资源与内存模式-resource--memory-profile) | - |
| **预留** `memory.*` | 各模式覆盖项（如 wasm_max_pages、js_heap_limit 等），同上 | - |

---

## 5. 交互流程 (Workflow)

### 5.1 启动阶段

1. 调用 `load_config(Some(config_path))` 或 `load_config(None)` 得到 `AppConfig`。
2. 调用 `validate_config(&cfg)`，失败则拒绝启动。
3. 调用 `init_logging(&cfg.log)` 初始化 tracing。

### 5.2 事件总线典型流程

1. **注册**：插件或宿主调用 `event_bus.on("tool_call", callback)` 或 `add_listener(..., Some(plugin_id), priority, callback)`。
2. **触发**：某模块调用 `emit_sync("tool_call", context)` 或 `emit_async(...)`。
3. **分发**：按 `priority` 降序执行回调；单次回调返回 `Err` 或 panic 仅记录日志，不中断其他回调和主流程。
4. **清理**：插件卸载时调用 `remove_plugin_listeners(plugin_id)`，或单次注销 `off(listener_id)`。

---

## 6. 示例代码 (Usage Examples)

### 6.1 加载配置并初始化日志

```rust
use pi_awsm::{load_config, validate_config, init_logging};

let cfg = load_config(Some(std::path::Path::new("config.toml")))?;
validate_config(&cfg)?;
init_logging(&cfg.log)?;
```

### 6.2 注册与触发事件

```rust
use pi_awsm::{DefaultEventBus, EventBus, EventContext};

let bus = DefaultEventBus::new();
let id = bus.on("tool_call", Box::new(|ctx| {
    println!("tool_call: {}", ctx.payload);
    Ok(())
}));
let ctx = EventContext::new("tool_call", serde_json::json!({"toolName": "read_file"}));
let _ = bus.emit_sync("tool_call", ctx);
bus.off(id);
```

### 6.3 插件注册与卸载时清理

```rust
let id = bus.add_listener("input", false, Some("my_plugin".to_string()), 0, callback);
// ... 插件运行 ...
bus.remove_plugin_listeners("my_plugin");
```

---

## 7. 验收标准 (Testing & QA)

- **门禁**：`cargo clippy`、`cargo fmt` 通过。
- **单测**：基础设施层单测覆盖率 ≥ 90%；错误、配置、事件总线、平台、日志均有单元测试。
- **边界**：单 listener 抛错或 panic 时，其余 listener 仍执行、主流程不崩溃（见 `event_bus::tests` 中 `single_listener_error_does_not_abort_others`、`listener_panic_is_caught_others_still_run`）。
