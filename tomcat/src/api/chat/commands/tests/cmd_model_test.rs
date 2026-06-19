use super::super::{
    cmd_model::format_model_list_line, parse_chat_command, ChatCommand, ModelCommand,
};
use crate::api::chat::commands::help_text;
use crate::core::llm::{Capabilities, ModelEntry};

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
        parse_chat_command("/model use deepseek-v4-pro"),
        ChatCommand::Model(ModelCommand::Use {
            model_id: "deepseek-v4-pro".to_string(),
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

#[test]
fn format_model_list_line_uses_local_id_not_upstream_model_name() {
    let entry = ModelEntry {
        id: "gpt-5.4_litellm-sunmi".to_string(),
        model_name: Some("gpt-5.4".to_string()),
        api: "openai-responses".to_string(),
        provider: "litellm-sunmi".to_string(),
        api_key_env: Some("LITELLM_SUNMI_API_KEY".to_string()),
        base_url: Some("https://aigateway.sunmi.com".to_string()),
        capabilities: Capabilities {
            vision: true,
            files: true,
            tools: true,
            reasoning: true,
            web_search: false,
        },
        context_window: None,
        cost: None,
        thinking_format: Some("openai".to_string()),
    };

    let line = format_model_list_line(&entry, true, false);
    assert!(line.contains("gpt-5.4_litellm-sunmi [current]"));
    assert!(!line.contains("  - gpt-5.4 [current]"));
}
