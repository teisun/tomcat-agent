//! Background compaction (A). The foreground validates the durable tail and appends
//! a cut marker before taking a single working-set snapshot for the spawned task.
//!
//! ```text
//! Idle ── start ──► Running ── settle success ──► CachedCompleted ── take ──► Idle
//!                      │                               ▲
//!                      └── settle failure ──► ExhaustedPending (retry at turn boundaries)
//!                                    reload cache ─────┘
//! ```
//!
//! The task generates one summary; provider transport owns any network retries. On
//! success it writes the crash cache and emits a diagnostic completion event. Applying
//! the ready summary writes its body to the existing transcript marker.
//!
//! We only await A when foreground compaction would otherwise start. Stop preserves
//! A; a handoff timeout aborts and reaps it before another summary is allowed to start.

use std::path::Path;
use std::sync::Arc;

use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use tracing::warn;

use crate::core::llm::{ChatMessage, ChatMessageRole, ChatRequest, LlmProvider, MessageKind};
use crate::core::plan_runtime::ControlSnapshot;
use crate::core::session::manager::{
    compound_turn_id, estimate_msg_chars, estimated_tokens_from_chars, CompactionResult,
};
use crate::core::session::preheat_cache::write_preheat_cache;
use crate::core::session::transcript::{
    append_entry, entry_id, read_entries_tail, BranchSummaryEntry, CustomEntry, TranscriptEntry,
};
use crate::core::session::user_message_sidecar::{
    ensure_user_message_sidecar_current, recent_user_message_texts,
};

use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;
use crate::infra::event_bus::ScopedEventEmitter;
use crate::infra::events::AgentEvent;

use super::machine_block;
use super::truncation::floor_char_boundary;

const PREHEAT_TAIL_MESSAGE_SCAN: usize = 256;
/// 生成摘要时随调用携带的可选运行时资料，避免入口继续膨胀位置参数。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SummaryRequestOptions<'a> {
    pub cache_key: Option<&'a str>,
    pub resolved_output_limit: Option<u32>,
    pub input_budget_tokens: Option<usize>,
    pub transcript_path: Option<&'a Path>,
}

// ---------------------------------------------------------------------------
// Prompt templates (T2-P0-002 Phase B — 8 节模板，唯一来源：
//   - docs/reports/compaction-prompt-cc-vs-pi.md §5.3 (BASE)
//   - docs/reports/compaction-prompt-cc-vs-pi.md §5.4 (UPDATE)
//
// 设计要点（详见 docs/reports/compaction-prompt-cc-vs-pi.md §5.5 与 §5.7.1）：
//   1. 首行固定 `Respond with text only. Do not call any tools.` —— 防止部分 provider
//      在摘要场景误调工具，与 generate_summary 中 ChatRequest.tools = None 形成双保险。
//   2. 指令区追加 `First reason internally, then output the final summary.` —— Two-pass
//      decision freeze（关闭 #T-044）的替代策略，让模型走内部隐式推理，避免双轮草稿翻倍 token。
//   3. 分节结构 + Next Steps verbatim 引用，让下一轮 LLM 能从摘要直接接力。
//      用户原话不再让模型复述（模型会编），改由 `machine_block` 在模型产出后由代码逐字拼接。
//   4. 历史模板对齐 context-management.md §7.1 / §7.3 仍保留，Phase G 由
//      `impl-G-arch-spec-doc` 在 `Compaction v2（T2-P0-002）` 小节统一记录。
// ---------------------------------------------------------------------------

pub(super) const SUMMARIZATION_PROMPT: &str = r#"Respond with text only. Do not call any tools.

Create a structured context checkpoint that another LLM will use to continue the work.
The entire summary should be under ~8K tokens. Prioritize actionable information.

First reason internally, then output the final summary.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items.]

## Progress
### Done
- [x] [Completed task] (file: path/to/file, if applicable)
- [x] ...

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Errors Encountered
- **[Error description]**: [How it was fixed / current status]
- [Or "(none)" if no errors]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Most immediate next step. Include a short quote from the latest conversation showing what was being worked on.]
2. [Subsequent steps]

## Critical Context
- [Any data, file paths, variable names, error messages, or references needed to continue]
- [Or "(none)" if not applicable]"#;

pub(super) const UPDATE_SUMMARIZATION_PROMPT: &str = r#"Respond with text only. Do not call any tools.

Update the existing structured summary with new information. The output REPLACES the old summary entirely.

First reason internally, then output the final summary.

Existing summary:
{existing_summary}

RULES:
- PRESERVE information from the previous summary that is still relevant
- ADD new progress, decisions, errors, and context from the new messages
- UPDATE Progress: move items from "In Progress" to "Done" when completed
- UPDATE "Next Steps" to reflect the latest state
- REMOVE information that is no longer relevant to free space
- The complete updated summary should be under ~8K tokens
- When the old summary is already large, compress older details to stay within budget
- PRESERVE exact file paths, function names, and error messages

Use the EXACT same format as the original summary (Goal / Progress / Errors Encountered / Key Decisions / Next Steps / Critical Context)."#;

// ---------------------------------------------------------------------------
// PreheatState (internal — not pub)
// ---------------------------------------------------------------------------

#[derive(Default)]
enum PreheatState {
    #[default]
    Idle,
    /// Reload：磁盘上已有未消费的 preheat 摘要，下一轮 `poll_result` 直接返回。
    CachedCompleted {
        result: CompactionResult,
    },
    Running {
        handle: JoinHandle<Result<CompactionResult, AppError>>,
        transcript_path: std::path::PathBuf,
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

fn append_preheat_terminal_event(
    transcript_path: &Path,
    event: &'static str,
    error: Option<String>,
) {
    if transcript_path.as_os_str().is_empty() {
        return;
    }
    let mut extra = serde_json::json!({
        "event": event,
        "source": "preheat",
    });
    if let Some(error) = error {
        extra["error"] = serde_json::Value::String(error);
    }
    let append_result = crate::infra::config::lock::with_config_lock(transcript_path, || {
        append_entry(
            transcript_path,
            &TranscriptEntry::Custom(CustomEntry {
                id: None,
                parent_id: None,
                timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                extra,
            }),
        )
    });
    if let Err(error) = append_result {
        warn!(%error, transcript = %transcript_path.display(), "failed to persist preheat terminal diagnostic");
    }
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
    /// The task failed and can be retried at a turn boundary.
    Exhausted,
    /// Timed out and reaped; the task is no longer running.
    Failed,
}

// ---------------------------------------------------------------------------
// Preheat (public struct)
// ---------------------------------------------------------------------------

/// 异步预热状态机。外部通过方法与之交互，内部状态枚举不可见。
pub struct Preheat {
    state: PreheatState,
    #[cfg(test)]
    snapshot_copies: usize,
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
            #[cfg(test)]
            snapshot_copies: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn snapshot_copies(&self) -> usize {
        self.snapshot_copies
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

    /// A usable summary is cached, but has not yet been applied.
    pub fn preheat_result_pending(&self) -> bool {
        self.is_finished()
    }

    pub fn is_exhausted_pending(&self) -> bool {
        matches!(self.state, PreheatState::ExhaustedPending)
    }

    /// Check after `settle`: a completed task does not necessarily have a usable summary.
    pub fn is_finished(&self) -> bool {
        matches!(self.state, PreheatState::CachedCompleted { .. })
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
    /// The foreground appends its cut marker before spawn. The spawned task only computes the
    /// summary (and optional crash cache), so it can never race foreground transcript appends.
    /// Transport retries belong to the provider; this task generates one summary.
    /// 返回 true = 已启动。
    ///
    /// 接受独立参数而非 `&ContextState`，避免与 `ctx.preheat` 的 `&mut self` 冲突。
    #[allow(clippy::too_many_arguments)]
    pub fn try_start(
        &mut self,
        usage_ratio: f64,
        messages: &[ChatMessage],
        transcript_path: &std::path::Path,
        cache_key: Option<String>,
        llm: Arc<dyn LlmProvider>,
        resolved_output_limit: Option<u32>,
        input_budget_tokens: Option<usize>,
        config: &ContextConfig,
        emitter: Arc<ScopedEventEmitter>,
        control: Option<ControlSnapshot>,
    ) -> bool {
        if !self.is_idle() {
            return false;
        }
        self.launch(
            usage_ratio,
            messages,
            transcript_path,
            cache_key,
            llm,
            resolved_output_limit,
            input_budget_tokens,
            config,
            emitter,
            control,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn launch(
        &mut self,
        usage_ratio: f64,
        messages: &[ChatMessage],
        transcript_path: &Path,
        cache_key: Option<String>,
        llm: Arc<dyn LlmProvider>,
        resolved_output_limit: Option<u32>,
        input_budget_tokens: Option<usize>,
        config: &ContextConfig,
        emitter: Arc<ScopedEventEmitter>,
        control: Option<ControlSnapshot>,
    ) -> bool {
        if usage_ratio < 0.50 {
            return false;
        }
        let Some((covered_start_id, covered_end_id)) =
            snapshot_message_bounds_for_preheat(messages)
        else {
            return false;
        };
        let batch_compaction_id = compound_turn_id(&covered_start_id, &covered_end_id);
        let covered_count = messages.len();

        // This is the only moment where appending a boundary marker also puts it exactly at the
        // semantic cut: the snapshot's covered end is still the latest durable message. Custom
        // diagnostic rows may follow that message during a tool loop and do not change the
        // conversation boundary. Never defer this write into the background task, where
        // intervening message rows would make it a costly mid-file insertion.
        if !transcript_path.as_os_str().is_empty() {
            let _guard = match crate::infra::config::lock::acquire_config_lock(transcript_path) {
                Ok(guard) => guard,
                Err(error) => {
                    warn!(%error, "could not lock transcript for preheat marker");
                    return false;
                }
            };
            let covered_end_is_latest_message =
                match read_entries_tail(transcript_path, PREHEAT_TAIL_MESSAGE_SCAN) {
                    Ok(entries) => entries
                        .iter()
                        .rev()
                        .find(|entry| matches!(entry, TranscriptEntry::Message(_)))
                        .and_then(entry_id)
                        .is_some_and(|id| id == covered_end_id),
                    Err(error) => {
                        warn!(
                            transcript = %transcript_path.display(),
                            %error,
                            "could not verify preheat marker placement"
                        );
                        false
                    }
                };
            if !covered_end_is_latest_message {
                warn!(
                    transcript = %transcript_path.display(),
                    covered_end_id,
                    "preheat snapshot is no longer the transcript tail; not creating a misplaced marker"
                );
                return false;
            }
            let marker = TranscriptEntry::BranchSummary(BranchSummaryEntry {
                id: Some(batch_compaction_id.clone()),
                parent_id: None,
                timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                summary: None,
                covered_start_id: Some(covered_start_id.clone()),
                covered_end_id: Some(covered_end_id.clone()),
                covered_count: Some(covered_count),
                is_boundary: Some(true),
                preheat_compaction_id: Some(batch_compaction_id.clone()),
                estimated_covered_tokens_before: None,
                estimated_summary_tokens: None,
                estimated_tokens_saved: None,
                error: None,
                attempts: None,
                superseded: false,
            });
            if let Err(error) = append_entry(transcript_path, &marker) {
                warn!(
                    transcript = %transcript_path.display(),
                    %error,
                    "preheat marker append failed; not starting an unpersistable preheat"
                );
                return false;
            }
        }

        #[cfg(test)]
        {
            self.snapshot_copies += 1;
        }
        let snapshot = messages.to_vec();
        let transcript_path = transcript_path.to_path_buf();
        let compaction_model = config.compaction_model.clone();
        let running_start_id = covered_start_id.clone();
        let running_end_id = covered_end_id.clone();
        let ratio_before = usage_ratio;
        let cache_key = cache_key.unwrap_or_default();
        let running_transcript_path = transcript_path.clone();

        let existing_summary = find_last_summary(&snapshot);

        let eb = emitter.clone();
        let handle = tokio::spawn(async move {
            let started = Instant::now();
            let summary_text = match generate_summary_with_output_limit(
                &snapshot,
                existing_summary.as_deref(),
                &*llm,
                &compaction_model,
                control.as_ref(),
                SummaryRequestOptions {
                    cache_key: (!cache_key.is_empty()).then_some(cache_key.as_str()),
                    resolved_output_limit,
                    input_budget_tokens,
                    transcript_path: (!transcript_path.as_os_str().is_empty())
                        .then_some(transcript_path.as_path()),
                },
            )
            .await
            {
                Ok(summary) => summary,
                Err(error) => {
                    warn!(%error, "background compaction failed");
                    let detail = error.to_string();
                    let _ = eb.emit(AgentEvent::CompactionError {
                        exhausted_after_retries: false,
                        attempts: 1,
                        error: detail.clone(),
                        source: "preheat".to_string(),
                        ratio: Some(ratio_before),
                    });
                    append_preheat_terminal_event(
                        &transcript_path,
                        "compaction_failed",
                        Some(detail),
                    );
                    return Err(error);
                }
            };
            let covered_chars: usize = snapshot.iter().map(estimate_msg_chars).sum();
            let est_covered_tok = estimated_tokens_from_chars(covered_chars);
            let est_summary_tok = estimated_tokens_from_chars(summary_text.len());
            let est_saved = est_covered_tok.saturating_sub(est_summary_tok);
            let elapsed_ms = started.elapsed().as_millis() as u64;
            let result = CompactionResult {
                summary_text,
                covered_start_id,
                covered_end_id,
                covered_count,
                transcript_compaction_entry_id: Some(batch_compaction_id),
                estimated_covered_tokens_before: Some(est_covered_tok),
                estimated_summary_tokens: Some(est_summary_tok),
                estimated_tokens_saved: Some(est_saved),
                preheat_elapsed_ms: elapsed_ms,
            };
            if let Err(error) = write_preheat_cache(&transcript_path, &result) {
                warn!(transcript = %transcript_path.display(), %error,
                    "failed to cache completed preheat; applying in this process can still proceed");
            }
            let _ = eb.emit(AgentEvent::AutoCompactionEnd {
                elapsed_ms,
                summary_chars: result.summary_text.len(),
                covered_count,
                ratio_after: ratio_before,
                estimated_covered_tokens_before: est_covered_tok,
                estimated_summary_tokens: est_summary_tok,
                estimated_tokens_saved: est_saved,
            });
            Ok(result)
        });

        self.state = PreheatState::Running {
            handle,
            transcript_path: running_transcript_path,
            covered_start_id: running_start_id,
            covered_end_id: running_end_id,
            covered_count,
            started_at: Instant::now(),
        };

        true
    }

    /// Restart a failed task only if a new snapshot can actually be launched.
    #[allow(clippy::too_many_arguments)]
    pub fn try_restart_if_pending(
        &mut self,
        usage_ratio: f64,
        messages: &[ChatMessage],
        transcript_path: &std::path::Path,
        cache_key: Option<String>,
        llm: Arc<dyn LlmProvider>,
        resolved_output_limit: Option<u32>,
        input_budget_tokens: Option<usize>,
        config: &ContextConfig,
        emitter: Arc<ScopedEventEmitter>,
        control: Option<ControlSnapshot>,
    ) -> bool {
        if !self.is_exhausted_pending() {
            return false;
        }
        self.launch(
            usage_ratio,
            messages,
            transcript_path,
            cache_key,
            llm,
            resolved_output_limit,
            input_budget_tokens,
            config,
            emitter,
            control,
        )
    }

    /// Nonblocking collection at a checkpoint. Keep successful results until the application
    /// waterline is reached, and collect failures regardless of the current waterline.
    pub fn settle(&mut self) {
        let PreheatState::Running { handle, .. } = &mut self.state else {
            return;
        };
        if handle.is_finished() {
            if let Some(joined) = futures_util::FutureExt::now_or_never(handle) {
                self.finish_join(joined);
            }
        }
    }

    pub fn poll_result(&mut self) -> PreheatOutcome {
        self.settle();
        match self.state {
            PreheatState::CachedCompleted { .. } => {
                let PreheatState::CachedCompleted { result } = std::mem::take(&mut self.state)
                else {
                    unreachable!()
                };
                PreheatOutcome::Completed(result)
            }
            PreheatState::ExhaustedPending => PreheatOutcome::Exhausted,
            _ => PreheatOutcome::NotReady,
        }
    }

    /// Await the existing task without creating a new one. On timeout, reap it locally before
    /// handing control back. This does not guarantee that the remote provider stopped work.
    #[cfg(test)]
    pub async fn await_result(&mut self, timeout: Duration) -> PreheatOutcome {
        self.await_result_or_stop(timeout, &tokio_util::sync::CancellationToken::new())
            .await
    }

    pub(crate) async fn await_result_or_stop(
        &mut self,
        timeout: Duration,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> PreheatOutcome {
        if cancel.is_cancelled() {
            return PreheatOutcome::NotReady;
        }
        let PreheatState::Running { handle, .. } = &mut self.state else {
            return self.poll_result();
        };
        let joined = tokio::select! {
            biased;
            _ = cancel.cancelled() => return PreheatOutcome::NotReady,
            result = tokio::time::timeout(timeout, &mut *handle) => result,
        };
        let joined = match joined {
            Ok(joined) => joined,
            Err(_) => {
                handle.abort();
                match handle.await {
                    Err(error) if error.is_cancelled() => {
                        let PreheatState::Running {
                            transcript_path, ..
                        } = std::mem::take(&mut self.state)
                        else {
                            unreachable!()
                        };
                        warn!(?timeout, "preheat timed out and was reaped locally");
                        append_preheat_terminal_event(
                            &transcript_path,
                            "compaction_aborted",
                            Some(format!("preheat timed out after {timeout:?}")),
                        );
                        return PreheatOutcome::Failed;
                    }
                    // It finished between the deadline and the cancellation request.
                    joined => joined,
                }
            }
        };
        self.finish_join(joined);
        if cancel.is_cancelled() {
            return PreheatOutcome::NotReady;
        }
        self.poll_result()
    }

    fn finish_join(
        &mut self,
        joined: Result<Result<CompactionResult, AppError>, tokio::task::JoinError>,
    ) {
        self.state = match joined {
            Ok(Ok(result)) => PreheatState::CachedCompleted { result },
            Ok(Err(_)) => PreheatState::ExhaustedPending,
            Err(error) => {
                if let PreheatState::Running {
                    transcript_path, ..
                } = &self.state
                {
                    warn!(%error, "preheat task failed to join");
                    append_preheat_terminal_event(
                        transcript_path,
                        "compaction_failed",
                        Some(error.to_string()),
                    );
                }
                PreheatState::ExhaustedPending
            }
        };
    }

    /// any → Idle。取消运行中任务 + 清除 pending。
    pub fn abort(&mut self) {
        if let PreheatState::Running {
            handle,
            transcript_path,
            ..
        } = std::mem::replace(&mut self.state, PreheatState::Idle)
        {
            handle.abort();
            append_preheat_terminal_event(&transcript_path, "compaction_aborted", None);
        }
    }
}

// ---------------------------------------------------------------------------
// generate_summary
// ---------------------------------------------------------------------------

/// 根据 messages snapshot 生成 LLM 摘要（首次或 UPDATE 模式）。
///
/// 模型只负责中间那段叙述。控制态与用户原话由 [`machine_block`] 在模型返回**之后**
/// 拼到最前面，`## Progress` 也由控制态快照选定的待办来源重写——
/// 这三样东西不经过模型，也就不可能被模型编造。
pub async fn generate_summary(
    snapshot: &[ChatMessage],
    previous_summary: Option<&str>,
    llm: &dyn LlmProvider,
    compaction_model: &str,
    control: Option<&ControlSnapshot>,
    cache_key: Option<&str>,
) -> Result<String, AppError> {
    generate_summary_with_output_limit(
        snapshot,
        previous_summary,
        llm,
        compaction_model,
        control,
        SummaryRequestOptions {
            cache_key,
            ..Default::default()
        },
    )
    .await
}

/// Same as [`generate_summary`], with a provider-wire output limit resolved
/// from the selected compaction model's capability.
pub(crate) async fn generate_summary_with_output_limit(
    snapshot: &[ChatMessage],
    previous_summary: Option<&str>,
    llm: &dyn LlmProvider,
    compaction_model: &str,
    control: Option<&ControlSnapshot>,
    options: SummaryRequestOptions<'_>,
) -> Result<String, AppError> {
    let prompt = if let Some(existing) = previous_summary {
        // 回灌前先剥掉机器区：上一版的控制态已经过期，用户原话下面会重新拼。
        UPDATE_SUMMARIZATION_PROMPT.replace("{existing_summary}", &machine_block::strip(existing))
    } else {
        SUMMARIZATION_PROMPT.to_string()
    };

    let batch_text = summary_projection(snapshot, &prompt, options.input_budget_tokens);

    // Compaction MUST NOT carry tools — 显式 `tools: None` 与 prompt 首行
    // `Respond with text only. Do not call any tools.` 形成双保险：
    //   - 即使某些 provider 忽略 prompt 指令，请求体里没有 tool schema 也无法触发 tool_call；
    //   - 若未来通过 `..Default::default()` 引入新字段，显式赋值能保证 compaction 路径不被默认 tool 推送污染。
    // 详见 docs/reports/compaction-prompt-cc-vs-pi.md §5.6 / §5.7.1，以及计划 §6.B 子项 2。
    let req = ChatRequest {
        model: compaction_model.to_string(),
        messages: vec![ChatMessage::system(&prompt), ChatMessage::user(&batch_text)],
        resolved_output_limit: options.resolved_output_limit,
        stream: Some(false),
        tools: None,
        cache_key: options.cache_key.map(str::to_owned),
        ..Default::default()
    };

    let resp = llm.chat_collect(req).await?;
    let mut text = resp
        .choices
        .first()
        .and_then(|c| c.message.text_content())
        .unwrap_or("")
        .to_string();

    if text.trim().is_empty() {
        return Err(AppError::internal("LLM returned empty summary"));
    }

    if let Some(progress) = control.and_then(|control| control.progress.as_ref()) {
        text = machine_block::override_progress_section(&text, progress);
    }

    let sidecar_path = match options.transcript_path {
        Some(path) => ensure_user_message_sidecar_current(path).await,
        None => None,
    };
    let verbatim_user_messages = sidecar_path
        .as_deref()
        .and_then(|path| {
            recent_user_message_texts(path, machine_block::VERBATIM_MESSAGE_LIMIT)
                .map_err(|error| {
                    warn!(
                        sidecar = %path.display(),
                        %error,
                        "failed to read recent user input from sidecar; falling back to snapshot"
                    );
                    error
                })
                .ok()
        })
        .unwrap_or_else(|| machine_block::collect_verbatim_user_messages(snapshot));
    let blocks = machine_block::render_with_sidecar_for_messages(
        control,
        &verbatim_user_messages,
        sidecar_path.as_deref(),
        snapshot,
    );
    Ok(machine_block::prepend(&blocks, &text))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// A marker must be appended after a durable normal-message tail, not after an already compacted
/// summary's synthetic id. Skip `CompactionSummary` and use the first/last ordinary message ids.
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

fn find_last_summary(messages: &[ChatMessage]) -> Option<String> {
    messages.iter().rev().find_map(|m| {
        if m.kind == MessageKind::CompactionSummary {
            m.text_content().map(|s| s.to_string())
        } else {
            None
        }
    })
}

fn summary_projection(snapshot: &[ChatMessage], prompt: &str, budget: Option<usize>) -> String {
    let full = render_summary_messages(snapshot, true);
    if budget.is_some_and(|limit| estimated_tokens_from_chars(prompt.len() + full.len()) > limit) {
        // This projection is only used by the summary model, not the transcript or main request.
        render_summary_messages(snapshot, false)
    } else {
        full
    }
}

#[cfg(test)]
pub(super) fn messages_to_text(messages: &[ChatMessage]) -> String {
    render_summary_messages(messages, true)
}

fn render_summary_messages(messages: &[ChatMessage], include_tool_results: bool) -> String {
    let mut buf = String::new();
    for m in messages {
        if !include_tool_results && m.role == ChatMessageRole::Tool {
            continue;
        }
        match m.kind {
            MessageKind::CompactionSummary => {
                buf.push_str("[Previous Summary]\n");
                if let Some(text) = m.text_content() {
                    buf.push_str(&machine_block::strip(text));
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
#[path = "tests/preheat_test.rs"]
mod tests;
