//! Local commands handled by `pi chat` before a line is sent to the LLM.

mod cmd_help;
mod cmd_path;
mod parse;

pub use cmd_path::render_path_menu;
pub(super) use parse::{dispatch_chat_command, ChatCommandOutcome};
pub use parse::{parse_chat_command, ChatCommand};
