//! Tests for `commands::parse` — slash-command recognition contract.

use super::super::{parse_chat_command, ChatCommand};

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
