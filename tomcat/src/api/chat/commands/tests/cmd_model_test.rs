use super::super::{parse_chat_command, ChatCommand, ModelCommand};
use crate::api::chat::commands::help_text;

#[test]
fn model_command_parses_current_list_and_use() {
    assert_eq!(
        parse_chat_command("/model"),
        ChatCommand::Model(ModelCommand::Current)
    );
    assert_eq!(
        parse_chat_command("/model current"),
        ChatCommand::Model(ModelCommand::Current)
    );
    assert_eq!(
        parse_chat_command("/model list"),
        ChatCommand::Model(ModelCommand::List)
    );
    assert_eq!(
        parse_chat_command("/model use deepseek-reasoner"),
        ChatCommand::Model(ModelCommand::Use {
            model_id: "deepseek-reasoner".to_string(),
        })
    );
}

#[test]
fn model_command_invalid_args_return_usage_error() {
    assert!(matches!(
        parse_chat_command("/model use"),
        ChatCommand::UsageError { .. }
    ));
    assert!(matches!(
        parse_chat_command("/model foo"),
        ChatCommand::UsageError { .. }
    ));
}

#[test]
fn help_text_mentions_model_command() {
    let h = help_text();
    assert!(h.contains("/model [current|list|use <id>]"));
}
