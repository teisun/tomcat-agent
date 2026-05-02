//! # `core::llm::tests` 测试目录
//!
//! 与 `core::agent_loop` 一致：整个 `core::llm` 模块只有**一个** `tests/` 目录，
//! 历史上挂在 `openai/`、`token_usage/`、`types/` 三处子模块下的 `tests/`
//! 已上抬合并到此处，按主题拆分为多个文件。
//!
//! ## 文件分组
//!
//! - `mocks`：跨用例共享的 fixture（如 OpenAI .env 加载）。
//! - `openai_provider_test`：`OpenAiProvider::new` / `count_tokens` / `is_retriable`
//!   等不依赖网络的可执行行为；以及 `chat_real_request_response_print`
//!   这种依赖真实 OPENAI_API_KEY 的 `#[ignore]` 用例。
//! - `openai_stream_test`：SSE 流解析与 `OpenAiStreamChunk → StreamEvent` 的转换。
//! - `system_prompt_test`：system prompt builder / workspace state section 的渲染。
//! - `token_usage_test`：`SessionTokenUsage::add` 累加正确性。
//! - `types_test`：`ChatMessage` / `ChatRequest` / `StreamEvent` 的构造与序列化。

mod mocks;
mod openai_provider_test;
mod openai_stream_test;
mod system_prompt_test;
mod token_usage_test;
mod types_test;
