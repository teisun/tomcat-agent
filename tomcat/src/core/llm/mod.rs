//! # LLM 统一接入模块
//!
//! 定义 [`LlmProvider`] trait 与协议无关的请求/响应/流事件类型。**Provider 实现文件**
//! （`openai.rs` / `openai_responses.rs` / 未来新增的 `xxx.rs`）由 [`registry`] 子模块
//! 通过 `#[cfg] #[path]` 内挂并提供 [`build_provider`]，本入口**不再**为每个 Provider 显式
//! `mod xxx;` 与 `pub use xxx::XxxProvider;`——上层一律通过 registry / resolver 产出的
//! `Arc<dyn LlmProvider>` 使用具体实现。
//!
//! 模型/会话相关的 token 统计在 [`token_usage`]，model_override 由请求层传入并与
//! `SessionEntry` 约定一致。本模块不直接依赖 `SessionEntry`，仅消费已解析的
//! [`ChatRequest`]。

pub mod auth;
pub mod catalog;
pub(crate) mod http_client;
pub mod openai_files;
mod provider;
mod registry;
pub mod replay_policy;
pub mod resolver;
pub(crate) mod retry_delay;
pub mod system_prompt;
pub mod thinking_policy;
mod token_usage;
mod types;

pub use auth::{env_name_for_provider, missing_key_message, AuthStore, Credential};
pub use catalog::{Capabilities, Cost, ModelCatalog, ModelEntry};
pub use provider::LlmProvider;
pub use registry::{build_provider, registered_provider_ids};
#[allow(unused_imports)]
pub use replay_policy::{
    apply_text_downgrade, model_family, plan as plan_replay, CaptureMode, DowngradeMode,
    ProviderCompatProfile, ReplayAcceptance, ReplayAction,
};
pub use resolver::{DefaultLlmResolver, LlmResolver, LlmScene, ResolvedCall};
pub use token_usage::SessionTokenUsage;
pub use thinking_policy::ThinkingLevel;
#[allow(unused_imports)]
pub use types::{
    ChatMessage, ChatMessageContent, ChatMessageContentPart, ChatMessageRole, ChatRequest,
    ChatResponse, ChatResponseChoice, ContinuityMetadata, MessageKind, ProviderRefs,
    ReasoningContinuation, ReasoningFormat, ReplayRequirement, StreamEvent, ThinkingSource,
    TokenUsage, FILE_MAX_BYTES, IMAGE_MAX_BYTES,
};

#[cfg(test)]
mod tests;
