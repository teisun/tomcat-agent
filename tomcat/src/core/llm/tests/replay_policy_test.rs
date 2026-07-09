//! `core::llm::replay_policy` 焦小测。

use crate::core::llm::replay_policy::{
    apply_text_downgrade, classify_replay_downgrade, model_family, plan, plan_scoped,
    warn_worthy_downgrade, ProviderCompatProfile, ReplayAction, ReplayDowngradeKind,
    ReplayDowngradeReport, ReplayWindow,
};
use crate::core::llm::types::{
    ChatMessage, ContinuityMetadata, MessageKind, ReasoningContinuation, ReasoningFormat,
    ReplayRequirement,
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

fn anthropic_reasoning_message() -> ChatMessage {
    ChatMessage::assistant("answer").with_reasoning_state(
        Some("anthropic summary".to_string()),
        Some(ReasoningContinuation {
            source_provider: "anthropic".to_string(),
            source_api: "messages".to_string(),
            source_model: "claude-opus-4-6".to_string(),
            format: ReasoningFormat::AnthropicThinkingBlocks,
            opaque_payload: serde_json::json!([{
                "type": "thinking",
                "thinking": "internal",
                "signature": "sig_123"
            }]),
            fallback_text: Some("anthropic summary".to_string()),
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
fn replay_policy_anthropic_same_profile_keeps_signed_thinking_blocks() {
    let msg = anthropic_reasoning_message();
    let profile = ProviderCompatProfile::anthropic_messages("claude-opus-4-8");
    assert_eq!(plan(&profile, &msg), ReplayAction::KeepOpaque);
    assert_eq!(
        msg.reasoning_continuation.as_ref().unwrap().opaque_payload[0]["signature"],
        "sig_123"
    );
}

#[test]
fn replay_policy_deepseek_v4_tool_turn_keeps_reasoning_content() {
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
fn replay_policy_deepseek_v4_non_tool_turn_keeps_reasoning_content() {
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
            replay_requirement: ReplayRequirement::SameProfileOptional,
        }),
    );
    let profile = ProviderCompatProfile::chat_completions("deepseek-v4-flash");
    assert_eq!(plan(&profile, &msg), ReplayAction::KeepOpaque);
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
fn classify_replay_downgrade_reports_cross_profile_incompatibility() {
    let msg = openai_reasoning_message();
    let target = ProviderCompatProfile::chat_completions("deepseek-v4-pro");
    let action = plan(&target, &msg);
    assert_eq!(
        classify_replay_downgrade(&target, &msg, &action),
        Some(ReplayDowngradeKind::CrossProfile)
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
    // MiMo 初版按 exact-profile：family 即模型名本身（仅自家可 replay）。
    assert_eq!(model_family("mimo-v2.5-pro"), "mimo-v2.5-pro");
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

fn mimo_reasoning_message() -> ChatMessage {
    ChatMessage::assistant("answer").with_reasoning_state(
        Some("mimo summary".to_string()),
        Some(ReasoningContinuation {
            source_provider: "mimo".to_string(),
            source_api: "chat_completions".to_string(),
            source_model: "mimo-v2.5-pro".to_string(),
            format: ReasoningFormat::DeepseekReasoningContent,
            opaque_payload: serde_json::json!({"reasoning_content":"mimo internal"}),
            fallback_text: Some("mimo summary".to_string()),
            provider_refs: None,
        }),
        Some(ContinuityMetadata {
            had_tool_call: false,
            replay_requirement: ReplayRequirement::SameProfileOptional,
        }),
    )
}

#[test]
fn chat_completions_profile_is_data_driven_for_mimo() {
    // MiMo 仅靠 catalog/数据表一条数据即得到 ReasoningContent profile（与 deepseek 同一条逻辑）。
    let profile = ProviderCompatProfile::chat_completions("mimo-v2.5-pro");
    assert_eq!(profile.provider, "mimo");
    assert_eq!(profile.model_family, "mimo-v2.5-pro");
    assert!(profile.requires_tool_turn_replay);
    assert_eq!(profile.api_family, "chat_completions");
    // 标了 reasoning_content → 走 KeepOpaque 续传。
    assert_eq!(
        plan(&profile, &mimo_reasoning_message()),
        ReplayAction::KeepOpaque
    );
}

#[test]
fn mimo_and_deepseek_do_not_cross_replay() {
    // 数据驱动后两族共用一条代码逻辑，但 same_profile 比对保证不会 KeepOpaque 互串 blob；
    // 跨 profile 且有 fallback_text 时优雅降级为 ConvertToText（不再 StripOpaque）。
    let mimo_msg = mimo_reasoning_message();
    let deepseek_target = ProviderCompatProfile::chat_completions("deepseek-v4-pro");
    // mimo continuity 落到 deepseek target：非同 profile，不吃 opaque blob，转文本续传。
    assert_eq!(
        plan(&deepseek_target, &mimo_msg),
        ReplayAction::ConvertToText("mimo summary".to_string())
    );

    let deepseek_msg = deepseek_v4_compatible_message();
    let mimo_target = ProviderCompatProfile::chat_completions("mimo-v2.5-pro");
    assert_eq!(
        plan(&mimo_target, &deepseek_msg),
        ReplayAction::ConvertToText("safe summary".to_string())
    );
}

#[test]
fn classify_replay_downgrade_reports_same_profile_shape_mismatch() {
    let msg = ChatMessage::assistant("answer").with_reasoning_state(
        Some("safe summary".to_string()),
        Some(ReasoningContinuation {
            source_provider: "deepseek".to_string(),
            source_api: "chat_completions".to_string(),
            source_model: "deepseek-v4-pro".to_string(),
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
    );
    let target = ProviderCompatProfile::chat_completions("deepseek-v4-flash");
    let action = plan(&target, &msg);
    // 同 profile 但 format 不匹配：落不到 KeepOpaque，有 fallback_text → 转文本（仍按
    // SameProfileIncompatible 归类，始终告警）。
    assert_eq!(
        action,
        ReplayAction::ConvertToText("safe summary".to_string())
    );
    assert_eq!(
        classify_replay_downgrade(&target, &msg, &action),
        Some(ReplayDowngradeKind::SameProfileIncompatible)
    );
}

fn deepseek_v4_compatible_message() -> ChatMessage {
    ChatMessage::assistant("answer").with_reasoning_state(
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
            replay_requirement: ReplayRequirement::SameProfileOptional,
        }),
    )
}

#[test]
fn replay_window_strips_older_history_but_keeps_latest_assistant() {
    let target = ProviderCompatProfile::chat_completions("deepseek-v4-pro");
    let messages = vec![
        ChatMessage::user("q1"),
        deepseek_v4_compatible_message(), // idx 1: older assistant turn
        ChatMessage::user("q2"),          // idx 2: latest real user turn-start
        deepseek_v4_compatible_message(), // idx 3: latest assistant turn
    ];
    let window = ReplayWindow::compute(&messages);

    // 窗口外的旧轮次即使同 profile 兼容也必须 strip（不转文本、静默）。
    assert!(!window.contains(1));
    assert_eq!(
        plan_scoped(&target, &messages[1], window.contains(1)),
        ReplayAction::StripOpaque
    );

    // 最新 assistant turn 始终在窗口内，同 profile → KeepOpaque。
    assert!(window.contains(3));
    assert_eq!(
        plan_scoped(&target, &messages[3], window.contains(3)),
        ReplayAction::KeepOpaque
    );
}

#[test]
fn replay_window_ignores_steering_as_turn_start() {
    let mut steering = ChatMessage::user("steer mid-turn");
    steering.kind = MessageKind::Steering;
    let messages = vec![
        ChatMessage::user("q1"),          // idx 0: real user turn-start
        deepseek_v4_compatible_message(), // idx 1: current tool turn assistant
        steering,                         // idx 2: steering (role=user, kind=Steering)
    ];
    let window = ReplayWindow::compute(&messages);

    // steering 不应把窗口起点推到它之后，否则会误伤当前轮的 assistant。
    assert!(window.contains(1));
}

#[test]
fn warn_worthy_skips_graceful_text_but_flags_total_loss() {
    let target = ProviderCompatProfile::openai_responses("gpt-5");

    // 跨 profile + 有 fallback_text → ConvertToText：设计内优雅降级，不告警。
    let with_text = deepseek_v4_compatible_message();
    let action = plan_scoped(&target, &with_text, true);
    assert_eq!(
        action,
        ReplayAction::ConvertToText("safe summary".to_string())
    );
    assert_eq!(warn_worthy_downgrade(&target, &with_text, &action), None);

    // 跨 profile + 无任何文本可救 → StripOpaque：continuity 彻底丢失，告警。
    let no_text = ChatMessage::assistant("answer").with_reasoning_state(
        None,
        Some(ReasoningContinuation {
            source_provider: "deepseek".to_string(),
            source_api: "chat_completions".to_string(),
            source_model: "deepseek-v4-pro".to_string(),
            format: ReasoningFormat::DeepseekReasoningContent,
            opaque_payload: serde_json::json!({"reasoning_content":"internal"}),
            fallback_text: None,
            provider_refs: None,
        }),
        Some(ContinuityMetadata {
            had_tool_call: true,
            replay_requirement: ReplayRequirement::SameProfileRequired,
        }),
    );
    let action = plan_scoped(&target, &no_text, true);
    assert_eq!(action, ReplayAction::StripOpaque);
    assert_eq!(
        warn_worthy_downgrade(&target, &no_text, &action),
        Some(ReplayDowngradeKind::CrossProfile)
    );
}

#[test]
fn deepseek_to_mimo_window_converts_to_text_without_warn() {
    // 回归：终端里观察到的现象——deepseek-v4-pro 续传切到 mimo-v2.5-pro。
    // 两者同为 chat_completions/reasoning_content profile，但 provider 不同（跨 profile）。
    // 期望：窗口内有 fallback_text → 优雅转文本续传，且不触发 cross_profile_lost 误报 warn。
    let deepseek_source = deepseek_v4_compatible_message();
    let mimo_target = ProviderCompatProfile::chat_completions("mimo-v2.5-pro");

    let action = plan_scoped(&mimo_target, &deepseek_source, true);
    assert_eq!(
        action,
        ReplayAction::ConvertToText("safe summary".to_string())
    );

    // 仍被归类为跨 profile 降级（确实没有 KeepOpaque 互吃 blob），但不 warn-worthy。
    assert_eq!(
        classify_replay_downgrade(&mimo_target, &deepseek_source, &action),
        Some(ReplayDowngradeKind::CrossProfile)
    );
    assert_eq!(
        warn_worthy_downgrade(&mimo_target, &deepseek_source, &action),
        None
    );

    // 走一遍请求级聚合器：窗口内这条按 graceful_text 计入，emit 时无 sample → 完全静默。
    let mut report = ReplayDowngradeReport::default();
    report.record_in_window(&mimo_target, &deepseek_source, &action);
    report.emit(&mimo_target);
}

#[test]
fn warn_worthy_flags_same_profile_incompatible_but_not_keep() {
    // 同 profile（deepseek/chat_completions/deepseek-v4）但 format 与 capture_mode 不匹配，
    // 落不到 KeepOpaque → 任何非 keep 动作都算异常，必告警。
    let mismatched = ChatMessage::assistant("answer").with_reasoning_state(
        Some("safe summary".to_string()),
        Some(ReasoningContinuation {
            source_provider: "deepseek".to_string(),
            source_api: "chat_completions".to_string(),
            source_model: "deepseek-v4-pro".to_string(),
            format: ReasoningFormat::OpenaiResponsesReasoningItems,
            opaque_payload: serde_json::json!([{ "type": "reasoning" }]),
            fallback_text: Some("safe summary".to_string()),
            provider_refs: None,
        }),
        Some(ContinuityMetadata {
            had_tool_call: false,
            replay_requirement: ReplayRequirement::SameProfileOptional,
        }),
    );
    let target = ProviderCompatProfile::chat_completions("deepseek-v4-flash");
    let action = plan_scoped(&target, &mismatched, true);
    // 同 profile 不匹配且有 fallback_text → 转文本，但 SameProfileIncompatible 始终告警。
    assert_eq!(
        action,
        ReplayAction::ConvertToText("safe summary".to_string())
    );
    assert_eq!(
        warn_worthy_downgrade(&target, &mismatched, &action),
        Some(ReplayDowngradeKind::SameProfileIncompatible)
    );

    // KeepOpaque（窗口内同 profile 兼容）永远不告警。
    let compatible = deepseek_v4_compatible_message();
    let keep = plan_scoped(&target, &compatible, true);
    assert_eq!(keep, ReplayAction::KeepOpaque);
    assert_eq!(warn_worthy_downgrade(&target, &compatible, &keep), None);
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
