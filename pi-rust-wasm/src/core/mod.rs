//! # 宿主核心能力层
//!
//! 会话管理、LLM 接入、4 原语、工具注册、插件生命周期等核心引擎，仅在宿主层运行。

pub(crate) mod llm;
pub mod session;

pub use llm::{
    ChatMessage, ChatRequest, ChatResponse, LlmProvider, OpenAiProvider, SessionTokenUsage,
    StreamEvent,
};
pub use session::{
    load_store, save_store, SessionEntry, SessionHeader, SessionManager, SessionStore,
    TranscriptEntry, DEFAULT_SESSION_KEY,
};
