//! Tests for `commands::cmd_help`.

use super::super::{parse_chat_command, ChatCommand};
use crate::api::chat::commands::help_text;

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

#[test]
fn help_text_mentions_checkpoint_commands() {
    let h = help_text();
    assert!(h.contains("/ckpt"), "/help 应列出 /ckpt：{}", h);
    assert!(h.contains("/restore"), "/help 应列出 /restore：{}", h);
}

#[test]
fn help_text_mentions_plan_without_goal_argument() {
    let h = help_text();
    assert!(h.contains("\n  /plan                      "), "/help 应列出无参 /plan：{}", h);
    assert!(
        !h.contains("/plan \"<目标>\""),
        "/help 不应再暴露旧的 /plan 目标参数用法：{}",
        h
    );
}
