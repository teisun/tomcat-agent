use crate::api::chat::prompt::{user_prompt_for_mode, user_prompt_for_mode_with_model};
use crate::core::plan_runtime::state::PlanState;

#[test]
fn prompt_shows_current_model_in_chat_mode() {
    let prompt = user_prompt_for_mode_with_model(&PlanState::Chat, "gpt-5.4");
    assert_eq!(prompt, "u[Chat|gpt-5.4]> ");
}

#[test]
fn prompt_without_model_falls_back_to_original_format() {
    let base = user_prompt_for_mode(&PlanState::Planning);
    let prompt = user_prompt_for_mode_with_model(&PlanState::Planning, "  ");
    assert_eq!(prompt, base);
}
