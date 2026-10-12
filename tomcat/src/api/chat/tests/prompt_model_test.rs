use crate::api::chat::prompt::{user_prompt_for_mode, user_prompt_for_mode_with_model};
use crate::core::session::AgentMode;

#[test]
fn prompt_shows_current_model_in_chat_mode() {
    let prompt = user_prompt_for_mode_with_model(AgentMode::Chat, None, "gpt-5.4");
    assert_eq!(prompt, "u[Chat|gpt-5.4]> ");
}

#[test]
fn prompt_without_model_falls_back_to_original_format() {
    let base = user_prompt_for_mode(AgentMode::Plan, None);
    let prompt = user_prompt_for_mode_with_model(AgentMode::Plan, None, "  ");
    assert_eq!(prompt, base);
}

#[test]
fn prompt_updates_model_label_without_changing_chat_prompt_shape() {
    let before = user_prompt_for_mode_with_model(AgentMode::Chat, None, "gpt-5.4");
    let after = user_prompt_for_mode_with_model(AgentMode::Chat, None, "gpt-5.2");
    assert_eq!(before, "u[Chat|gpt-5.4]> ");
    assert_eq!(after, "u[Chat|gpt-5.2]> ");
    assert_ne!(before, after);
}
