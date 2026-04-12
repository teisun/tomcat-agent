//! Context management data structures (TASK-17 / TASK-20 / TASK-21 §5.7).

use std::path::PathBuf;

use tracing::warn;

use crate::core::agent_loop::AgentMessage;
use crate::core::compaction::preheat::Preheat;
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

/// `start::end` 的右段；无 `::` 时返回 `None`（旧式单层 turn id）。
pub fn compound_id_suffix(turn_id: &str) -> Option<&str> {
    turn_id.rsplit_once("::").map(|(_, r)| r)
}

/// `start::end` 的左段；无 `::` 时整个串视为左段。
pub fn compound_id_prefix(turn_id: &str) -> &str {
    turn_id.split_once("::").map(|(l, _)| l).unwrap_or(turn_id)
}

fn user_turn_matches_covered_end(turn: &TurnEntry, covered_end: &str) -> bool {
    match turn {
        TurnEntry::UserTurn { end_id, id, .. } => {
            end_id == covered_end
                || id == covered_end
                || compound_id_suffix(id).is_some_and(|s| s == covered_end)
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// TurnEntry
// ---------------------------------------------------------------------------

/// 上下文管理的分组单位：一条 user 消息及其后所有 assistant/tool 消息，
/// 或一条 Compaction 生成的结构化摘要。
#[derive(Debug, Clone)]
pub enum TurnEntry {
    UserTurn {
        /// 恒等于 `compound_turn_id(start_id, end_id)`（§5.7）。
        id: String,
        /// 本条 turn 在 transcript 中**首条** message 的 id（通常为 user 行）。
        start_id: String,
        /// 本条 turn 在 transcript 中**末条** message 的 id。
        end_id: String,
        messages: Vec<AgentMessage>,
        timestamp: String,
    },
    SummaryTurn {
        id: String,
        summary: String,
        timestamp: String,
    },
}

impl TurnEntry {
    pub fn id(&self) -> &str {
        match self {
            TurnEntry::UserTurn { id, .. } => id,
            TurnEntry::SummaryTurn { id, .. } => id,
        }
    }

    pub fn timestamp(&self) -> &str {
        match self {
            TurnEntry::UserTurn { timestamp, .. } => timestamp,
            TurnEntry::SummaryTurn { timestamp, .. } => timestamp,
        }
    }
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
    pub user_turns_list: Vec<TurnEntry>,
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

    /// 将本轮对话包登记到 `user_turns_list`（供后续 `build_context_from_state` / 压缩等使用）。
    ///
    /// **不计入** `estimate_context_chars` / `post_usage_appended_chars`：同一轮的用户句、assistant
    /// 与 tool 结果已在 `chat` 的 `on_message_appended` 与 `agent_loop` 内增量累加；此处再按
    /// `estimate_turn_chars` 加一遍会导致下一轮「首轮 LLM 前」的 `context_metrics` 虚高（约一整轮重复）。
    pub fn on_new_user_turn(&mut self, turn: TurnEntry) {
        self.user_turns_list.push(turn);
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

    /// 将已完成的 CompactionResult 应用到 user_turns_list：
    /// §5.7.5：最小 `k` 使 `UserTurn` 与 `covered_end_id` 匹配（`end_id` / `id` / 复合 id 右段），
    /// 再 `splice(0..=k, [SummaryTurn])`。无匹配时返回 [`AppError::ApplyBoundaryStale`]（不第二遍扫 start/end）。
    pub fn apply_boundary(&mut self, result: CompactionResult) -> Result<(), AppError> {
        let covered_end = result.covered_end_id.as_str();
        let covered_start = result.covered_start_id.as_str();

        let k_message = self
            .user_turns_list
            .iter()
            .position(|t| user_turn_matches_covered_end(t, covered_end));

        let (start, end) = if let Some(k) = k_message {
            if let Some(TurnEntry::UserTurn { start_id, id, .. }) = self.user_turns_list.first() {
                let prefix = compound_id_prefix(id);
                if start_id.as_str() != covered_start && prefix != covered_start {
                    warn!(
                        covered_start_id = %result.covered_start_id,
                        covered_end_id = %result.covered_end_id,
                        %start_id,
                        "apply_boundary: covered_start_id does not match first turn (continuing with splice 0..=k)"
                    );
                }
            }
            let dup_end_count = self
                .user_turns_list
                .iter()
                .filter(|t| user_turn_matches_covered_end(t, covered_end))
                .count();
            if dup_end_count > 1 {
                warn!(
                    covered_end_id = %result.covered_end_id,
                    matches = dup_end_count,
                    "apply_boundary: multiple UserTurns match covered_end_id (scenario 5 / id collision); using minimal index"
                );
            }
            (0usize, k)
        } else {
            return Err(AppError::ApplyBoundaryStale {
                covered_end_id: result.covered_end_id.clone(),
            });
        };

        let batch_chars: usize = self.user_turns_list[start..=end]
            .iter()
            .map(estimate_turn_chars)
            .sum();
        let summary_chars = result.summary_text.len();

        let summary_id = result
            .transcript_compaction_entry_id
            .clone()
            .unwrap_or_else(|| compound_turn_id(covered_start, covered_end));
        let summary_turn = TurnEntry::SummaryTurn {
            id: summary_id,
            summary: result.summary_text,
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        };

        self.user_turns_list.splice(start..=end, [summary_turn]);
        self.estimate_context_chars = self.estimate_context_chars.saturating_sub(batch_chars);
        self.estimate_context_chars += summary_chars;
        self.invalidate_api_usage();

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// estimate_turn_chars
// ---------------------------------------------------------------------------

/// 与 `ContextState::estimated_token_count` 的纯字符 fallback 一致：`chars / 4`。
#[inline]
pub fn estimated_tokens_from_chars(chars: usize) -> usize {
    chars / 4
}

/// 估算单个 TurnEntry 的字符数。
pub fn estimate_turn_chars(turn: &TurnEntry) -> usize {
    match turn {
        TurnEntry::UserTurn { messages, .. } => messages
            .iter()
            .map(|m| match m {
                AgentMessage::User { text } => text.len(),
                AgentMessage::Assistant { text, tool_calls } => {
                    text.len()
                        + tool_calls
                            .iter()
                            .map(|tc| tc.name.len() + tc.arguments.len() + tc.id.len() + 40)
                            .sum::<usize>()
                }
                AgentMessage::ToolResult { content, .. } => content.len(),
                AgentMessage::System { text } => text.len(),
                AgentMessage::Steering { text, .. } => text.len(),
                AgentMessage::CompactionSummary { summary } => summary.len(),
            })
            .sum(),
        TurnEntry::SummaryTurn { summary, .. } => summary.len(),
    }
}
