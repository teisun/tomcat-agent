//! # Reasoning continuity replay policy
//!
//! 集中定义 transcript-first continuity 的 profile 与 replay 决策，避免把
//! `keep / strip` 规则散落到各个 provider wire 适配器中。

use super::types::{
    ChatMessage, ChatMessageRole, ReasoningContinuation, ReasoningFormat, ReplayRequirement,
};
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
}

/// 对单条 assistant turn continuity 的出站决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayAction {
    KeepOpaque,
    StripOpaque,
}

/// 降级日志的根因分类；用于区分真正的跨 profile 退化与同 profile 下的异常不兼容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayDowngradeKind {
    CrossProfile,
    SameProfileIncompatible,
}

impl ReplayDowngradeKind {
    fn as_str(self) -> &'static str {
        match self {
            ReplayDowngradeKind::CrossProfile => "cross_profile",
            ReplayDowngradeKind::SameProfileIncompatible => "same_profile_incompatible",
        }
    }

    fn message(self) -> &'static str {
        match self {
            ReplayDowngradeKind::CrossProfile => {
                "reasoning continuity downgraded for incompatible target profile"
            }
            ReplayDowngradeKind::SameProfileIncompatible => {
                "reasoning continuity could not be replayed within target profile"
            }
        }
    }
}

/// chat-completions `reasoning_content` continuity 的**数据表（单一事实源）**。
///
/// 设计目标：把「哪个模型走 reasoning_content 续传」从代码里的 `match "deepseek"`
/// 改成一张数据表。新增同类模型 = 加一行；continuity 链路的各道门只读
/// [`ProviderCompatProfile`] 字段（`capture_mode` / `api_family` / `provider`+`model_family`），
/// 不再按厂商名硬编码。DeepSeek 与 MiMo 现在都只是表里的一行，共用同一条逻辑。
struct ChatCompletionsContinuityRule {
    /// [`model_family`] 归一后的家族名。
    family: &'static str,
    /// 逻辑厂商（用于 same-profile 比对与日志）。
    provider: &'static str,
    profile_id: &'static str,
    preserve_history: bool,
}

/// 走 `reasoning_content` 续传的模型家族；不在表内的 chat-completions 模型默认不续传。
const CHAT_COMPLETIONS_CONTINUITY_RULES: &[ChatCompletionsContinuityRule] = &[
    ChatCompletionsContinuityRule {
        family: "deepseek-v4",
        provider: "deepseek",
        profile_id: "deepseek.v4.reasoning_content",
        preserve_history: false,
    },
    ChatCompletionsContinuityRule {
        family: "mimo-v2.5-pro",
        provider: "mimo",
        profile_id: "mimo.v2_5_pro.reasoning_content",
        preserve_history: false,
    },
    ChatCompletionsContinuityRule {
        family: "kimi-k3",
        provider: "moonshot",
        profile_id: "moonshot.k3.reasoning_content",
        preserve_history: true,
    },
];

impl ProviderCompatProfile {
    /// K3 requires preserved thinking across real user turns as well as tool loops.
    /// https://platform.kimi.ai/docs/guide/use-thinking-models
    pub fn preserves_thinking_history(&self) -> bool {
        self.api_family == "chat_completions"
            && CHAT_COMPLETIONS_CONTINUITY_RULES
                .iter()
                .any(|rule| rule.family == self.model_family && rule.preserve_history)
    }

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
        }
    }

    pub fn openai_responses_routed(
        model: &str,
        provider: &str,
        base_url: &str,
        credential_fingerprint: &str,
    ) -> Self {
        let provider = normalized_route_component(provider, "openai");
        let base_url = normalized_route_component(base_url, "default-base");
        let credential_fingerprint =
            normalized_route_component(credential_fingerprint, "anonymous-credential");
        let model_family = model_family(model);
        Self {
            profile_id: format!(
                "openai.responses.route/{provider}/{model_family}/{base_url}/{credential_fingerprint}"
            ),
            provider,
            api_family: "responses".to_string(),
            model_family,
            capture_mode: CaptureMode::OpaqueItems,
            replay_acceptance: ReplayAcceptance::SameProfileOnly,
            requires_tool_turn_replay: false,
            supports_response_id_hint: true,
        }
    }

    pub fn chat_completions(model: &str) -> Self {
        let family = model_family(model);
        match CHAT_COMPLETIONS_CONTINUITY_RULES
            .iter()
            .find(|rule| rule.family == family)
        {
            Some(rule) => Self {
                profile_id: rule.profile_id.to_string(),
                provider: rule.provider.to_string(),
                api_family: "chat_completions".to_string(),
                model_family: family,
                capture_mode: CaptureMode::ReasoningContent,
                replay_acceptance: ReplayAcceptance::SameProfileOnly,
                requires_tool_turn_replay: true,
                supports_response_id_hint: false,
            },
            None => Self {
                profile_id: "openai.chat_completions.default".to_string(),
                provider: "openai".to_string(),
                api_family: "chat_completions".to_string(),
                model_family: family,
                capture_mode: CaptureMode::None,
                replay_acceptance: ReplayAcceptance::Never,
                requires_tool_turn_replay: false,
                supports_response_id_hint: false,
            },
        }
    }

    pub fn anthropic_messages(model: &str) -> Self {
        Self {
            profile_id: "anthropic.messages.default".to_string(),
            provider: "anthropic".to_string(),
            api_family: "messages".to_string(),
            model_family: model_family(model),
            capture_mode: CaptureMode::OpaqueItems,
            replay_acceptance: ReplayAcceptance::SameProfileOnly,
            requires_tool_turn_replay: true,
            supports_response_id_hint: false,
        }
    }
}

/// 统一计算 assistant turn continuity 的出站策略。
pub fn plan(target: &ProviderCompatProfile, message: &ChatMessage) -> ReplayAction {
    let Some(continuation) = message.reasoning_continuation.as_ref() else {
        return ReplayAction::StripOpaque;
    };
    if is_compatible(target, continuation) {
        return ReplayAction::KeepOpaque;
    }
    ReplayAction::StripOpaque
}

/// 带「可 replay 窗口」约束的出站决策：窗口外的历史 turn 一律 `StripOpaque`
/// （只保留消息原有可见内容，丢弃隐藏 continuity blob）；窗口内沿用 [`plan`]。
pub fn plan_scoped(
    target: &ProviderCompatProfile,
    message: &ChatMessage,
    in_window: bool,
) -> ReplayAction {
    if !in_window && !target.preserves_thinking_history() {
        return ReplayAction::StripOpaque;
    }
    plan(target, message)
}

/// 「可 replay 窗口」：只有最新 assistant turn 与「当前 turn」（最后一条真实 user 之后的
/// 消息）内的 continuity 参与 opaque/文本 replay；更早的历史轮次出站时一律 strip。
///
/// 这样既保住当前轮的高保真续传，又从根上避免对整段历史逐条降级判定与刷屏。
#[derive(Debug, Clone, Copy)]
pub struct ReplayWindow {
    current_turn_start: usize,
}

impl ReplayWindow {
    /// 基于整段 `messages` 计算窗口边界。
    /// - `current_turn_start`：最后一条可作为模型输入的 user message（Normal 或 Signal）
    ///   之后的位置；Steering、Nudge 与 compaction summary 不会切断当前窗口。无则为 0。
    pub fn compute(messages: &[ChatMessage]) -> Self {
        let current_turn_start = messages
            .iter()
            .rposition(|m| matches!(m.role, ChatMessageRole::User) && m.kind.is_replay_input())
            .map(|i| i + 1)
            .unwrap_or(0);
        Self { current_turn_start }
    }

    /// 该下标的消息是否落在可 replay 窗口内。
    pub fn contains(&self, idx: usize) -> bool {
        idx >= self.current_turn_start
    }
}

/// 将 continuity 的降级根因转成稳定分类，便于日志与测试复用。
pub fn classify_replay_downgrade(
    target: &ProviderCompatProfile,
    message: &ChatMessage,
    action: &ReplayAction,
) -> Option<ReplayDowngradeKind> {
    let continuation = message.reasoning_continuation.as_ref()?;
    match action {
        ReplayAction::KeepOpaque => None,
        ReplayAction::StripOpaque => Some(if same_profile(target, continuation) {
            ReplayDowngradeKind::SameProfileIncompatible
        } else {
            ReplayDowngradeKind::CrossProfile
        }),
    }
}

/// 判断某个**窗口内** turn 的降级是否值得告警，并返回根因分类。
///
/// 返回 `None` = 静默：要么是 `KeepOpaque`（成功），要么是跨 profile 的 opaque strip
/// （opaque reasoning 无法安全跨 profile 重放，预期行为，不刷屏）。
/// 返回 `Some(kind)` = 告警：
/// - **A. SameProfileIncompatible**：同 profile 却没能 `KeepOpaque`（任何非 keep 动作都算异常）；
pub fn warn_worthy_downgrade(
    target: &ProviderCompatProfile,
    message: &ChatMessage,
    action: &ReplayAction,
) -> Option<ReplayDowngradeKind> {
    let kind = classify_replay_downgrade(target, message, action)?;
    match kind {
        ReplayDowngradeKind::SameProfileIncompatible => Some(kind),
        ReplayDowngradeKind::CrossProfile => None,
    }
}

fn action_label(action: &ReplayAction) -> &'static str {
    match action {
        ReplayAction::KeepOpaque => "keep_opaque",
        ReplayAction::StripOpaque => "strip_opaque",
    }
}

#[derive(Debug)]
struct ReplayDowngradeSample {
    kind: ReplayDowngradeKind,
    action_label: &'static str,
    source_provider: String,
    source_api: String,
    source_model: String,
    had_tool_call: bool,
}

/// 按请求聚合的 replay 降级告警收集器：把「逐消息 warn」换成「每请求至多一条汇总」。
///
/// 不记录 opaque payload；窗口外老历史的静默 strip 仅计数、从不告警。
#[derive(Debug, Default)]
pub struct ReplayDowngradeReport {
    warn_worthy: usize,
    same_profile_incompatible: usize,
    stripped_old_history: usize,
    sample: Option<ReplayDowngradeSample>,
}

impl ReplayDowngradeReport {
    /// Record one replay decision that was evaluated for this outbound request.
    ///
    /// Chat Completions invokes this only for its current replay window;
    /// Responses invokes it for the full explicit history. The name therefore
    /// describes the decision, not a windowing policy owned by this collector.
    pub fn record_replay_decision(
        &mut self,
        target: &ProviderCompatProfile,
        message: &ChatMessage,
        action: &ReplayAction,
    ) {
        if classify_replay_downgrade(target, message, action).is_none() {
            return;
        }
        let Some(warn_kind) = warn_worthy_downgrade(target, message, action) else {
            return;
        };
        self.warn_worthy += 1;
        match warn_kind {
            ReplayDowngradeKind::SameProfileIncompatible => self.same_profile_incompatible += 1,
            ReplayDowngradeKind::CrossProfile => {}
        }
        if self.sample.is_none() {
            if let Some(continuation) = message.reasoning_continuation.as_ref() {
                self.sample = Some(ReplayDowngradeSample {
                    kind: warn_kind,
                    action_label: action_label(action),
                    source_provider: continuation.source_provider.clone(),
                    source_api: continuation.source_api.clone(),
                    source_model: model_family(&continuation.source_model),
                    had_tool_call: message
                        .continuity
                        .as_ref()
                        .map(|meta| meta.had_tool_call)
                        .unwrap_or(false),
                });
            }
        }
    }

    /// 记录窗口外被静默 strip 的历史 continuity（仅统计，不告警）。
    pub fn record_stripped_old_history(&mut self, message: &ChatMessage) {
        if message.reasoning_continuation.is_some() {
            self.stripped_old_history += 1;
        }
    }

    /// 每请求至多一条汇总告警；无 warn-worthy 项时完全静默。
    pub fn emit(&self, target: &ProviderCompatProfile) {
        let Some(sample) = self.sample.as_ref() else {
            return;
        };
        warn!(
            target_profile = %target.profile_id,
            downgrade_kind = sample.kind.as_str(),
            warn_worthy = self.warn_worthy,
            same_profile_incompatible = self.same_profile_incompatible,
            stripped_old_history = self.stripped_old_history,
            source_provider = %sample.source_provider,
            source_api = %sample.source_api,
            source_model = %sample.source_model,
            action = sample.action_label,
            had_tool_call = sample.had_tool_call,
            "{}（历史老 turn 已按窗口策略静默 strip）",
            sample.kind.message()
        );
    }
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

fn is_compatible(target: &ProviderCompatProfile, continuation: &ReasoningContinuation) -> bool {
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
                && continuation.source_api == "responses"
                && target.api_family == "responses"
                && same_profile(target, continuation)
        }
        // chat-completions reasoning_content：不再按厂商名硬编码，改为按 profile 数据判定。
        // 任意标记为 ReasoningContent 的 chat-completions 模型（deepseek / mimo / 未来同类）
        // 只要 source 与 target 是同一 profile（provider + model_family 一致）即可 replay。
        ReasoningFormat::DeepseekReasoningContent => {
            continuation.source_api == "chat_completions"
                && target.api_family == "chat_completions"
                && matches!(target.capture_mode, CaptureMode::ReasoningContent)
                && same_profile(target, continuation)
        }
        ReasoningFormat::AnthropicThinkingBlocks => {
            continuation.source_provider == "anthropic"
                && continuation.source_api == "messages"
                && target.api_family == "messages"
                && matches!(target.capture_mode, CaptureMode::OpaqueItems)
                && same_profile(target, continuation)
        }
    }
}

fn same_profile(target: &ProviderCompatProfile, continuation: &ReasoningContinuation) -> bool {
    if let Some(replay_profile_id) = continuation
        .provider_refs
        .as_ref()
        .and_then(|refs| refs.replay_profile_id.as_deref())
    {
        return continuation.source_api == target.api_family
            && model_family(&continuation.source_model) == target.model_family
            && replay_profile_id == target.profile_id;
    }
    if continuation.source_api == "responses"
        && target.api_family == "responses"
        && target.profile_id != "openai.responses.default"
    {
        // New routed profiles must carry an explicit replay profile id; otherwise fail closed
        // and strip opaque reasoning rather than risk cross-relay replay.
        return false;
    }
    continuation.source_provider == target.provider
        && continuation.source_api == target.api_family
        && model_family(&continuation.source_model) == target.model_family
}

fn normalized_route_component(raw: &str, fallback: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/').to_ascii_lowercase();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed
    }
}

/// 归一到 profile 粒度的 model family。
pub fn model_family(model: &str) -> String {
    let lower = model.trim().to_ascii_lowercase();
    if lower.starts_with("deepseek-v4-pro") || lower.starts_with("deepseek-v4-flash") {
        "deepseek-v4".to_string()
    } else if lower.starts_with("gpt-5") {
        "gpt-5".to_string()
    } else if lower.starts_with("claude-opus-4-") {
        "claude-opus-4".to_string()
    } else if lower.is_empty() {
        "unknown".to_string()
    } else {
        lower
    }
}
