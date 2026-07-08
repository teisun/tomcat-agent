//! # Reasoning continuity replay policy
//!
//! 集中定义 transcript-first continuity 的 profile 与 replay 决策，避免把
//! `keep / convert / strip` 规则散落到各个 provider wire 适配器中。

use super::types::{
    ChatMessage, ChatMessageContent, ChatMessageRole, MessageKind, ReasoningContinuation,
    ReasoningFormat, ReplayRequirement,
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
}

/// 走 `reasoning_content` 续传的模型家族；不在表内的 chat-completions 模型默认不续传。
const CHAT_COMPLETIONS_CONTINUITY_RULES: &[ChatCompletionsContinuityRule] = &[
    ChatCompletionsContinuityRule {
        family: "deepseek-v4",
        provider: "deepseek",
        profile_id: "deepseek.v4.reasoning_content",
    },
    ChatCompletionsContinuityRule {
        family: "mimo-v2.5-pro",
        provider: "mimo",
        profile_id: "mimo.v2_5_pro.reasoning_content",
    },
];

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
                // 跨 profile（如 deepseek↔mimo）切换时，若有 fallback_text/thinking_text 则优雅
                // 降级为 ConvertToText（不告警），仅在无文本可救时才 StripOpaque——与续传文档
                // §4.2.4/§4.2.6 的降级阶梯一致。
                downgrade_mode: DowngradeMode::FallbackText,
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
                downgrade_mode: DowngradeMode::VisibleHistoryOnly,
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
            downgrade_mode: DowngradeMode::FallbackText,
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
    if matches!(target.downgrade_mode, DowngradeMode::FallbackText) {
        if let Some(text) = continuity_fallback_text(message, continuation) {
            return ReplayAction::ConvertToText(text);
        }
    }
    ReplayAction::StripOpaque
}

/// 带「可 replay 窗口」约束的出站决策：窗口外的历史 turn 一律 `StripOpaque`
/// （只保留消息原有可见内容，丢弃隐藏 continuity blob，不转文本）；窗口内沿用 [`plan`]。
pub fn plan_scoped(
    target: &ProviderCompatProfile,
    message: &ChatMessage,
    in_window: bool,
) -> ReplayAction {
    if !in_window {
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
    last_assistant_idx: Option<usize>,
}

impl ReplayWindow {
    /// 基于整段 `messages` 计算窗口边界。
    /// - `current_turn_start`：最后一条「真实 user 问句」（`role=user` 且 `kind=Normal`，
    ///   排除 steering 与 compaction summary）之后的位置；无则为 0。
    /// - `last_assistant_idx`：最后一条 assistant 消息下标，保证最新 assistant turn 始终在窗口内。
    pub fn compute(messages: &[ChatMessage]) -> Self {
        let current_turn_start = messages
            .iter()
            .rposition(|m| {
                matches!(m.role, ChatMessageRole::User) && matches!(m.kind, MessageKind::Normal)
            })
            .map(|i| i + 1)
            .unwrap_or(0);
        let last_assistant_idx = messages
            .iter()
            .rposition(|m| matches!(m.role, ChatMessageRole::Assistant));
        Self {
            current_turn_start,
            last_assistant_idx,
        }
    }

    /// 该下标的消息是否落在可 replay 窗口内。
    pub fn contains(&self, idx: usize) -> bool {
        idx >= self.current_turn_start || Some(idx) == self.last_assistant_idx
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
        ReplayAction::ConvertToText(_) | ReplayAction::StripOpaque => {
            Some(if same_profile(target, continuation) {
                ReplayDowngradeKind::SameProfileIncompatible
            } else {
                ReplayDowngradeKind::CrossProfile
            })
        }
    }
}

/// 判断某个**窗口内** turn 的降级是否值得告警，并返回根因分类。
///
/// 返回 `None` = 静默：要么是 `KeepOpaque`（成功），要么是跨 profile 的 `ConvertToText`
/// （设计内的优雅降级，推理已保成文本，预期行为，不刷屏）。
/// 返回 `Some(kind)` = 告警：
/// - **A. SameProfileIncompatible**：同 profile 却没能 `KeepOpaque`（任何非 keep 动作都算异常）；
/// - **B. CrossProfile + `StripOpaque`**：跨 profile 且连文本都救不回 → continuity 彻底丢失。
pub fn warn_worthy_downgrade(
    target: &ProviderCompatProfile,
    message: &ChatMessage,
    action: &ReplayAction,
) -> Option<ReplayDowngradeKind> {
    let kind = classify_replay_downgrade(target, message, action)?;
    match kind {
        ReplayDowngradeKind::SameProfileIncompatible => Some(kind),
        ReplayDowngradeKind::CrossProfile => match action {
            ReplayAction::StripOpaque => Some(kind),
            _ => None,
        },
    }
}

fn action_label(action: &ReplayAction) -> &'static str {
    match action {
        ReplayAction::KeepOpaque => "keep_opaque",
        ReplayAction::ConvertToText(_) => "convert_to_text",
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
    cross_profile_lost: usize,
    graceful_text: usize,
    stripped_old_history: usize,
    sample: Option<ReplayDowngradeSample>,
}

impl ReplayDowngradeReport {
    /// 记录窗口内一条 turn 的出站结果（`KeepOpaque` / 无 continuation 不计入）。
    pub fn record_in_window(
        &mut self,
        target: &ProviderCompatProfile,
        message: &ChatMessage,
        action: &ReplayAction,
    ) {
        if classify_replay_downgrade(target, message, action).is_none() {
            return;
        }
        if matches!(action, ReplayAction::ConvertToText(_)) {
            self.graceful_text += 1;
        }
        let Some(warn_kind) = warn_worthy_downgrade(target, message, action) else {
            return;
        };
        self.warn_worthy += 1;
        match warn_kind {
            ReplayDowngradeKind::SameProfileIncompatible => self.same_profile_incompatible += 1,
            ReplayDowngradeKind::CrossProfile => self.cross_profile_lost += 1,
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
            cross_profile_lost = self.cross_profile_lost,
            graceful_text = self.graceful_text,
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
                && continuation.source_provider == "openai"
                && continuation.source_api == "responses"
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
    } else if lower.starts_with("claude-opus-4-") {
        "claude-opus-4".to_string()
    } else if lower.is_empty() {
        "unknown".to_string()
    } else {
        lower
    }
}
