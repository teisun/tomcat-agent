//! # CLI 对话入口
//!
//! - `context.rs` 负责 `ChatContext` 装配与启动期依赖注入。
//! - `run_loop/` 负责对话主循环、单轮驱动与相关运行期 helper。

mod context;
mod prompt;
mod run_loop;
mod session_runtime;

#[cfg(test)]
mod tests;

pub mod cli_turn_renderer;
pub mod commands;
pub mod events;
pub mod panels;
pub mod permission;
pub mod preflight;

pub use context::{ChatContext, ChatContextOverrides, CliConfirmation};
pub(crate) use run_loop::{build_system_text, sync_context_state_system_prompt_len};
pub use run_loop::{chat_loop, run_chat_turn};
pub use session_runtime::{GlobalServices, ScopeServices, SessionRuntime, SessionRuntimeRegistry};

#[cfg(test)]
pub(crate) use context::resolve_initial_thinking_display;
#[cfg(test)]
pub(crate) use run_loop::{
    build_turn_checkpoint_request, checkpoint_warn_line, cleanup_openai_files_on_session_end,
    is_append_message_chain_invariant, is_fatal_error, persist_turn_result,
    register_thinking_persist_listeners, schedule_checkpoint_prune,
    try_rehydrate_context_state_after_append_invariant, unregister_thinking_persist_listeners,
};
