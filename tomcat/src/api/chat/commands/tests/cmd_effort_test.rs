//! `/effort` 命令解析与持久化的焦小测。

use super::super::cmd_effort::{apply_level, parse_effort_level};
use super::super::{help_text, parse_chat_command, ChatCommand};
use crate::core::llm::ThinkingLevel;
use crate::ModelThinkingStore;

#[test]
fn effort_levels_parse_correctly() {
    assert_eq!(parse_effort_level("low"), Some(ThinkingLevel::Low));
    assert_eq!(parse_effort_level("medium"), Some(ThinkingLevel::Medium));
    assert_eq!(parse_effort_level("high"), Some(ThinkingLevel::High));
    assert_eq!(parse_effort_level("xhigh"), Some(ThinkingLevel::Xhigh));
}

#[test]
fn effort_command_parses_all_supported_levels() {
    assert_eq!(
        parse_chat_command("/effort low"),
        ChatCommand::Effort {
            level: ThinkingLevel::Low,
        }
    );
    assert_eq!(
        parse_chat_command("/effort medium"),
        ChatCommand::Effort {
            level: ThinkingLevel::Medium,
        }
    );
    assert_eq!(
        parse_chat_command("/effort high"),
        ChatCommand::Effort {
            level: ThinkingLevel::High,
        }
    );
    assert_eq!(
        parse_chat_command("/effort xhigh"),
        ChatCommand::Effort {
            level: ThinkingLevel::Xhigh,
        }
    );
}

#[test]
fn effort_without_arg_is_usage_error() {
    assert!(matches!(
        parse_chat_command("/effort"),
        ChatCommand::UsageError { .. }
    ));
}

#[test]
fn effort_unknown_arg_is_usage_error() {
    let cmd = parse_chat_command("/effort turbo");
    let msg = match cmd {
        ChatCommand::UsageError { message } => message,
        other => panic!("应为 UsageError，实际：{:?}", other),
    };
    assert!(
        msg.contains("low|medium|high|xhigh"),
        "错误文案应列出合法值：{msg}"
    );
    assert!(msg.contains("turbo"), "错误文案应回显非法输入：{msg}");
}

#[test]
fn apply_level_persists_model_override() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");
    let store = ModelThinkingStore::load(&path, ThinkingLevel::Medium).unwrap();

    apply_level(&store, "gpt-5.4", ThinkingLevel::Xhigh).unwrap();
    assert_eq!(store.get("gpt-5.4"), ThinkingLevel::Xhigh);

    let reloaded = ModelThinkingStore::load(&path, ThinkingLevel::Medium).unwrap();
    assert_eq!(reloaded.get("gpt-5.4"), ThinkingLevel::Xhigh);
}

#[test]
fn help_text_mentions_effort_command() {
    let h = help_text();
    assert!(h.contains("/effort"), "/help 应列出 /effort：{h}");
    assert!(
        h.contains("low|medium|high|xhigh"),
        "帮助文案应展示 /effort 支持档位：{h}"
    );
}
