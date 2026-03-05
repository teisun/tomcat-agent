//! # pi_awsm 库
//!
//! 基础设施层与核心能力层（含 LLM 接入），供 session_cli / wasm_plugin / chat 等模块依赖。
//! 对外 API 通过 `infra` 与 `core` 统一暴露，符合编码与分层架构规范。

pub mod api;
pub mod core;
pub mod infra;

pub use core::{
    ChatMessage, ChatRequest, ChatResponse, LlmProvider, OpenAiProvider, SessionTokenUsage,
    StreamEvent,
};
pub use infra::{
    init_logging, load_config, normalize_path, read_file_utf8, validate_config, write_file_atomic,
    AgentEvent, AppConfig, AppError, DefaultEventBus, EventBus, EventContext, EventListenerId,
    ExtensionEvent, LlmConfig, LogConfig, PrimitiveConfig, SecurityConfig,
};
pub use api::run_cli;
pub use core::{
    load_store, save_store, SessionEntry, SessionHeader, SessionManager, SessionStore,
    TranscriptEntry, DEFAULT_SESSION_KEY,
};
