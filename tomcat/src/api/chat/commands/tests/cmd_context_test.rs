//! `/context` 命令解析与持久化的焦小测。

use super::super::cmd_context::{apply_context_window, parse_context_window};
use super::super::{help_text, parse_chat_command, ChatCommand};
use crate::core::llm::ThinkingLevel;
use crate::ModelPrefsStore;

#[test]
fn context_window_parser_accepts_positive_u32_only() {
    assert_eq!(parse_context_window("400000"), Some(400_000));
    assert_eq!(parse_context_window("0"), None);
    assert_eq!(parse_context_window("-1"), None);
    assert_eq!(parse_context_window("not-a-number"), None);
}

#[test]
fn context_command_parses_supported_shape() {
    assert_eq!(
        parse_chat_command("/context 1000000"),
        ChatCommand::Context {
            context_window: 1_000_000,
        }
    );
    assert!(matches!(
        parse_chat_command("/context"),
        ChatCommand::UsageError { .. }
    ));
    assert!(matches!(
        parse_chat_command("/context 0"),
        ChatCommand::UsageError { .. }
    ));
}

#[test]
fn apply_context_window_persists_model_override() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");
    let store = ModelPrefsStore::load(&path, ThinkingLevel::Medium).unwrap();

    apply_context_window(&store, "gpt-5.6", 1_000_000).unwrap();
    assert_eq!(store.context_window_for("gpt-5.6"), Some(1_000_000));

    let reloaded = ModelPrefsStore::load(&path, ThinkingLevel::Medium).unwrap();
    assert_eq!(reloaded.context_window_for("gpt-5.6"), Some(1_000_000));
}

#[test]
fn help_text_mentions_context_command() {
    assert!(help_text().contains("/context"));
}
