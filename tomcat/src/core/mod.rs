//! # 宿主核心能力层
//!
//! 会话管理、LLM 接入、4 原语、工具注册、插件生命周期等核心引擎，仅在宿主层运行。

pub mod agent_loop;
pub mod agent_registry;
pub mod checkpoint;
pub mod compaction;
pub mod llm;
pub mod package;
pub mod permission;
pub mod plan_runtime;
pub mod prompts;
pub mod security;
pub mod session;
pub mod skill;
pub mod tools;

pub use agent_loop::{AgentLoop, AgentLoopConfig, AgentRunResult, ToolCallInfo};
pub use checkpoint::{
    compute_resume_plan, CheckpointDiff, CheckpointError, CheckpointId, CheckpointKind,
    CheckpointMeta, CheckpointRecordRequest, CheckpointRestoreReport, CheckpointStore, ListOptions,
    NoopStore, RestoreOptions, ResumePlan, RetentionPolicy, ShadowGitStore,
    SwitchingCheckpointStore,
};
pub use context_metrics::{ContextLiveMetrics, ContextMetrics};
pub use llm::system_prompt;
pub use llm::{
    build_provider, AuthStore, Capabilities, ChatMessage, ChatMessageContentPart, ChatRequest,
    ChatResponse, ChatResponseChoice, Credential, DefaultLlmResolver, LlmProvider, LlmResolver,
    LlmScene, ModelCatalog, ModelEntry, ResolvedCall, SessionTokenUsage, StreamEvent,
    FILE_MAX_BYTES, IMAGE_MAX_BYTES,
};
pub use package::{
    canonical_scope_root, load_package_registry, load_plugin_registry, resolve_layer_paths,
    resolve_runtime_layer_paths, save_package_registry, save_plugin_registry,
    DetectedPackageSource, DetectedPackageSourceKind, InstallOutcome, LayerPaths,
    PackageLayerListing, PackageManager, PackageManifest, PackagePluginRecord, PackageRecord,
    PackageRegistryFile, PackageResourceKind, PackageSkillRecord, PackageSourceKind,
    PackageVisibility, PluginRegistryEntry, PluginRegistryFile, PreparedInstall, UninstallOutcome,
    PACKAGE_MANIFEST_SCHEMA_V1, PACKAGE_REGISTRY_SCHEMA_V1,
};
pub use primitives::{
    BashResult, DirEntry, EditFileResult, EditOperation, EditOperationType, PrimitiveExecutor,
    PrimitiveOperation, ReadBinaryResult, ReadResult, ReadTextResult, SearchFileCount,
    SearchFileMatch, SearchFilesArgs, SearchFilesOutput, SearchFilesOutputMode, SearchFilesQuery,
    SearchFilesResultMode, SearchFilesStats, SearchFilesTarget, WriteFileResult,
};
pub use session::context_metrics;
pub use session::{
    build_context_from_state, compound_turn_id, fnv1a_hex, init_context_state, load_store,
    project_root, resolve_session_mode, save_store, session_key_for, session_key_for_agent,
    ApiUsage, BranchSummaryEntry, CompactionResult, ContextState, SessionEntry, SessionHeader,
    SessionManager, SessionMode, SessionStore, TranscriptEntry, DEFAULT_SESSION_KEY,
};
pub use tools::contract::confirmation;
pub use tools::contract::confirmation::{
    AllowAllConfirmation, ConfirmDecision, DenyAllConfirmation, UserConfirmationProvider,
};
pub use tools::contract::registry::{DefaultToolRegistry, Tool, ToolExecutor, ToolRegistry};
pub use tools::primitive as primitives;
pub use tools::primitive::DefaultPrimitiveExecutor;
