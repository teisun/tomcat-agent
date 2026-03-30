//! # 宿主核心能力层
//!
//! 会话管理、LLM 接入、4 原语、工具注册、插件生命周期等核心引擎，仅在宿主层运行。

pub mod agent_loop;
pub mod compaction;
pub mod confirmation;
pub mod executor;
pub(crate) mod llm;
pub mod primitives;
pub mod session;
pub mod system_prompt;
pub mod tools;

pub use agent_loop::{
    agent_messages_from_chat, convert_to_llm_format, AgentLoop, AgentLoopConfig, AgentMessage,
    AgentRunResult, ToolCallInfo,
};
pub use confirmation::{AllowAllConfirmation, DenyAllConfirmation, UserConfirmationProvider};
pub use executor::DefaultPrimitiveExecutor;
pub use llm::{
    ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice, LlmProvider, OpenAiProvider,
    SessionTokenUsage, StreamEvent,
};
pub use primitives::{
    BashResult, DirEntry, EditFileResult, EditOperation, EditOperationType, PrimitiveExecutor,
    PrimitiveOperation, WriteFileResult,
};
pub use session::{
    build_context_from_state, init_context_state, load_store, save_store, CompactionEntry,
    ContextState, SessionEntry, SessionHeader, SessionManager, SessionStore, TranscriptEntry,
    TurnEntry, DEFAULT_SESSION_KEY,
};
pub use tools::{DefaultToolRegistry, Tool, ToolExecutor, ToolRegistry};
