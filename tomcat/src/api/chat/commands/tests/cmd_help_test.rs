//! Tests for `commands::cmd_help`.

use super::super::{parse_chat_command, ChatCommand};

fn assert_not_command(input: &str) {
    assert!(matches!(
        parse_chat_command(input),
        ChatCommand::NotACommand(_)
    ));
}

#[test]
fn help_command_is_lowercase_only() {
    assert_eq!(parse_chat_command("/help"), ChatCommand::Help);
    assert!(matches!(
        parse_chat_command("/help foo"),
        ChatCommand::UsageError { .. }
    ));
    assert_not_command("/HELP");
}
