//! CLI 子命令实现：init、doctor、config、session、plugin、audit、chat 对话模式。

pub mod chat;
pub mod cli;
pub mod render;

pub use cli::run_cli;
