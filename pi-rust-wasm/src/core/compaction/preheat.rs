//! Layer 1 异步预热：后台 tokio task 生成 LLM 摘要 + 写入 transcript。
//!
//! `Preheat` struct 封装完整的预热状态机（Idle / Running / ExhaustedPending），
//! 外部仅通过 try_start / try_restart_if_pending / poll_result / await_result / abort
//! 等公共方法与之交互，不直接访问内部状态枚举。

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use tracing::warn;

use crate::core::llm::{ChatMessage, ChatRequest, LlmProvider};
use crate::core::session::manager::{
    estimate_turn_chars, estimated_tokens_from_chars, generate_entry_id, CompactionResult,
    TurnEntry,
};
use crate::core::session::transcript::{append_entry, CompactionEntry, TranscriptEntry};
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

    // --- 状态转换 ---

    /// Idle → Running。条件：ratio >= 0.50、有 turns、当前 Idle。
    /// spawn 内 generate_summary 最多 3 次 retry；
    /// 成功 emit AutoCompactionEnd，耗尽 emit CompactionError(exhausted)。
    /// 返回 true = 已启动。
    ///
    /// 接受独立参数而非 `&ContextState`，避免与 `ctx.preheat` 的 `&mut self` 冲突。
    pub fn try_start(
        &mut self,
        usage_ratio: f64,
        turns: &[TurnEntry],
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
        if turns.is_empty() {
            return false;
        }

        let snapshot = turns.to_vec();
        let first_id = snapshot.first().map(|t| t.id().to_string()).unwrap();
        let last_id = snapshot.last().map(|t| t.id().to_string()).unwrap();
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
                        let entry_id = generate_entry_id();
                        let covered_chars: usize = snapshot.iter().map(estimate_turn_chars).sum();
                        let est_covered_tok = estimated_tokens_from_chars(covered_chars);
                        let est_summary_tok = estimated_tokens_from_chars(summary_text.len());
                        let est_saved = est_covered_tok.saturating_sub(est_summary_tok);
                        let elapsed_ms = started.elapsed().as_millis() as u64;

                        let compaction_entry = TranscriptEntry::Compaction(CompactionEntry {
                            id: Some(entry_id.clone()),
                            parent_id: None,
                            timestamp: chrono::Utc::now()
                                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                            summary: Some(summary_text.clone()),
                            covered_start_id: Some(first_id.clone()),
                            covered_end_id: Some(last_id.clone()),
                            covered_count: Some(covered_count),
                            is_boundary: Some(false),
                            preheat_compaction_id: Some(entry_id.clone()),
                            estimated_covered_tokens_before: Some(est_covered_tok),
                            estimated_summary_tokens: Some(est_summary_tok),
                            estimated_tokens_saved: Some(est_saved),
                        });

                        let transcript_compaction_entry_id =
                            if transcript_path.as_os_str().is_empty() {
                                None
                            } else {
                                match append_entry(&transcript_path, &compaction_entry) {
                                    Ok(()) => Some(entry_id),
                                    Err(e) => {
                                        warn!("preheat append_entry failed: {}", e);
                                        None
                                    }
                                }
                            };

                        return Ok(CompactionResult {
                            summary_text,
                            covered_start_id: first_id,
                            covered_end_id: last_id,
                            covered_count,
                            transcript_compaction_entry_id,
                            estimated_covered_tokens_before: Some(est_covered_tok),
                            estimated_summary_tokens: Some(est_summary_tok),
                            estimated_tokens_saved: Some(est_saved),
                            preheat_elapsed_ms: elapsed_ms,
                        });
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

        self.state = PreheatState::Running {
            handle,
            covered_start_id: turns.first().unwrap().id().to_string(),
            covered_end_id: turns.last().unwrap().id().to_string(),
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
        turns: &[TurnEntry],
        transcript_path: &std::path::Path,
        llm: Arc<dyn LlmProvider>,
        config: &ContextConfig,
        event_bus: Arc<dyn EventBus>,
    ) -> bool {
        if !self.is_exhausted_pending() {
            return false;
        }
        self.state = PreheatState::Idle;
        self.try_start(usage_ratio, turns, transcript_path, llm, config, event_bus)
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

/// 根据 turns snapshot 生成 LLM 摘要（首次或 UPDATE 模式）。
pub async fn generate_summary(
    snapshot: &[TurnEntry],
    previous_summary: Option<&str>,
    llm: &dyn LlmProvider,
    compaction_model: &str,
) -> Result<String, AppError> {
    let batch_text = turns_to_text(snapshot);

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

fn find_last_summary(turns: &[TurnEntry]) -> Option<String> {
    turns.iter().rev().find_map(|t| {
        if let TurnEntry::SummaryTurn { summary, .. } = t {
            Some(summary.clone())
        } else {
            None
        }
    })
}

fn turns_to_text(turns: &[TurnEntry]) -> String {
    let mut buf = String::new();
    for turn in turns {
        match turn {
            TurnEntry::UserTurn { messages, .. } => {
                for msg in messages {
                    match msg {
                        crate::core::agent_loop::AgentMessage::User { text } => {
                            buf.push_str("[User] ");
                            buf.push_str(text);
                            buf.push('\n');
                        }
                        crate::core::agent_loop::AgentMessage::Assistant { text, .. } => {
                            buf.push_str("[Assistant] ");
                            buf.push_str(text);
                            buf.push('\n');
                        }
                        crate::core::agent_loop::AgentMessage::ToolResult { content, .. } => {
                            buf.push_str("[ToolResult] ");
                            let preview = if content.len() > 200 {
                                let end = floor_char_boundary(content, 200);
                                &content[..end]
                            } else {
                                content
                            };
                            buf.push_str(preview);
                            buf.push('\n');
                        }
                        _ => {}
                    }
                }
            }
            TurnEntry::SummaryTurn { summary, .. } => {
                buf.push_str("[Previous Summary]\n");
                buf.push_str(summary);
                buf.push('\n');
            }
        }
    }
    buf
}
