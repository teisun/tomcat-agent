//! Layer 1 异步预热：后台 tokio task 生成 LLM 摘要 + 写入 transcript。
//!
//! `Preheat` struct 封装完整的预热状态机（Idle / Running / ExhaustedPending），
//! 外部仅通过 try_start / try_restart_if_pending / poll_result / await_result / abort
//! 等公共方法与之交互，不直接访问内部状态枚举。

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use tracing::warn;

use crate::core::llm::{ChatMessage, ChatMessageRole, ChatRequest, LlmProvider, MessageKind};
use crate::core::session::manager::{
    compound_turn_id, estimate_msg_chars, estimated_tokens_from_chars, CompactionResult,
};
use crate::core::session::transcript::{
    insert_entry_after_message_id, BranchSummaryEntry, TranscriptEntry,
};
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;
use crate::infra::event_bus::{EventBus, EventContext};
use crate::infra::events::AgentEvent;

use super::truncation::floor_char_boundary;

const MAX_PREHEAT_RETRIES: u32 = 3;

// ---------------------------------------------------------------------------
// Prompt templates (aligned with context-management.md §7.1 / §7.3)
// ---------------------------------------------------------------------------

const SUMMARIZATION_PROMPT: &str = r#"You are a context compaction assistant. Summarize the following conversation segment into a structured format under ~8K tokens. Preserve all critical information needed for the AI assistant to continue working effectively.

Output format:
## Goal
What the user is trying to accomplish.

## Constraints
Any rules, preferences, or constraints mentioned.

## Progress
What has been done so far (key actions, tool calls, results).

### In Progress
Current tasks that are underway but not yet completed.

### Blocked
Tasks that cannot proceed and their reasons.

## Key Decisions
Important decisions made and their rationale.

## Critical Context
File paths, variable names, error messages, or other specific details that must be preserved.

## Next Steps
What should happen next based on the conversation."#;

const UPDATE_SUMMARIZATION_PROMPT: &str = r#"You are a context compaction assistant. You have an existing summary and a new conversation segment. Merge them into a single updated summary under ~8K tokens, keeping the same structured format. Drop information that is no longer relevant, and add new information from the recent segment.

Existing summary:
{existing_summary}

Output format:
## Goal
## Constraints
## Progress
### In Progress
### Blocked
## Key Decisions
## Critical Context
## Next Steps"#;

// ---------------------------------------------------------------------------
// PreheatState (internal — not pub)
// ---------------------------------------------------------------------------

enum PreheatState {
    Idle,
    /// Reload：磁盘上已有未消费的 preheat 摘要，下一轮 `poll_result` 直接返回。
    CachedCompleted {
        result: CompactionResult,
    },
    Running {
        handle: JoinHandle<Result<CompactionResult, AppError>>,
        #[allow(dead_code)]
        covered_start_id: String,
        #[allow(dead_code)]
        covered_end_id: String,
        #[allow(dead_code)]
        covered_count: usize,
        started_at: Instant,
    },
    ExhaustedPending,
}

// ---------------------------------------------------------------------------
// PreheatOutcome (public)
// ---------------------------------------------------------------------------

/// poll_result / await_result 的返回值。
#[derive(Debug)]
pub enum PreheatOutcome {
    /// 摘要生成成功，调用方应 apply_boundary。
    Completed(CompactionResult),
    /// 任务尚未完成，或当前非 Running 状态。
    NotReady,
    /// 3 次 retry 全部失败，已转入 ExhaustedPending。
    Exhausted,
    /// JoinHandle panic 或其他非预期错误，已转入 Idle。
    Failed,
}

// ---------------------------------------------------------------------------
// Preheat (public struct)
// ---------------------------------------------------------------------------

/// 异步预热状态机。外部通过方法与之交互，内部状态枚举不可见。
pub struct Preheat {
    state: PreheatState,
}

impl std::fmt::Debug for Preheat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match &self.state {
            PreheatState::Idle => "Idle",
            PreheatState::CachedCompleted { .. } => "CachedCompleted",
            PreheatState::Running { .. } => "Running",
            PreheatState::ExhaustedPending => "ExhaustedPending",
        };
        f.debug_struct("Preheat").field("state", &label).finish()
    }
}

impl Default for Preheat {
    fn default() -> Self {
        Self::new()
    }
}

impl Preheat {
    pub fn new() -> Self {
        Self {
            state: PreheatState::Idle,
        }
    }

    // --- 查询 ---

    pub fn is_idle(&self) -> bool {
        matches!(self.state, PreheatState::Idle)
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state, PreheatState::Running { .. })
    }

    /// LLM 摘要任务仍在执行（`Running` 且 JoinHandle 未完成）。
    pub fn is_warmup_task_active(&self) -> bool {
        matches!(
            &self.state,
            PreheatState::Running { handle, .. } if !handle.is_finished()
        )
    }

    /// 摘要已在内存/磁盘就绪，尚未被 `poll_result` 消费并进入 apply。
    pub fn preheat_result_pending(&self) -> bool {
        match &self.state {
            PreheatState::CachedCompleted { .. } => true,
            PreheatState::Running { handle, .. } => handle.is_finished(),
            _ => false,
        }
    }

    pub fn is_exhausted_pending(&self) -> bool {
        matches!(self.state, PreheatState::ExhaustedPending)
    }

    /// `CachedCompleted`（reload 恢复）或 Running 且 JoinHandle 已完成。
    pub fn is_finished(&self) -> bool {
        match &self.state {
            PreheatState::CachedCompleted { .. } => true,
            PreheatState::Running { handle, .. } => handle.is_finished(),
            _ => false,
        }
    }

    pub fn started_at(&self) -> Option<Instant> {
        match &self.state {
            PreheatState::Running { started_at, .. } => Some(*started_at),
            _ => None,
        }
    }

    /// Idle 或已有 `CachedCompleted` 时注入磁盘恢复的摘要；Running / ExhaustedPending 时忽略。
    pub fn restore_completed(&mut self, result: CompactionResult) {
        match self.state {
            PreheatState::Idle | PreheatState::CachedCompleted { .. } => {
                self.state = PreheatState::CachedCompleted { result };
            }
            PreheatState::Running { .. } | PreheatState::ExhaustedPending => {}
        }
    }

    /// `poll_result` 已交出 `CompactionResult` 且 `apply_boundary` 失败时调用：回到 `CachedCompleted`，
    /// 以便后续重试 apply，并避免 `Preheat` 误留在 `Idle` 导致 timing ⑤ 再次 `try_start`、叠未应用摘要。
    pub(crate) fn restore_pending_result(&mut self, result: CompactionResult) {
        match self.state {
            PreheatState::Idle => {
                self.state = PreheatState::CachedCompleted { result };
            }
            _ => {
                warn!(
                    "restore_pending_result: expected Idle after failed apply, state={:?}",
                    self
                );
            }
        }
    }

    /// 防御性丢弃尚未 `poll_result` 的完成态（陈旧 apply 等路径）；**仅** `CachedCompleted` → `Idle`。
    pub fn discard_cached_completed(&mut self) {
        if matches!(self.state, PreheatState::CachedCompleted { .. }) {
            self.state = PreheatState::Idle;
        }
    }

    // --- 状态转换 ---

    /// Idle → Running。条件：ratio >= 0.50、有 messages、且当前为 **Idle**。
    /// `CachedCompleted` / `Running` / `ExhaustedPending` 时均不启动，避免已有未消费摘要时又开新预热。
    /// spawn 内 generate_summary 最多 3 次 retry；
    /// 成功且 `insert_entry_after_message_id` 成功（或无 transcript 路径）时 emit AutoCompactionEnd；耗尽 emit CompactionError(exhausted)。
    /// 返回 true = 已启动。
    ///
    /// 接受独立参数而非 `&ContextState`，避免与 `ctx.preheat` 的 `&mut self` 冲突。
    pub fn try_start(
        &mut self,
        usage_ratio: f64,
        messages: &[ChatMessage],
        transcript_path: &std::path::Path,
        llm: Arc<dyn LlmProvider>,
        config: &ContextConfig,
        event_bus: Arc<dyn EventBus>,
    ) -> bool {
        if !self.is_idle() {
            return false;
        }
        if usage_ratio < 0.50 {
            return false;
        }
        if messages.is_empty() {
            return false;
        }

        let snapshot = messages.to_vec();
        let Some((covered_start_id, covered_end_id)) =
            snapshot_message_bounds_for_preheat(&snapshot)
        else {
            return false;
        };
        let batch_compaction_id = compound_turn_id(&covered_start_id, &covered_end_id);
        let covered_count = snapshot.len();
        let transcript_path = transcript_path.to_path_buf();
        let compaction_model = config.compaction_model.clone();
        let ratio_before = usage_ratio;

        let existing_summary = find_last_summary(&snapshot);

        let eb = event_bus.clone();
        let handle = tokio::spawn(async move {
            let started = Instant::now();
            let mut last_error = String::new();

            for attempt in 1..=MAX_PREHEAT_RETRIES {
                match generate_summary(
                    &snapshot,
                    existing_summary.as_deref(),
                    &*llm,
                    &compaction_model,
                )
                .await
                {
                    Ok(summary_text) => {
                        let covered_chars: usize = snapshot.iter().map(estimate_msg_chars).sum();
                        let est_covered_tok = estimated_tokens_from_chars(covered_chars);
                        let est_summary_tok = estimated_tokens_from_chars(summary_text.len());
                        let est_saved = est_covered_tok.saturating_sub(est_summary_tok);
                        let elapsed_ms = started.elapsed().as_millis() as u64;

                        let branch_summary_entry =
                            TranscriptEntry::BranchSummary(BranchSummaryEntry {
                                id: Some(batch_compaction_id.clone()),
                                parent_id: None,
                                timestamp: chrono::Utc::now()
                                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                                summary: Some(summary_text.clone()),
                                covered_start_id: Some(covered_start_id.clone()),
                                covered_end_id: Some(covered_end_id.clone()),
                                covered_count: Some(covered_count),
                                is_boundary: Some(false),
                                preheat_compaction_id: Some(batch_compaction_id.clone()),
                                estimated_covered_tokens_before: Some(est_covered_tok),
                                estimated_summary_tokens: Some(est_summary_tok),
                                estimated_tokens_saved: Some(est_saved),
                            });

                        let (transcript_compaction_entry_id, append_ok) = if transcript_path
                            .as_os_str()
                            .is_empty()
                        {
                            (Some(batch_compaction_id.clone()), true)
                        } else {
                            match insert_entry_after_message_id(
                                &transcript_path,
                                &covered_end_id,
                                &branch_summary_entry,
                            ) {
                                Ok(()) => (Some(batch_compaction_id.clone()), true),
                                Err(e) => {
                                    warn!("preheat insert_entry_after_message_id failed: {}", e);
                                    (None, false)
                                }
                            }
                        };

                        let result = CompactionResult {
                            summary_text,
                            covered_start_id,
                            covered_end_id,
                            covered_count,
                            transcript_compaction_entry_id,
                            estimated_covered_tokens_before: Some(est_covered_tok),
                            estimated_summary_tokens: Some(est_summary_tok),
                            estimated_tokens_saved: Some(est_saved),
                            preheat_elapsed_ms: elapsed_ms,
                        };

                        if append_ok {
                            emit_agent_event(
                                &*eb,
                                AgentEvent::AutoCompactionEnd {
                                    elapsed_ms,
                                    summary_chars: result.summary_text.len(),
                                    covered_count,
                                    ratio_after: ratio_before,
                                    estimated_covered_tokens_before: est_covered_tok,
                                    estimated_summary_tokens: est_summary_tok,
                                    estimated_tokens_saved: est_saved,
                                },
                            );
                        }

                        return Ok(result);
                    }
                    Err(e) => {
                        last_error = e.to_string();
                        warn!(
                            "preheat attempt {}/{} failed: {}",
                            attempt, MAX_PREHEAT_RETRIES, last_error
                        );
                    }
                }
            }

            emit_agent_event(
                &*eb,
                AgentEvent::CompactionError {
                    exhausted_after_retries: true,
                    attempts: MAX_PREHEAT_RETRIES,
                    error: last_error.clone(),
                    source: "preheat".to_string(),
                    ratio: Some(ratio_before),
                },
            );

            Err(AppError::Llm(format!(
                "preheat exhausted after {} retries: {}",
                MAX_PREHEAT_RETRIES, last_error
            )))
        });

        let Some((run_s, run_e)) = snapshot_message_bounds_for_preheat(messages) else {
            return false;
        };
        self.state = PreheatState::Running {
            handle,
            covered_start_id: run_s,
            covered_end_id: run_e,
            covered_count,
            started_at: Instant::now(),
        };

        true
    }

    /// ExhaustedPending → Running（条件：ratio >= 0.50）。
    /// 内部先转 Idle 再调 try_start。
    pub fn try_restart_if_pending(
        &mut self,
        usage_ratio: f64,
        messages: &[ChatMessage],
        transcript_path: &std::path::Path,
        llm: Arc<dyn LlmProvider>,
        config: &ContextConfig,
        event_bus: Arc<dyn EventBus>,
    ) -> bool {
        if !self.is_exhausted_pending() {
            return false;
        }
        self.state = PreheatState::Idle;
        self.try_start(usage_ratio, messages, transcript_path, llm, config, event_bus)
    }

    /// 非阻塞获取结果。CachedCompleted → Idle + Completed；
    /// Running(finished) → Idle + Completed；
    /// Running(exhausted Err) → ExhaustedPending + Exhausted；
    /// Running(panic) → Idle + Failed；其他情况 → NotReady。
    pub fn poll_result(&mut self) -> PreheatOutcome {
        if matches!(self.state, PreheatState::CachedCompleted { .. }) {
            let old = std::mem::replace(&mut self.state, PreheatState::Idle);
            return match old {
                PreheatState::CachedCompleted { result } => PreheatOutcome::Completed(result),
                _ => PreheatOutcome::NotReady,
            };
        }

        let is_finished = matches!(
            self.state,
            PreheatState::Running { ref handle, .. } if handle.is_finished()
        );
        if !is_finished {
            return PreheatOutcome::NotReady;
        }

        let old = std::mem::replace(&mut self.state, PreheatState::Idle);
        match old {
            PreheatState::Running { handle, .. } => {
                match futures_util::FutureExt::now_or_never(handle) {
                    Some(Ok(Ok(result))) => PreheatOutcome::Completed(result),
                    Some(Ok(Err(_e))) => {
                        self.state = PreheatState::ExhaustedPending;
                        PreheatOutcome::Exhausted
                    }
                    Some(Err(e)) => {
                        warn!("preheat task panicked: {}", e);
                        PreheatOutcome::Failed
                    }
                    None => PreheatOutcome::NotReady,
                }
            }
            _ => PreheatOutcome::NotReady,
        }
    }

    /// 阻塞等待结果（带超时），用于 ratio >= 0.98 的同步等待路径。
    pub async fn await_result(&mut self, timeout: Duration) -> PreheatOutcome {
        if matches!(self.state, PreheatState::CachedCompleted { .. }) {
            let old = std::mem::replace(&mut self.state, PreheatState::Idle);
            return match old {
                PreheatState::CachedCompleted { result } => PreheatOutcome::Completed(result),
                _ => PreheatOutcome::NotReady,
            };
        }

        let is_running = matches!(self.state, PreheatState::Running { .. });
        if !is_running {
            return PreheatOutcome::NotReady;
        }

        let old = std::mem::replace(&mut self.state, PreheatState::Idle);
        match old {
            PreheatState::Running { handle, .. } => {
                match tokio::time::timeout(timeout, handle).await {
                    Ok(Ok(Ok(result))) => PreheatOutcome::Completed(result),
                    Ok(Ok(Err(_e))) => {
                        self.state = PreheatState::ExhaustedPending;
                        PreheatOutcome::Exhausted
                    }
                    Ok(Err(e)) => {
                        warn!("preheat task panicked during await: {}", e);
                        PreheatOutcome::Failed
                    }
                    Err(_) => {
                        warn!("preheat timed out after {:?}, clearing", timeout);
                        PreheatOutcome::Failed
                    }
                }
            }
            _ => PreheatOutcome::NotReady,
        }
    }

    /// any → Idle。取消运行中任务 + 清除 pending。
    pub fn abort(&mut self) {
        if let PreheatState::Running { handle, .. } =
            std::mem::replace(&mut self.state, PreheatState::Idle)
        {
            handle.abort();
        }
    }
}

// ---------------------------------------------------------------------------
// generate_summary
// ---------------------------------------------------------------------------

/// 根据 messages snapshot 生成 LLM 摘要（首次或 UPDATE 模式）。
pub async fn generate_summary(
    snapshot: &[ChatMessage],
    previous_summary: Option<&str>,
    llm: &dyn LlmProvider,
    compaction_model: &str,
) -> Result<String, AppError> {
    let batch_text = messages_to_text(snapshot);

    let prompt = if let Some(existing) = previous_summary {
        UPDATE_SUMMARIZATION_PROMPT.replace("{existing_summary}", existing)
    } else {
        SUMMARIZATION_PROMPT.to_string()
    };

    let req = ChatRequest {
        model: compaction_model.to_string(),
        messages: vec![ChatMessage::system(&prompt), ChatMessage::user(&batch_text)],
        stream: Some(false),
        ..Default::default()
    };

    let resp = llm.chat(req).await?;
    let text = resp
        .choices
        .first()
        .and_then(|c| c.message.text_content())
        .unwrap_or("")
        .to_string();

    if text.is_empty() {
        return Err(AppError::internal("LLM returned empty summary"));
    }

    Ok(text)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// §5.7：`insert_entry_after_message_id` 的锚点必须是 **MessageId**（transcript MessageEntry 的 id），
/// 不能是 CompactionSummary 消息的 msg_id（那是 BranchSummary entry 的 id）。
/// 因此跳过 CompactionSummary 消息，取首个与最后一个普通消息的 msg_id。
fn snapshot_message_bounds_for_preheat(messages: &[ChatMessage]) -> Option<(String, String)> {
    let first_start = messages.iter().find_map(|m| {
        if m.kind != MessageKind::CompactionSummary {
            m.msg_id.clone()
        } else {
            None
        }
    })?;
    let last_end = messages.iter().rev().find_map(|m| {
        if m.kind != MessageKind::CompactionSummary {
            m.msg_id.clone()
        } else {
            None
        }
    })?;
    Some((first_start, last_end))
}

fn emit_agent_event(event_bus: &dyn EventBus, event: AgentEvent) {
    let payload = serde_json::to_value(&event).unwrap_or(serde_json::Value::Null);
    let event_name = payload
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let ctx = EventContext::new(event_name.clone(), payload);
    let _ = event_bus.emit_sync(&event_name, ctx);
}

fn find_last_summary(messages: &[ChatMessage]) -> Option<String> {
    messages.iter().rev().find_map(|m| {
        if m.kind == MessageKind::CompactionSummary {
            m.text_content().map(|s| s.to_string())
        } else {
            None
        }
    })
}

fn messages_to_text(messages: &[ChatMessage]) -> String {
    let mut buf = String::new();
    for m in messages {
        match m.kind {
            MessageKind::CompactionSummary => {
                buf.push_str("[Previous Summary]\n");
                if let Some(text) = m.text_content() {
                    buf.push_str(text);
                    buf.push('\n');
                }
            }
            _ => match m.role {
                ChatMessageRole::User => {
                    buf.push_str("[User] ");
                    if let Some(text) = m.text_content() {
                        buf.push_str(text);
                    }
                    buf.push('\n');
                }
                ChatMessageRole::Assistant => {
                    buf.push_str("[Assistant] ");
                    if let Some(text) = m.text_content() {
                        buf.push_str(text);
                    }
                    buf.push('\n');
                }
                ChatMessageRole::Tool => {
                    buf.push_str("[ToolResult] ");
                    if let Some(text) = m.text_content() {
                        let preview = if text.len() > 200 {
                            let end = floor_char_boundary(text, 200);
                            &text[..end]
                        } else {
                            text
                        };
                        buf.push_str(preview);
                    }
                    buf.push('\n');
                }
                _ => {}
            },
        }
    }
    buf
}

#[cfg(test)]
mod snapshot_message_bounds_tests {
    use super::snapshot_message_bounds_for_preheat;
    use crate::core::llm::{ChatMessage, ChatMessageContent, ChatMessageRole, MessageKind};

    fn normal_msg(id: &str) -> ChatMessage {
        ChatMessage {
            role: ChatMessageRole::User,
            content: Some(ChatMessageContent::Text("text".into())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            msg_id: Some(id.to_string()),
            kind: MessageKind::Normal,
            timestamp: Some("ts".into()),
        }
    }

    fn summary_msg(id: &str) -> ChatMessage {
        let mut m = ChatMessage::compaction_summary("prev");
        m.msg_id = Some(id.to_string());
        m
    }

    #[test]
    fn skips_leading_summary_turn() {
        let messages = vec![
            summary_msg("batch_S::batch_E"),
            normal_msg("m0"),
            normal_msg("m1"),
            normal_msg("m2"),
            normal_msg("m3"),
        ];
        let (s, e) = snapshot_message_bounds_for_preheat(&messages).unwrap();
        assert_eq!(s, "m0");
        assert_eq!(e, "m3");
    }

    #[test]
    fn skips_trailing_summary_turn() {
        let messages = vec![
            normal_msg("a"),
            normal_msg("b"),
            summary_msg("x::y"),
        ];
        let (s, e) = snapshot_message_bounds_for_preheat(&messages).unwrap();
        assert_eq!(s, "a");
        assert_eq!(e, "b");
    }

    #[test]
    fn none_when_no_normal_message() {
        let messages = vec![summary_msg("only::summary")];
        assert!(snapshot_message_bounds_for_preheat(&messages).is_none());
    }
}
