//! `/help` command implementation.

use super::parse::{ChatCommand, ChatCommandOutcome};
use crate::infra::i18n::tr;

pub(crate) fn parse_args(tokens: Vec<String>) -> ChatCommand {
    match tokens.as_slice() {
        [_cmd] => ChatCommand::Help,
        [_cmd, ..] => ChatCommand::UsageError {
            message: tr("slash.helpUsage", &[]),
        },
        _ => ChatCommand::Help,
    }
}

pub(crate) fn run() -> ChatCommandOutcome {
    println!("{}", help_text());
    ChatCommandOutcome::Handled
}

pub(crate) fn help_text() -> String {
    let mut text = tr("slash.help", &[]);
    for command in super::shared::shared_slash_commands() {
        text.push_str(&format!("\n  {}  {}", command.usage, command.summary));
    }
    text
}
