//! # 宿主核心能力层
//!
//! 会话管理、LLM 接入、4 原语、工具注册、插件生命周期等核心引擎，仅在宿主层运行。

pub mod confirmation;
pub mod executor;
pub(crate) mod llm;
pub mod primitives;
pub mod session;
pub mod tools;

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
    load_store, save_store, SessionEntry, SessionHeader, SessionManager, SessionStore,
    TranscriptEntry, DEFAULT_SESSION_KEY,
};
pub use tools::{DefaultToolRegistry, Tool, ToolExecutor, ToolRegistry};
