//! # pi_awsm 库
//!
//! 基础设施层与核心能力层（含 LLM、会话、4 原语、工具、插件），供 session_cli / wasm_plugin / chat 等模块依赖。
//! 对外 API 通过 `infra`、`core`、`ext` 统一暴露，符合编码与分层架构规范。

pub mod api;
pub mod core;
pub mod ext;
pub mod infra;

pub use api::run_cli;
pub use core::{
    AllowAllConfirmation, BashResult, ChatMessage, ChatRequest, ChatResponse, DefaultPrimitiveExecutor,
    DefaultToolRegistry, DenyAllConfirmation, DirEntry, EditFileResult, EditOperation, EditOperationType,
    LlmProvider, OpenAiProvider, PrimitiveExecutor, PrimitiveOperation, SessionTokenUsage, StreamEvent,
    Tool, ToolExecutor, ToolRegistry, UserConfirmationProvider, WriteFileResult,
};
pub use core::{
    load_store, save_store, SessionEntry, SessionHeader, SessionManager, SessionStore,
    TranscriptEntry, DEFAULT_SESSION_KEY,
};
pub use ext::{
    HostApiDispatcher, HostRequest, HostResponse, PluginInfo, PluginInstance, PluginManager,
    PluginManifest, PluginStatus, WasmEngine, WasmEngineConfig, WasmInstance, invoke_host_func,
    invoke_host_func_with, parse_manifest,
};
pub use infra::{
    init_logging, load_config, normalize_path, read_file_utf8, validate_config, write_file_atomic,
    AgentEvent, AppConfig, AppError, AuditPrimitiveOp, AuditRecorder, DefaultEventBus, EventBus,
    EventContext, EventListenerId, ExtensionEvent, HostcallAuditEntry, LlmConfig, LogConfig,
    PrimitiveAuditEntry, PrimitiveConfig, SecurityConfig, ToolAuditEntry, TracingAuditRecorder,
};
