use super::super::tool_result_media::{tool_result_media_mode, ToolResultMediaMode};

#[test]
fn tool_result_media_defaults_follow_verified_api_contracts() {
    assert_eq!(
        tool_result_media_mode("anthropic-messages"),
        ToolResultMediaMode::Native
    );
    assert_eq!(
        tool_result_media_mode("openai-responses"),
        ToolResultMediaMode::Native
    );
    assert_eq!(tool_result_media_mode("openai"), ToolResultMediaMode::Split);
    assert_eq!(
        tool_result_media_mode("unknown"),
        ToolResultMediaMode::Split
    );
}
