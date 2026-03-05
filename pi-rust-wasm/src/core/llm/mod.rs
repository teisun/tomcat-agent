//! # LLM 统一接入模块
//!
//! 定义 LlmProvider Trait、OpenAI 格式适配器、流式/非流式调用、限流与指数退避重试、
//! Token 统计、会话级模型配置（model_override 由请求层传入，与 SessionEntry 约定一致）。
//! 本模块不直接依赖 SessionEntry，仅消费已解析的 ChatRequest。

mod openai;
mod provider;
mod token_usage;
mod types;

pub use openai::OpenAiProvider;
pub use provider::LlmProvider;
pub use token_usage::SessionTokenUsage;
#[allow(unused_imports)]
pub use types::{
    ChatMessage, ChatMessageContent, ChatMessageRole, ChatRequest, ChatResponse,
    ChatResponseChoice, StreamEvent, TokenUsage,
};
