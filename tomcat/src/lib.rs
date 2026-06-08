//! # tomcat 库
//!
//! 基础设施层与核心能力层（含 LLM、会话、4 原语、工具、插件），供 session_cli / wasm_plugin / chat 等模块依赖。
//! 对外 API 通过 `infra`、`core`、`ext` 统一暴露，符合编码与分层架构规范。

pub mod api;
pub mod core;
pub mod ext;
pub mod infra;
#[cfg(test)]
pub(crate) mod test_support;

pub use api::chat::{chat_loop, run_chat_turn, ChatContext};
pub use api::run_cli;
pub use core::agent_loop::AgentRunOutcome;
pub use core::{
    build_context_from_state, compound_turn_id, init_context_state, load_store, save_store,
    BranchSummaryEntry, CompactionResult, ContextState, SessionEntry, SessionHeader,
    SessionManager, SessionStore, TranscriptEntry, DEFAULT_SESSION_KEY,
};
pub use core::{
    resolve_llm, AgentLoop, AgentLoopConfig, AgentRunResult, AllowAllConfirmation, AuthStore,
    BashResult, Capabilities, ChatMessage, ChatMessageContentPart, ChatRequest, ChatResponse,
    ChatResponseChoice, CheckpointDiff, CheckpointError, CheckpointId, CheckpointKind,
    CheckpointMeta, CheckpointRecordRequest, CheckpointRestoreReport, CheckpointStore, Credential,
    DefaultLlmResolver, DefaultPrimitiveExecutor, DefaultToolRegistry, DenyAllConfirmation,
    DirEntry, EditFileResult, EditOperation, EditOperationType, ListOptions, LlmProvider,
    LlmResolver, LlmScene, ModelCatalog, ModelEntry, NoopStore, PrimitiveExecutor,
    PrimitiveOperation, ReadBinaryResult, ReadResult, ReadTextResult, ResolvedCall, RestoreOptions,
    ResumePlan, RetentionPolicy, SearchFileCount, SearchFileMatch, SearchFilesArgs,
    SearchFilesOutput, SearchFilesOutputMode, SearchFilesQuery, SearchFilesResultMode,
    SearchFilesStats, SearchFilesTarget, SessionTokenUsage, ShadowGitStore, StreamEvent,
    SwitchingCheckpointStore, Tool, ToolCallInfo, ToolExecutor, ToolRegistry,
    UserConfirmationProvider, WriteFileResult, FILE_MAX_BYTES, IMAGE_MAX_BYTES,
};
pub use ext::{
    invoke_host_func, invoke_host_func_with, parse_manifest, transpile_pi_plugin_for_quickjs,
    transpile_typescript, EventEnvelope, HostApiDispatcher, HostRequest, HostResponse, PluginInfo,
    PluginInstance, PluginManager, PluginManifest, PluginStatus, RuntimeManager,
    SharedRuntimeManager, VmActorHandle, VmActorState, VmCommand, VmRuntimeKey, WasmEngine,
    WasmEngineConfig, WasmInstance,
};
pub use infra::{
    ensure_embedded_assets, ensure_work_dir_structure, get_work_dir, init_logging, llm_error,
    llm_http_status_error, load_config, load_config_toml_file, normalize_path, read_file_utf8,
    resolve_agent_definition_dir, resolve_agent_dir, resolve_agent_trail_dir, resolve_assets_dir,
    resolve_audit_dir, resolve_checkpoints_dir, resolve_dot_tomcat_temp_dir, resolve_log_dir,
    resolve_memory_dir, resolve_plugins_dir, resolve_quickjs_path, resolve_sessions_dir,
    resolve_tmp_dir, resolve_workspace_dir, resolve_workspace_roots_paths, validate_config, wire,
    write_file_atomic, AgentConfig, AgentEvent, AppConfig, AppError, AuditEntry, AuditFilter,
    AuditPrimitiveOp, AuditRecorder, AuditStore, CheckpointConfig, ContextConfig, DefaultEventBus,
    EventBus, EventContext, EventListenerId, ExtensionEvent, FileAuditRecorder, HostcallAuditEntry,
    LlmConfig, LlmError, LlmErrorStage, LogConfig, PluginLifecycleAuditEntry, PreflightConfig,
    PrimitiveAuditEntry, PrimitiveConfig, ResumeHydrationMode, SecurityConfig, ToolAuditEntry,
    TracingAuditRecorder, WasmConfig, WorkspaceConfig, BRAND_ID, CLI_NAME, DEFAULT_CONFIG_FILENAME,
    DEFAULT_CONFIG_PATH, DEFAULT_LLM_MODEL, DEFAULT_WORK_DIR, ENV_PREFIX, INTERNAL_STABLE_ID,
    PRODUCT_NAME, QUICKJS_MODULES_PATH_ENV,
};
