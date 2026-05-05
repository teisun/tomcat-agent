//! # 宿主核心能力层
//!
//! 会话管理、LLM 接入、4 原语、工具注册、插件生命周期等核心引擎，仅在宿主层运行。

pub mod agent_loop;
pub mod compaction;
pub mod llm;
pub mod permission;
pub mod session;
pub mod tools;

pub use agent_loop::{AgentLoop, AgentLoopConfig, AgentRunResult, ToolCallInfo};
pub use confirmation::{
    AllowAllConfirmation, ConfirmDecision, DenyAllConfirmation, UserConfirmationProvider,
};
pub use context_metrics::{ContextLiveMetrics, ContextMetrics};
pub use llm::system_prompt;
pub use llm::{
    ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice, LlmProvider, OpenAiProvider,
    OpenAiResponsesProvider, SessionTokenUsage, StreamEvent,
};
pub use primitives::{
    BashResult, DirEntry, EditFileResult, EditOperation, EditOperationType, PrimitiveExecutor,
    PrimitiveOperation, SearchFileCount, SearchFileMatch, SearchFilesArgs, SearchFilesOutput,
    SearchFilesOutputMode, SearchFilesQuery, SearchFilesResultMode, SearchFilesStats,
    SearchFilesTarget, WriteFileResult,
};
pub use session::context_metrics;
pub use session::{
    build_context_from_state, compound_turn_id, init_context_state, load_store, save_store,
    ApiUsage, BranchSummaryEntry, CompactionResult, ContextState, SessionEntry, SessionHeader,
    SessionManager, SessionStore, TranscriptEntry, DEFAULT_SESSION_KEY,
};
pub use tools::primitive as primitives;
pub use tools::primitive::confirmation;
pub use tools::primitive::DefaultPrimitiveExecutor;
pub use tools::{DefaultToolRegistry, Tool, ToolExecutor, ToolRegistry};
