//! Context management data structures (TASK-17 / TASK-20).

use std::path::PathBuf;
use std::time::Instant;

use tokio::task::JoinHandle;

use crate::core::agent_loop::AgentMessage;
use crate::infra::error::AppError;

// ---------------------------------------------------------------------------
// TurnEntry
// ---------------------------------------------------------------------------

/// 上下文管理的分组单位：一条 user 消息及其后所有 assistant/tool 消息，
/// 或一条 Compaction 生成的结构化摘要。
#[derive(Debug, Clone)]
pub enum TurnEntry {
    UserTurn {
        id: String,
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
// CompactionSummary / CompactionResult (TASK-20)
// ---------------------------------------------------------------------------

/// 异步预热任务的产物对象。同一时间仅允许一个预热任务。
pub struct CompactionSummary {
    pub task_handle: JoinHandle<Result<CompactionResult, AppError>>,
    pub covered_start_id: String,
    pub covered_end_id: String,
    pub started_at: Instant,
}

/// 异步预热任务完成后的结果。
#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub summary_text: String,
    pub covered_start_id: String,
    pub covered_end_id: String,
    pub covered_count: usize,
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
    /// 异步预热单例：同一时间仅允许一个预热任务。
    pub compaction_summary: Option<CompactionSummary>,
    /// Legacy: Phase E (20.10) will remove this field.
    pub compaction_consecutive_failures: u32,
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

    /// 新 user turn 完成后追加到 turns 列表并更新估算。
    pub fn on_new_user_turn(&mut self, turn: TurnEntry) {
        let chars = estimate_turn_chars(&turn);
        self.estimate_context_chars += chars;
        self.post_usage_appended_chars += chars;
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

    /// 取消并清除正在运行的异步预热任务。幂等——无任务时为 no-op。
    pub fn abort_preheat(&mut self) {
        if let Some(pending) = self.compaction_summary.take() {
            pending.task_handle.abort();
        }
    }

    /// 将已完成的 CompactionResult 应用到 user_turns_list：
    /// 通过 ID 匹配找到 covered 范围，splice 替换为 SummaryTurn，
    /// 重算 estimate_context_chars，invalidate API usage。
    pub fn apply_boundary(&mut self, result: CompactionResult) -> Result<(), AppError> {
        let start_idx = self
            .user_turns_list
            .iter()
            .position(|t| t.id() == result.covered_start_id);
        let end_idx = self
            .user_turns_list
            .iter()
            .position(|t| t.id() == result.covered_end_id);

        let (start, end) = match (start_idx, end_idx) {
            (Some(s), Some(e)) if s <= e => (s, e),
            _ => {
                return Err(AppError::Config(
                    "apply_boundary: covered range not found in user_turns_list (IDs may have been invalidated by Layer 3)".to_string(),
                ));
            }
        };

        let batch_chars: usize = self.user_turns_list[start..=end]
            .iter()
            .map(estimate_turn_chars)
            .sum();
        let summary_chars = result.summary_text.len();

        let new_id = format!(
            "summary_{}_{}",
            chrono::Utc::now().timestamp_micros(),
            start
        );
        let summary_turn = TurnEntry::SummaryTurn {
            id: new_id,
            summary: result.summary_text,
            timestamp: chrono::Utc::now()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
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
