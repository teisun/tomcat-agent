//! # 宿主核心能力层 (core)
//!
//! 定义会话管理、LLM、4 原语、工具注册等 Trait，供宿主 API 层与插件生命周期使用。
//! 具体实现由各任务（003、004、005、006）落地。

pub mod llm;
pub mod primitives;
pub mod tools;

pub use llm::{ChatMessage, ChatRequest, ChatResponse, LlmProvider, StreamEvent};
pub use primitives::{
    BashResult, DirEntry, EditFileResult, EditOperation, EditOperationType, PrimitiveExecutor,
    PrimitiveOperation, WriteFileResult,
};
pub use tools::{Tool, ToolRegistry};
