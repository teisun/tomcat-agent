//! `/thinking` 命令解析与开关行为的焦小测。
//!
//! 覆盖：
//! - `parse_chat_command("/thinking")` 解析为 `Toggle`；
//! - `/thinking on/off/toggle` 各自走到正确的 `ThinkingAction`；
//! - `/thinking xxx` 报 UsageError；
//! - `apply_action` 在 AtomicBool 上的 on/off/toggle 行为；
//! - `/help` 文案包含 `/thinking`。

use std::sync::atomic::{AtomicBool, Ordering};

use super::super::cmd_thinking::{apply_action, ThinkingAction};
use super::super::{help_text, parse_chat_command, ChatCommand};

#[test]
fn bare_thinking_defaults_to_toggle() {
    assert!(matches!(
        parse_chat_command("/thinking"),
        ChatCommand::Thinking {
            action: ThinkingAction::Toggle
        }
    ));
}

#[test]
fn thinking_on_off_toggle_parse_correctly() {
    assert!(matches!(
        parse_chat_command("/thinking on"),
        ChatCommand::Thinking {
            action: ThinkingAction::On
        }
    ));
    assert!(matches!(
        parse_chat_command("/thinking off"),
        ChatCommand::Thinking {
            action: ThinkingAction::Off
        }
    ));
    assert!(matches!(
        parse_chat_command("/thinking toggle"),
        ChatCommand::Thinking {
            action: ThinkingAction::Toggle
        }
    ));
}

#[test]
fn thinking_unknown_subcommand_is_usage_error() {
    let cmd = parse_chat_command("/thinking foo");
    let msg = match cmd {
        ChatCommand::UsageError { message } => message,
        other => panic!("应为 UsageError，实际：{:?}", other),
    };
    assert!(msg.contains("on/off/toggle"), "错误文案应说明合法值：{}", msg);
    assert!(msg.contains("foo"), "错误文案应回显未知子命令：{}", msg);
}

#[test]
fn thinking_extra_args_is_usage_error() {
    assert!(matches!(
        parse_chat_command("/thinking on extra"),
        ChatCommand::UsageError { .. }
    ));
}

#[test]
fn apply_on_sets_true_regardless_of_prev() {
    let f = AtomicBool::new(false);
    assert_eq!(apply_action(&f, ThinkingAction::On), true);
    assert!(f.load(Ordering::Acquire));
    assert_eq!(apply_action(&f, ThinkingAction::On), true);
    assert!(f.load(Ordering::Acquire));
}

#[test]
fn apply_off_sets_false_regardless_of_prev() {
    let f = AtomicBool::new(true);
    assert_eq!(apply_action(&f, ThinkingAction::Off), false);
    assert!(!f.load(Ordering::Acquire));
}

#[test]
fn apply_toggle_flips_strictly() {
    let f = AtomicBool::new(false);
    assert_eq!(apply_action(&f, ThinkingAction::Toggle), true);
    assert!(f.load(Ordering::Acquire));
    assert_eq!(apply_action(&f, ThinkingAction::Toggle), false);
    assert!(!f.load(Ordering::Acquire));
}

#[test]
fn help_text_mentions_thinking_command() {
    let h = help_text();
    assert!(h.contains("/thinking"), "/help 应列出 /thinking：{}", h);
}
