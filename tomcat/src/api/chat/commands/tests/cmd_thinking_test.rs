//! `/thinking` 命令解析与开关行为的焦小测。
//!
//! 覆盖：
//! - `parse_chat_command("/thinking")` 解析为 `Toggle`；
//! - `/thinking minimal/summary/full/toggle` 各自走到正确的 `ThinkingAction`；
//! - 历史 `on/off` 作为兼容别名；
//! - `/thinking xxx` 报 UsageError；
//! - `apply_action` 在 `AtomicU8` 上的三档设置与循环切换；
//! - `/help` 文案包含 `/thinking`。

use std::sync::atomic::{AtomicU8, Ordering};

use super::super::cmd_thinking::{apply_action, ThinkingAction};
use super::super::{help_text, parse_chat_command, ChatCommand};
use crate::infra::config::ThinkingDisplay;

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
fn thinking_modes_and_aliases_parse_correctly() {
    assert!(matches!(
        parse_chat_command("/thinking minimal"),
        ChatCommand::Thinking {
            action: ThinkingAction::Minimal
        }
    ));
    assert!(matches!(
        parse_chat_command("/thinking summary"),
        ChatCommand::Thinking {
            action: ThinkingAction::Summary
        }
    ));
    assert!(matches!(
        parse_chat_command("/thinking full"),
        ChatCommand::Thinking {
            action: ThinkingAction::Full
        }
    ));
    assert!(matches!(
        parse_chat_command("/thinking toggle"),
        ChatCommand::Thinking {
            action: ThinkingAction::Toggle
        }
    ));
    assert!(matches!(
        parse_chat_command("/thinking on"),
        ChatCommand::Thinking {
            action: ThinkingAction::Full
        }
    ));
    assert!(matches!(
        parse_chat_command("/thinking off"),
        ChatCommand::Thinking {
            action: ThinkingAction::Summary
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
    assert!(
        msg.contains("minimal/summary/full/toggle"),
        "错误文案应说明合法值：{}",
        msg
    );
    assert!(msg.contains("foo"), "错误文案应回显未知子命令：{}", msg);
}

#[test]
fn thinking_extra_args_is_usage_error() {
    assert!(matches!(
        parse_chat_command("/thinking summary extra"),
        ChatCommand::UsageError { .. }
    ));
}

#[test]
fn apply_explicit_mode_sets_target_regardless_of_prev() {
    let f = AtomicU8::new(ThinkingDisplay::Minimal.as_u8());
    assert_eq!(
        apply_action(&f, ThinkingAction::Summary),
        ThinkingDisplay::Summary
    );
    assert_eq!(
        ThinkingDisplay::from_u8(f.load(Ordering::Acquire)),
        ThinkingDisplay::Summary
    );
    assert_eq!(
        apply_action(&f, ThinkingAction::Full),
        ThinkingDisplay::Full
    );
    assert_eq!(
        ThinkingDisplay::from_u8(f.load(Ordering::Acquire)),
        ThinkingDisplay::Full
    );
    assert_eq!(
        apply_action(&f, ThinkingAction::Minimal),
        ThinkingDisplay::Minimal
    );
    assert_eq!(
        ThinkingDisplay::from_u8(f.load(Ordering::Acquire)),
        ThinkingDisplay::Minimal
    );
}

#[test]
fn apply_toggle_cycles_summary_full_minimal() {
    let f = AtomicU8::new(ThinkingDisplay::Summary.as_u8());
    assert_eq!(
        apply_action(&f, ThinkingAction::Toggle),
        ThinkingDisplay::Full
    );
    assert_eq!(
        ThinkingDisplay::from_u8(f.load(Ordering::Acquire)),
        ThinkingDisplay::Full
    );
    assert_eq!(
        apply_action(&f, ThinkingAction::Toggle),
        ThinkingDisplay::Minimal
    );
    assert_eq!(
        ThinkingDisplay::from_u8(f.load(Ordering::Acquire)),
        ThinkingDisplay::Minimal
    );
    assert_eq!(
        apply_action(&f, ThinkingAction::Toggle),
        ThinkingDisplay::Summary
    );
    assert_eq!(
        ThinkingDisplay::from_u8(f.load(Ordering::Acquire)),
        ThinkingDisplay::Summary
    );
}

#[test]
fn help_text_mentions_thinking_command() {
    let h = help_text();
    assert!(h.contains("/thinking"), "/help 应列出 /thinking：{}", h);
    assert!(
        h.contains("minimal|summary|full|toggle"),
        "帮助文案应展示三档：{}",
        h
    );
}
