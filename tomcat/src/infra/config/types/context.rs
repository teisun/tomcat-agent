use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ResumeHydrationMode {
    #[default]
    Auto,
    Full,
    Tail,
}

/// 上下文管理配置：token-aware 滑窗与 Compaction 参数。
/// 详见 `docs/architecture/context-management.md`。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ContextConfig {
    /// 模型目录未声明上下文窗口时的保守兜底（token 数）。
    #[serde(default = "default_context_window", alias = "context_window")]
    pub context_window_fallback: usize,
    /// 额外本地输出预留。它不发给 provider，且永远不能低于模型的输出能力。
    ///
    /// 旧配置的 `max_output_tokens` 作为兼容别名读取；新配置不应再把模型能力
    /// 写在这里。
    #[serde(default, alias = "max_output_tokens")]
    pub output_reserve_tokens: Option<usize>,
    /// 受保护的最近 user turn 数（不参与 Layer 1 placeholder 压缩），默认 5。
    #[serde(default = "default_keep_recent_turns")]
    pub keep_recent_turns: usize,
    /// Compaction 摘要使用的 LLM 模型（可配低成本模型），默认 `gpt-5.2`。
    #[serde(default = "default_compaction_model")]
    pub compaction_model: String,
    /// Layer 0 落盘阈值：单条 tool_result 字符数上限，默认 50,000。
    #[serde(default = "default_layer0_single_result_max_chars")]
    pub layer0_single_result_max_chars: usize,
    /// Layer 0 占位符替换阈值：compactable zone 内 > 此值的 tool_result 被替换为占位符，默认 10,000。
    #[serde(default = "default_layer0_placeholder_threshold_chars")]
    pub layer0_placeholder_threshold_chars: usize,
    /// Current-tail guard 候选最小字符数：mid-turn reduction 只要工具结果长度达到此值即可入候选，默认 1。
    #[serde(default = "default_current_tail_compactable_min_chars")]
    pub current_tail_compactable_min_chars: usize,
    /// Current-tail guard 的单条大结果阈值：mid-turn reduction 复用 L0 helper 时使用，默认 10,000。
    #[serde(default = "default_current_tail_single_result_max_chars")]
    pub current_tail_single_result_max_chars: usize,
    /// Compaction 摘要最大 token 数（LLM max_tokens 参数），默认 10,000。
    #[serde(default = "default_compaction_max_tokens")]
    pub compaction_max_tokens: usize,
    /// chat/resume 恢复路径：`auto` 按 transcript 大小切换，`full` 强制旧路径，`tail`
    /// 强制 metadata-first + targeted hydrate。
    #[serde(default)]
    pub resume_hydration_mode: ResumeHydrationMode,
    /// `resume_hydration_mode=auto` 时，entry 数达到该阈值才启用 targeted hydrate。
    #[serde(default = "default_resume_lazy_threshold")]
    pub resume_lazy_threshold: usize,
}

fn default_context_window() -> usize {
    400_000
}

fn default_keep_recent_turns() -> usize {
    5
}

fn default_compaction_model() -> String {
    "gpt-5.2".to_string()
}

fn default_layer0_single_result_max_chars() -> usize {
    50_000
}

fn default_layer0_placeholder_threshold_chars() -> usize {
    10_000
}

fn default_current_tail_compactable_min_chars() -> usize {
    1
}

fn default_current_tail_single_result_max_chars() -> usize {
    10_000
}

fn default_compaction_max_tokens() -> usize {
    10_000
}

fn default_resume_lazy_threshold() -> usize {
    2_000
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            context_window_fallback: default_context_window(),
            output_reserve_tokens: None,
            keep_recent_turns: default_keep_recent_turns(),
            compaction_model: default_compaction_model(),
            layer0_single_result_max_chars: default_layer0_single_result_max_chars(),
            layer0_placeholder_threshold_chars: default_layer0_placeholder_threshold_chars(),
            current_tail_compactable_min_chars: default_current_tail_compactable_min_chars(),
            current_tail_single_result_max_chars: default_current_tail_single_result_max_chars(),
            compaction_max_tokens: default_compaction_max_tokens(),
            resume_hydration_mode: ResumeHydrationMode::default(),
            resume_lazy_threshold: default_resume_lazy_threshold(),
        }
    }
}

/// 将一个已解析的输入预算换成字符预算（chars/4 近似估算）。
pub fn compute_context_budget_chars_from_tokens(input_budget_tokens: usize) -> usize {
    input_budget_tokens * 4
}

/// 模型尚未解析时的兼容预算。生产聊天路径应使用
/// `EffectiveModelLimits::input_budget_tokens`，不能把本函数的回退值当作模型事实。
pub fn fallback_input_budget_tokens(config: &ContextConfig) -> usize {
    let unknown_model_reserve = (config.context_window_fallback / 4).min(128_000);
    let conservative_reserve = config
        .output_reserve_tokens
        .unwrap_or(0)
        .max(unknown_model_reserve);
    config
        .context_window_fallback
        .saturating_sub(conservative_reserve)
}

/// 兼容尚未解析模型能力的调用点。主聊天请求在解析到
/// `EffectiveModelLimits` 后必须改用其 `input_budget_tokens`。
pub fn compute_context_budget_chars(config: &ContextConfig) -> usize {
    compute_context_budget_chars_from_tokens(fallback_input_budget_tokens(config))
}
