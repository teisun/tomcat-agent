//! # pi_wasm 库
//!
//! 基础设施层与核心能力层（含 LLM、会话、4 原语、工具、插件），供 session_cli / wasm_plugin / chat 等模块依赖。
//! 对外 API 通过 `infra`、`core`、`ext` 统一暴露，符合编码与分层架构规范。

pub mod api;
pub mod core;
pub mod ext;
pub mod infra;

pub use api::run_cli;
pub use core::{
    agent_messages_from_chat, convert_to_llm_format, AgentLoop, AgentLoopConfig, AgentMessage,
    AgentRunResult, AllowAllConfirmation, BashResult, ChatMessage, ChatRequest, ChatResponse,
    DefaultPrimitiveExecutor, DefaultToolRegistry, DenyAllConfirmation, DirEntry, EditFileResult,
    EditOperation, EditOperationType, LlmProvider, OpenAiProvider, PrimitiveExecutor,
    PrimitiveOperation, SessionTokenUsage, StreamEvent, Tool, ToolCallInfo, ToolExecutor,
    ToolRegistry, UserConfirmationProvider, WriteFileResult,
};
pub use core::{
    load_store, save_store, SessionEntry, SessionHeader, SessionManager, SessionStore,
    TranscriptEntry, DEFAULT_SESSION_KEY,
};
pub use ext::{
    invoke_host_func, invoke_host_func_with, parse_manifest, transpile_pi_plugin_for_quickjs,
    transpile_typescript, EventEnvelope, HostApiDispatcher, HostRequest, HostResponse, PluginInfo,
    PluginInstance, PluginManager, PluginManifest, PluginStatus, RuntimeManager,
    SharedRuntimeManager, VmActorHandle, VmActorState, VmCommand, VmRuntimeKey, WasmEngine,
    WasmEngineConfig, WasmInstance,
};
pub use infra::{
    ensure_work_dir_structure, get_work_dir, init_logging, load_config, normalize_path,
    read_file_utf8, resolve_agent_dir, resolve_assets_dir, resolve_audit_dir, resolve_log_dir,
    resolve_memory_dir, resolve_plugins_dir, resolve_quickjs_path, resolve_sessions_dir,
    resolve_tmp_dir, resolve_workspace_dir, validate_config, wire, write_file_atomic, AgentConfig,
    AgentEvent, AppConfig, AppError, AuditEntry, AuditFilter, AuditPrimitiveOp, AuditRecorder,
    AuditStore, DefaultEventBus, EventBus, EventContext, EventListenerId, ExtensionEvent,
    FileAuditRecorder, HostcallAuditEntry, LlmConfig, LogConfig, PluginLifecycleAuditEntry,
    PrimitiveAuditEntry, PrimitiveConfig, SecurityConfig, ToolAuditEntry, TracingAuditRecorder,
    WasmConfig,
};
