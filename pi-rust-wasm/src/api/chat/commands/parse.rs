//! # Chat command parsing and dispatch
//!
//! `pi chat` currently supports two local commands before a line is sent to the
//! LLM:
//!
//! - `/path <path>` asks for access to one path through the existing permission
//!   menu.
//! - `/help` prints the command list.
//!
//! Command names are intentionally case-sensitive and lowercase-only. Unknown
//! slash-prefixed lines remain ordinary chat input.

use std::path::PathBuf;

use crate::api::chat::ChatContext;

use super::{cmd_help, cmd_path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatCommand {
    /// Not a recognized local command; send the original line to the LLM.
    NotACommand(String),
    /// `/path <path>` with exactly one path argument.
    Path {
        path: PathBuf,
        original_line: String,
    },
    /// `/help`.
    Help,
    /// Recognized command with invalid arguments.
    UsageError { message: String },
}

pub(crate) enum ChatCommandOutcome {
    /// Send `line` to the LLM as the current user turn.
    Continue { line: String },
    /// Command was fully handled locally; skip the current turn.
    Handled,
}

pub fn parse_chat_command(line: &str) -> ChatCommand {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ChatCommand::NotACommand(line.to_string());
    }

    let first_token = trimmed.split_whitespace().next().unwrap_or_default();
    if first_token != "/path" && first_token != "/help" {
        return ChatCommand::NotACommand(line.to_string());
    }

    let tokens = match shell_words::split(trimmed) {
        Ok(tokens) => tokens,
        Err(e) => {
            return ChatCommand::UsageError {
                message: format!("命令参数解析失败：{}", e),
            };
        }
    };

    match first_token {
        "/path" => cmd_path::parse_args(tokens, trimmed),
        "/help" => cmd_help::parse_args(tokens),
        _ => ChatCommand::NotACommand(line.to_string()),
    }
}

pub(crate) fn dispatch_chat_command(
    ctx: &ChatContext,
    command: ChatCommand,
    rl: &mut rustyline::DefaultEditor,
) -> ChatCommandOutcome {
    match command {
        ChatCommand::NotACommand(line) => ChatCommandOutcome::Continue { line },
        ChatCommand::Help => cmd_help::run(),
        ChatCommand::UsageError { message } => {
            println!("{}\n\n{}", message, cmd_help::help_text());
            ChatCommandOutcome::Handled
        }
        ChatCommand::Path {
            path,
            original_line,
        } => cmd_path::run(ctx, path, original_line, rl),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_not_command(input: &str) {
        assert!(matches!(
            parse_chat_command(input),
            ChatCommand::NotACommand(_)
        ));
    }

    #[test]
    fn unknown_slash_commands_are_chat() {
        assert_not_command("/foo /a");
        assert_not_command("/abs/path");
    }

    #[test]
    fn normal_text_with_path_is_chat() {
        assert_not_command("帮我看下 /a");
    }
}
