//! Context management data structures (TASK-17 / TASK-20 / TASK-21 §5.7).

use std::path::PathBuf;

use tracing::warn;

use crate::core::compaction::preheat::Preheat;
use crate::core::llm::{ChatMessage, ChatMessageContent, ChatMessageRole, MessageKind};
use crate::infra::error::AppError;

// ---------------------------------------------------------------------------
// §5.7 message / turn ids
// ---------------------------------------------------------------------------

/// 复合 TurnId：`start_id + "::" + end_id`（与 [context-management.md §5.7] 一致）。
///
/// MessageId 不得包含子串 `::`；若违反则打日志但仍拼接，避免线上硬崩。
pub fn compound_turn_id(start_id: &str, end_id: &str) -> String {
    if start_id.contains("::") || end_id.contains("::") {
        warn!(
            %start_id,
            %end_id,
            "compound_turn_id: message id should not contain `::` (reserved as turn separator)"
        );
    }
    format!("{start_id}::{end_id}")
}

// ---------------------------------------------------------------------------
// ApiUsage
// ---------------------------------------------------------------------------

/// API token 使用量快照（从 `StreamEvent::Usage` 捕获）。
#[derive(Debug, Clone)]
pub struct ApiUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

// ---------------------------------------------------------------------------
// CompactionResult (TASK-20)
// ---------------------------------------------------------------------------

/// 异步预热任务完成后的结果。
#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub summary_text: String,
    pub covered_start_id: String,
    pub covered_end_id: String,
    pub covered_count: usize,
    /// JSONL 中 `Compaction` 行的 `id`；apply 时用于原地将 `isBoundary` 置为 true。
    pub transcript_compaction_entry_id: Option<String>,
    /// L1 预热完成时估算：覆盖区 tokens（旧 transcript 无此字段时为 `None`）。
    pub estimated_covered_tokens_before: Option<usize>,
    pub estimated_summary_tokens: Option<usize>,
    /// L2 apply 时计入 `session_obs.compaction_tokens_freed`（`None` 视为 0）。
    pub estimated_tokens_saved: Option<usize>,
    /// 预热任务耗时（ms）；从 transcript 恢复的 pending 为 0。
    pub preheat_elapsed_ms: u64,
}

// ---------------------------------------------------------------------------
// SessionContextObservation / ContextLiveMetrics
// ---------------------------------------------------------------------------

/// 会话级可观测累计：与 [`crate::core::session::store::SessionEntry`] 在 user turn 末同步；**不**含瞬时 ratio/tokens。
#[derive(Debug, Clone, Default)]
pub struct SessionContextObservation {
    /// 成功 apply boundary / L3 trim 等次数（与 `SessionEntry.compaction_count`）。
    pub compaction_count: u32,
    /// 估算释放的 tokens（L0+L2+L3；与 `SessionEntry.compaction_tokens_freed`）。
    pub compaction_tokens_freed: usize,
    /// L0 落盘原始 Unicode 字符数（与 `SessionEntry.tool_result_chars_persisted`；事件字段仍名 bytes）。
    pub tool_result_chars_persisted: usize,
}

/// 瞬时上下文指标：仅内存，**不**写入 `sessions.json`。
#[derive(Debug, Clone, Default)]
pub struct ContextLiveMetrics {
    pub input_tokens_used: usize,
    pub context_utilization_ratio: f64,
    pub preheat_in_progress: bool,
    pub preheat_result_pending: bool,
}

// ---------------------------------------------------------------------------
// ContextState
// ---------------------------------------------------------------------------

/// 运行时上下文状态，在 `chat_loop` 外层初始化一次、跨迭代复用。
pub struct ContextState {
    pub messages: Vec<ChatMessage>,
    pub estimate_context_chars: usize,
    pub context_budget_chars: usize,
    pub context_budget_tokens: usize,
    pub last_api_usage: Option<ApiUsage>,
    pub post_usage_appended_chars: usize,
    /// 当前 session 的 transcript 文件路径，供异步预热 spawn 闭包 clone。
    pub transcript_path: PathBuf,
    /// 异步预热状态机（替代旧 `Option<CompactionSummary>`）。
    pub preheat: Preheat,
    /// 会话累计（刷盘子集）。
    pub session_obs: SessionContextObservation,
    /// 瞬时指标（`AgentLoop` 经方案 1 只写此处，不写独立 `metrics`）。
    pub live: ContextLiveMetrics,
}

fn _assert_send<T: Send>() {}
#[allow(dead_code)]
fn _static_assert_context_state_send() {
    _assert_send::<ContextState>();
}

impl ContextState {
    /// 追加消息后增量更新估算字符数和 post-usage 增量。
    pub fn on_message_appended(&mut self, content_len: usize) {
        self.estimate_context_chars += content_len;
        self.post_usage_appended_chars += content_len;
    }

    /// 估算当前上下文占用的 token 数。
    /// 有 API usage 时基于真实值 + 增量近似；否则 fallback 字符估算。
    pub fn estimated_token_count(&self) -> usize {
        if let Some(ref usage) = self.last_api_usage {
            let base = (usage.prompt_tokens + usage.completion_tokens) as usize;
            base + self.post_usage_appended_chars / 4
        } else {
            self.estimate_context_chars / 4
        }
    }

    /// 当前上下文利用率（0.0 ~ inf）。
    /// `context_budget_tokens == 0` 时返回 `f64::MAX` 以安全触发 Layer 3。
    pub fn usage_ratio(&self) -> f64 {
        if self.context_budget_tokens == 0 {
            return f64::MAX;
        }
        self.estimated_token_count() as f64 / self.context_budget_tokens as f64
    }

    /// LLM 返回 Usage 事件后刷新真实 token 计数，清零增量。
    pub fn update_api_usage(&mut self, prompt_tokens: u32, completion_tokens: u32) {
        self.last_api_usage = Some(ApiUsage {
            prompt_tokens,
            completion_tokens,
        });
        self.post_usage_appended_chars = 0;
    }

    /// compaction 后使 API usage 失效，后续 fallback 到字符估算。
    pub fn invalidate_api_usage(&mut self) {
        self.last_api_usage = None;
        self.post_usage_appended_chars = 0;
    }

    /// 当前上下文是否超预算（token 维度）。
    pub fn is_over_budget(&self) -> bool {
        self.estimated_token_count() > self.context_budget_tokens
    }

    /// 将已完成的 CompactionResult 应用到 messages 列表：
    /// 找到最后一条 `msg_id == covered_end_id` 的消息，将其及之前所有消息替换为摘要消息。
    /// 无匹配时返回 [`AppError::ApplyBoundaryStale`]。
    pub fn apply_boundary(&mut self, result: CompactionResult) -> Result<(), AppError> {
        let end_idx = self
            .messages
            .iter()
            .rposition(|m| m.msg_id.as_deref() == Some(result.covered_end_id.as_str()))
            .ok_or(AppError::ApplyBoundaryStale {
                covered_end_id: result.covered_end_id.clone(),
            })?;

        let batch_chars: usize = self.messages[..=end_idx]
            .iter()
            .map(estimate_msg_chars)
            .sum();

        let mut summary_msg = ChatMessage::compaction_summary(&result.summary_text);
        summary_msg.msg_id = result.transcript_compaction_entry_id.clone();

        self.messages.splice(..=end_idx, [summary_msg]);
        self.estimate_context_chars =
            self.estimate_context_chars.saturating_sub(batch_chars) + result.summary_text.len();
        self.invalidate_api_usage();
        Ok(())
    }

    /// 当前上下文中的 turn 数：user 消息 + compaction 摘要消息之和。
    pub fn turn_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|m| m.role == ChatMessageRole::User || m.kind == MessageKind::CompactionSummary)
            .count()
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// 与 `ContextState::estimated_token_count` 的纯字符 fallback 一致：`chars / 4`。
#[inline]
pub fn estimated_tokens_from_chars(chars: usize) -> usize {
    chars / 4
}

/// 估算单条 ChatMessage 的「字符等价长度」（用于 `estimate_context_chars` fallback）。
///
/// 多模态 `Parts` 调用 [`ChatMessageContentPart::estimated_chars`](crate::core::llm::types::ChatMessageContentPart::estimated_chars)
/// 折算（IMAGE_CHAR_ESTIMATE = 3600 / FILE_CHAR_ESTIMATE = 8000，常量定义在
/// [`crate::core::llm::types`] 顶部），从而与 `OpenAiProvider::count_tokens` /
/// `OpenAiResponsesProvider::count_tokens` 的分子口径对齐——保证 `ContextState::estimated_token_count`
/// 在首轮 stream 完成、`last_api_usage` 还是 `None` 时不会把多模态请求体积当成 0。
pub fn estimate_msg_chars(msg: &ChatMessage) -> usize {
    let content_len = match &msg.content {
        Some(ChatMessageContent::Text(s)) => s.len(),
        Some(ChatMessageContent::Parts(parts)) => {
            parts.iter().map(|p| p.estimated_chars()).sum::<usize>()
        }
        None => 0,
    };
    let tc_len = msg.tool_calls.as_ref().map_or(0, |tcs| {
        tcs.iter().map(|tc| tc.to_string().len()).sum::<usize>()
    });
    content_len + tc_len
}
