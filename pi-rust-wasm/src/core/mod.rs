//! 宿主核心能力层：会话管理、LLM 接入、4 原语、工具注册等。
//! 本层仅依赖 infra，不依赖 ext/api。

pub mod session;

pub use session::{
    load_store, save_store, SessionEntry, SessionHeader, SessionManager, SessionStore,
    TranscriptEntry, DEFAULT_SESSION_KEY,
};
