//! `core::llm::replay_policy` 焦小测。

use crate::core::llm::replay_policy::{
    apply_text_downgrade, model_family, plan, ProviderCompatProfile, ReplayAction,
};
use crate::core::llm::types::{
    ChatMessage, ContinuityMetadata, ReasoningContinuation, ReasoningFormat, ReplayRequirement,
};

fn openai_reasoning_message() -> ChatMessage {
    ChatMessage::assistant("answer").with_reasoning_state(
        Some("safe summary".to_string()),
        Some(ReasoningContinuation {
            source_provider: "openai".to_string(),
            source_api: "responses".to_string(),
            source_model: "gpt-5".to_string(),
            format: ReasoningFormat::OpenaiResponsesReasoningItems,
            opaque_payload: serde_json::json!([{
                "type": "reasoning",
                "encrypted_content": "enc_123"
            }]),
            fallback_text: Some("safe summary".to_string()),
            provider_refs: None,
        }),
        Some(ContinuityMetadata {
            had_tool_call: false,
            replay_requirement: ReplayRequirement::SameProfileOptional,
        }),
    )
}

#[test]
fn replay_policy_openai_responses_keeps_encrypted_reasoning() {
    let msg = openai_reasoning_message();
    let profile = ProviderCompatProfile::openai_responses("gpt-5");
    assert_eq!(plan(&profile, &msg), ReplayAction::KeepOpaque);
}

#[test]
fn replay_policy_deepseek_v4_tool_turn_requires_reasoning_content() {
    let msg = ChatMessage::assistant_with_tool_calls(
        None,
        vec![serde_json::json!({
            "id":"call_1",
            "type":"function",
            "function":{"name":"read","arguments":"{}"}
        })],
    )
    .with_reasoning_state(
        Some("tool summary".to_string()),
        Some(ReasoningContinuation {
            source_provider: "deepseek".to_string(),
            source_api: "chat_completions".to_string(),
            source_model: "deepseek-v4-pro".to_string(),
            format: ReasoningFormat::DeepseekReasoningContent,
            opaque_payload: serde_json::json!({"reasoning_content":"internal"}),
            fallback_text: Some("tool summary".to_string()),
            provider_refs: None,
        }),
        Some(ContinuityMetadata {
            had_tool_call: true,
            replay_requirement: ReplayRequirement::SameProfileOptional,
        }),
    );
    let profile = ProviderCompatProfile::chat_completions("deepseek-v4-flash");
    assert_eq!(plan(&profile, &msg), ReplayAction::KeepOpaque);
}

#[test]
fn replay_policy_deepseek_v4_non_tool_turn_strips_reasoning_content() {
    let msg = ChatMessage::assistant("answer").with_reasoning_state(
        Some("safe summary".to_string()),
        Some(ReasoningContinuation {
            source_provider: "deepseek".to_string(),
            source_api: "chat_completions".to_string(),
            source_model: "deepseek-v4-pro".to_string(),
            format: ReasoningFormat::DeepseekReasoningContent,
            opaque_payload: serde_json::json!({"reasoning_content":"internal"}),
            fallback_text: Some("safe summary".to_string()),
            provider_refs: None,
        }),
        Some(ContinuityMetadata {
            had_tool_call: false,
            replay_requirement: ReplayRequirement::SameProfileRequired,
        }),
    );
    let profile = ProviderCompatProfile::chat_completions("deepseek-v4-flash");
    assert_eq!(plan(&profile, &msg), ReplayAction::StripOpaque);
}

#[test]
fn cross_provider_downgrade_prefers_fallback_text() {
    let msg = ChatMessage::assistant("answer").with_reasoning_state(
        Some("safe summary".to_string()),
        Some(ReasoningContinuation {
            source_provider: "deepseek".to_string(),
            source_api: "chat_completions".to_string(),
            source_model: "deepseek-v4-pro".to_string(),
            format: ReasoningFormat::DeepseekReasoningContent,
            opaque_payload: serde_json::json!({"reasoning_content":"internal"}),
            fallback_text: Some("safe summary".to_string()),
            provider_refs: None,
        }),
        Some(ContinuityMetadata {
            had_tool_call: true,
            replay_requirement: ReplayRequirement::SameProfileRequired,
        }),
    );
    let target = ProviderCompatProfile::openai_responses("gpt-5");
    assert_eq!(
        plan(&target, &msg),
        ReplayAction::ConvertToText("safe summary".to_string())
    );
}

#[test]
fn apply_text_downgrade_appends_safe_continuity_text() {
    let msg = ChatMessage::assistant("visible answer");
    let downgraded = apply_text_downgrade(&msg, "safe summary");
    assert_eq!(
        downgraded.text_content(),
        Some("visible answer\n\n[reasoning continuity]\nsafe summary")
    );
    assert!(downgraded.reasoning_continuation.is_none());
}

#[test]
fn model_family_normalizes_known_models() {
    assert_eq!(model_family("deepseek-v4-pro"), "deepseek-v4");
    assert_eq!(model_family("deepseek-v4-flash"), "deepseek-v4");
    assert_eq!(model_family("deepseek-v3"), "deepseek-v3");
    assert_eq!(model_family("gpt-5-mini"), "gpt-5");
}

#[test]
fn replay_policy_deepseek_v4_cross_model_reuses_same_profile() {
    let msg = ChatMessage::assistant_with_tool_calls(
        None,
        vec![serde_json::json!({
            "id":"call_1",
            "type":"function",
            "function":{"name":"read","arguments":"{}"}
        })],
    )
    .with_reasoning_state(
        Some("tool summary".to_string()),
        Some(ReasoningContinuation {
            source_provider: "deepseek".to_string(),
            source_api: "chat_completions".to_string(),
            source_model: "deepseek-v4-pro".to_string(),
            format: ReasoningFormat::DeepseekReasoningContent,
            opaque_payload: serde_json::json!({"reasoning_content":"internal"}),
            fallback_text: Some("tool summary".to_string()),
            provider_refs: None,
        }),
        Some(ContinuityMetadata {
            had_tool_call: true,
            replay_requirement: ReplayRequirement::SameProfileRequired,
        }),
    );
    let profile = ProviderCompatProfile::chat_completions("deepseek-v4-flash");
    assert_eq!(profile.provider, "deepseek");
    assert_eq!(profile.model_family, "deepseek-v4");
    assert!(profile.requires_tool_turn_replay);
    assert_eq!(plan(&profile, &msg), ReplayAction::KeepOpaque);
}

#[test]
fn cross_provider_downgrade_keeps_semantic_history() {
    let msg = ChatMessage::assistant("visible answer").with_reasoning_state(
        None,
        Some(ReasoningContinuation {
            source_provider: "openai".to_string(),
            source_api: "responses".to_string(),
            source_model: "gpt-5".to_string(),
            format: ReasoningFormat::OpenaiResponsesReasoningItems,
            opaque_payload: serde_json::json!([{
                "type": "reasoning",
                "encrypted_content": "enc_123"
            }]),
            fallback_text: None,
            provider_refs: None,
        }),
        Some(ContinuityMetadata {
            had_tool_call: false,
            replay_requirement: ReplayRequirement::SameProfileOptional,
        }),
    );
    let target = ProviderCompatProfile::chat_completions("deepseek-v4-pro");
    assert_eq!(plan(&target, &msg), ReplayAction::StripOpaque);
    let stripped = msg.without_completion_metadata();
    assert_eq!(stripped.text_content(), Some("visible answer"));
    assert!(stripped.reasoning_continuation.is_none());
}
