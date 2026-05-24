//! # CLI 对话入口
//!
//! - `context.rs` 负责 `ChatContext` 装配与启动期依赖注入。
//! - `run_loop.rs` 负责对话主循环、单轮驱动与相关运行期 helper。

mod context;
mod run_loop;

#[cfg(test)]
mod tests;

pub mod cli_turn_renderer;
pub mod commands;
pub mod events;
pub mod panels;
pub mod permission;
pub mod plan_runtime;
pub mod preflight;

pub use context::{ChatContext, CliConfirmation};
pub use run_loop::{chat_loop, run_chat_turn};

#[cfg(test)]
pub(crate) use context::resolve_initial_show_thinking;
#[cfg(test)]
pub(crate) use run_loop::{
    build_turn_checkpoint_request, cleanup_openai_files_on_session_end, persist_turn_result,
    register_thinking_persist_listeners, schedule_checkpoint_prune,
    unregister_thinking_persist_listeners,
};
