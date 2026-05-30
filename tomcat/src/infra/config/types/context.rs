use serde::{Deserialize, Serialize};

use super::llm::DEFAULT_LLM_MODEL;

/// 上下文管理配置：token-aware 滑窗与 Compaction 参数。
/// 详见 `docs/architecture/context-management.md`。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ContextConfig {
    /// LLM 上下文窗口大小（token 数），默认 400,000（GPT-5.4）。
    #[serde(default = "default_context_window")]
    pub context_window: usize,
    /// LLM 最大输出 token 数，默认 128,000。
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: usize,
    /// 受保护的最近 user turn 数（不参与 Layer 1 placeholder 压缩），默认 5。
    #[serde(default = "default_keep_recent_turns")]
    pub keep_recent_turns: usize,
    /// Compaction 摘要使用的 LLM 模型（可配低成本模型），默认与主模型相同。
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
}

fn default_context_window() -> usize {
    400_000
}

fn default_max_output_tokens() -> usize {
    128_000
}

fn default_keep_recent_turns() -> usize {
    5
}

fn default_compaction_model() -> String {
    DEFAULT_LLM_MODEL.to_string()
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

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            context_window: default_context_window(),
            max_output_tokens: default_max_output_tokens(),
            keep_recent_turns: default_keep_recent_turns(),
            compaction_model: default_compaction_model(),
            layer0_single_result_max_chars: default_layer0_single_result_max_chars(),
            layer0_placeholder_threshold_chars: default_layer0_placeholder_threshold_chars(),
            current_tail_compactable_min_chars: default_current_tail_compactable_min_chars(),
            current_tail_single_result_max_chars: default_current_tail_single_result_max_chars(),
            compaction_max_tokens: default_compaction_max_tokens(),
        }
    }
}

/// 计算上下文预算（字符数）。
/// 公式：`(context_window - max_output_tokens) * 4`
/// 其中 `*4` 将 token 转为近似字符数（chars/4 启发式）。
/// 对齐 context-management.md §4.6。
pub fn compute_context_budget_chars(config: &ContextConfig) -> usize {
    let available_tokens = config
        .context_window
        .saturating_sub(config.max_output_tokens);
    available_tokens * 4
}
