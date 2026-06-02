//! # Reasoning continuity replay policy
//!
//! 集中定义 transcript-first continuity 的 profile 与 replay 决策，避免把
//! `keep / convert / strip` 规则散落到各个 provider wire 适配器中。

use super::types::{
    ChatMessage, ChatMessageContent, ContinuityMetadata, ReasoningContinuation, ReasoningFormat,
    ReplayRequirement,
};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use tracing::warn;

/// provider 对 continuity 的抓取形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    OpaqueItems,
    ReasoningContent,
    None,
}

/// 目标 provider 对 opaque blob 的接受范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayAcceptance {
    SameProfileOnly,
    SameApiFamily,
    Never,
}

/// opaque blob 不兼容时的降级策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DowngradeMode {
    FallbackText,
    VisibleHistoryOnly,
}

/// `(provider, api, model family)` 级别的兼容规则卡。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCompatProfile {
    pub profile_id: String,
    pub provider: String,
    pub api_family: String,
    pub model_family: String,
    pub capture_mode: CaptureMode,
    pub replay_acceptance: ReplayAcceptance,
    pub requires_tool_turn_replay: bool,
    pub supports_response_id_hint: bool,
    pub downgrade_mode: DowngradeMode,
}

/// 对单条 assistant turn continuity 的出站决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayAction {
    KeepOpaque,
    ConvertToText(String),
    StripOpaque,
}

static REPLAY_DOWNGRADE_WARNINGS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

impl ProviderCompatProfile {
    pub fn openai_responses(model: &str) -> Self {
        Self {
            profile_id: "openai.responses.default".to_string(),
            provider: "openai".to_string(),
            api_family: "responses".to_string(),
            model_family: model_family(model),
            capture_mode: CaptureMode::OpaqueItems,
            replay_acceptance: ReplayAcceptance::SameProfileOnly,
            requires_tool_turn_replay: false,
            supports_response_id_hint: true,
            downgrade_mode: DowngradeMode::FallbackText,
        }
    }

    pub fn chat_completions(model: &str) -> Self {
        let family = model_family(model);
        match family.as_str() {
            "deepseek-v4" => Self {
                profile_id: "deepseek.v4.tool_sensitive".to_string(),
                provider: "deepseek".to_string(),
                api_family: "chat_completions".to_string(),
                model_family: family,
                capture_mode: CaptureMode::ReasoningContent,
                replay_acceptance: ReplayAcceptance::SameProfileOnly,
                requires_tool_turn_replay: true,
                supports_response_id_hint: false,
                downgrade_mode: DowngradeMode::VisibleHistoryOnly,
            },
            _ => Self {
                profile_id: "openai.chat_completions.default".to_string(),
                provider: "openai".to_string(),
                api_family: "chat_completions".to_string(),
                model_family: family,
                capture_mode: CaptureMode::None,
                replay_acceptance: ReplayAcceptance::Never,
                requires_tool_turn_replay: false,
                supports_response_id_hint: false,
                downgrade_mode: DowngradeMode::VisibleHistoryOnly,
            },
        }
    }
}

/// 统一计算 assistant turn continuity 的出站策略。
pub fn plan(target: &ProviderCompatProfile, message: &ChatMessage) -> ReplayAction {
    let Some(continuation) = message.reasoning_continuation.as_ref() else {
        return ReplayAction::StripOpaque;
    };
    if is_compatible(target, continuation, message.continuity.as_ref()) {
        return ReplayAction::KeepOpaque;
    }
    if matches!(target.downgrade_mode, DowngradeMode::FallbackText) {
        if let Some(text) = continuity_fallback_text(message, continuation) {
            return ReplayAction::ConvertToText(text);
        }
    }
    ReplayAction::StripOpaque
}

/// 对跨 profile 的 continuity 降级做低噪音告警；不记录 opaque payload。
pub fn warn_replay_downgrade_once(
    target: &ProviderCompatProfile,
    message: &ChatMessage,
    action: &ReplayAction,
) {
    let Some(continuation) = message.reasoning_continuation.as_ref() else {
        return;
    };
    let action_label = match action {
        ReplayAction::KeepOpaque => return,
        ReplayAction::ConvertToText(_) => "convert_to_text",
        ReplayAction::StripOpaque => "strip_opaque",
    };
    let cache_key = format!(
        "{}|{}|{}|{}|{}",
        target.profile_id,
        continuation.source_provider,
        continuation.source_api,
        model_family(&continuation.source_model),
        action_label
    );
    let cache = REPLAY_DOWNGRADE_WARNINGS.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut seen) = cache.lock() else {
        return;
    };
    if !seen.insert(cache_key) {
        return;
    }
    drop(seen);

    warn!(
        target_profile = %target.profile_id,
        source_provider = %continuation.source_provider,
        source_api = %continuation.source_api,
        source_model = %model_family(&continuation.source_model),
        action = action_label,
        had_tool_call = message
            .continuity
            .as_ref()
            .map(|meta| meta.had_tool_call)
            .unwrap_or(false),
        "reasoning continuity downgraded for target profile"
    );
}

/// 当 opaque blob 无法原样回放时，取最佳 effort 的安全文本 continuity。
pub fn continuity_fallback_text(
    message: &ChatMessage,
    continuation: &ReasoningContinuation,
) -> Option<String> {
    continuation
        .fallback_text
        .clone()
        .or_else(|| message.thinking_text.clone())
        .filter(|text| !text.trim().is_empty())
}

/// 根据 profile 与 turn shape 计算 transcript 中应写入的 replay 强度。
pub fn replay_requirement_for_profile(
    profile: &ProviderCompatProfile,
    had_tool_call: bool,
) -> ReplayRequirement {
    match profile.replay_acceptance {
        ReplayAcceptance::Never => ReplayRequirement::Never,
        _ if profile.requires_tool_turn_replay && had_tool_call => {
            ReplayRequirement::SameProfileRequired
        }
        _ => ReplayRequirement::SameProfileOptional,
    }
}

/// 仅对出站 clone 生效：把 continuity 退化为安全文本，不污染 transcript 主账本。
pub fn apply_text_downgrade(message: &ChatMessage, continuity_text: &str) -> ChatMessage {
    let mut downgraded = message.without_completion_metadata();
    if continuity_text.trim().is_empty() {
        return downgraded;
    }

    let existing = match downgraded.content.take() {
        Some(ChatMessageContent::Text(text)) => Some(text),
        Some(other) => {
            downgraded.content = Some(other);
            None
        }
        None => None,
    };

    let new_text = match existing {
        Some(text) if !text.trim().is_empty() => {
            format!("{text}\n\n[reasoning continuity]\n{continuity_text}")
        }
        _ => continuity_text.to_string(),
    };
    downgraded.content = Some(ChatMessageContent::Text(new_text));
    downgraded
}

fn is_compatible(
    target: &ProviderCompatProfile,
    continuation: &ReasoningContinuation,
    continuity: Option<&ContinuityMetadata>,
) -> bool {
    match target.replay_acceptance {
        ReplayAcceptance::Never => return false,
        ReplayAcceptance::SameApiFamily if continuation.source_api != target.api_family => {
            return false
        }
        ReplayAcceptance::SameProfileOnly if !same_profile(target, continuation) => return false,
        ReplayAcceptance::SameApiFamily | ReplayAcceptance::SameProfileOnly => {}
    }

    match continuation.format {
        ReasoningFormat::OpenaiResponsesReasoningItems => {
            matches!(target.capture_mode, CaptureMode::OpaqueItems)
                && continuation.source_provider == "openai"
                && continuation.source_api == "responses"
                && same_profile(target, continuation)
        }
        ReasoningFormat::DeepseekReasoningContent => {
            if continuation.source_provider != "deepseek"
                || continuation.source_api != "chat_completions"
                || target.provider != "deepseek"
                || target.api_family != "chat_completions"
                || model_family(&continuation.source_model) != target.model_family
                || !matches!(target.capture_mode, CaptureMode::ReasoningContent)
            {
                return false;
            }
            if target.requires_tool_turn_replay {
                continuity.map(|meta| meta.had_tool_call).unwrap_or(false)
            } else {
                true
            }
        }
    }
}

fn same_profile(target: &ProviderCompatProfile, continuation: &ReasoningContinuation) -> bool {
    continuation.source_provider == target.provider
        && continuation.source_api == target.api_family
        && model_family(&continuation.source_model) == target.model_family
}

/// 归一到 profile 粒度的 model family。
pub fn model_family(model: &str) -> String {
    let lower = model.trim().to_ascii_lowercase();
    if lower.starts_with("deepseek-v4-pro") || lower.starts_with("deepseek-v4-flash") {
        "deepseek-v4".to_string()
    } else if lower.starts_with("gpt-5") {
        "gpt-5".to_string()
    } else if lower.is_empty() {
        "unknown".to_string()
    } else {
        lower
    }
}
