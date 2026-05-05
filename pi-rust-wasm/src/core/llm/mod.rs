//! # LLM 统一接入模块
//!
//! 定义 [`LlmProvider`] trait 与协议无关的请求/响应/流事件类型。**Provider 实现文件**
//! （`openai.rs` / `openai_responses.rs` / 未来新增的 `xxx.rs`）由 [`registry`] 子模块
//! 通过 `#[cfg] #[path]` 内挂并提供 [`resolve_llm`]，本入口**不再**为每个 Provider 显式
//! `mod xxx;` 与 `pub use xxx::XxxProvider;`——上层一律通过 `resolve_llm` 拿
//! `Arc<dyn LlmProvider>`。
//!
//! 模型/会话相关的 token 统计在 [`token_usage`]，model_override 由请求层传入并与
//! `SessionEntry` 约定一致。本模块不直接依赖 `SessionEntry`，仅消费已解析的
//! [`ChatRequest`]。

mod provider;
mod registry;
pub mod system_prompt;
mod token_usage;
mod types;

pub use provider::LlmProvider;
pub use registry::{registered_provider_ids, resolve_llm};
pub use token_usage::SessionTokenUsage;
#[allow(unused_imports)]
pub use types::{
    ChatMessage, ChatMessageContent, ChatMessageContentPart, ChatMessageRole, ChatRequest,
    ChatResponse, ChatResponseChoice, MessageKind, StreamEvent, TokenUsage, FILE_MAX_BYTES,
    IMAGE_MAX_BYTES,
};

#[cfg(test)]
mod tests;
