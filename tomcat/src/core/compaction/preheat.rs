//! # Layer-1 异步预压缩（Preheat）
//!
//! 后台 tokio task 在用户输入空闲期就把"段落级摘要"算好（LLM call），等到
//! `usage_ratio` 触发边界时直接把已就绪的摘要灌进 transcript，避免压缩本身
//! 阻塞用户输入；与 `apply.rs` 的"最后一刻压缩"形成两段式策略。
//!
//! ## 状态机（4 态）
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────────────┐
//! │                                                                           │
//! │           ┌───────┐  try_start (前置已通过)                              │
//! │   ┌──────►│ Idle  ├─────────────────────────────┐                        │
//! │   │       └───┬───┘                              │                        │
//! │   │           │ restore_completed (reload 时)    │ spawn(generate_summary)│
//! │   │           ▼                                  ▼                        │
//! │   │   ┌──────────────────┐                ┌─────────────┐                │
//! │   │   │ CachedCompleted  │ ◄──────────────┤   Running   │ ◄──────┐       │
//! │   │   │ {result}         │  Ok 完成        │ {handle,    │        │       │
//! │   │   └─────────┬────────┘                │  attempt,   │        │       │
//! │   │             │                          │  started_at}│        │       │
//! │   │             │ poll_result/             └──┬──────┬───┘        │       │
//! │   │             │ await_result                │      │            │       │
//! │   │             ▼                              │      │ Err 第 N 次│       │
//! │   │   PreheatOutcome::Completed                │      │  (N<3 重试)│       │
//! │   │             │                              │      ▼            │       │
//! │   │             │              Err 第 3 次  ◄──┘  retry: 重 spawn ─┘       │
//! │   │             │                              │                          │
//! │   └─ apply ─────┘                              ▼                          │
//! │                                       ┌────────────────────┐              │
//! │                                       │ ExhaustedPending   │              │
//! │                                       └─────────┬──────────┘              │
//! │                                                 │                          │
//! │              try_restart_if_pending(usage_ratio)│                          │
//! │              ── ratio 仍超阈值 ► 重 spawn ──────┘                          │
//! │              ── ratio 已下降   ► 留 ExhaustedPending（等下一轮）           │
//! │                                                                           │
//! │   abort()：任意态 ► JoinHandle.abort + state = Idle                       │
//! │   discard_cached_completed()：CachedCompleted ► Idle（手动丢弃）           │
//! │                                                                           │
//! └──────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## 5 个公共入口
//!
//! | 方法                          | 何时调用                                |
//! | ----------------------------- | --------------------------------------- |
//! | `try_start`                   | check_before_request 触发预热阈值       |
//! | `try_restart_if_pending`      | 下一回合开始前，从 ExhaustedPending 重启 |
//! | `poll_result`                 | apply_boundary 前非阻塞探一下结果       |
//! | `await_result(timeout)`       | 边界压缩兜底等待                         |
//! | `abort`                       | 退出 / 中断 / 显式取消                   |
//!
//! ## 为什么不直接 enum public
//!
//! `PreheatState` 持有 `JoinHandle` 与 `started_at` 等内部记账字段，外暴露会让
//! 调用方误改状态；改用 `is_idle()` / `is_running()` / `preheat_result_pending()`
//! 等查询方法保证状态机变迁路径可枚举。
//!
//! ## 副作用
//!
//! 预热启动时前台在 transcript 尾部追加一个 `BranchSummaryEntry` 切口标记；摘要完成后
//! 调用方在 `apply.rs` 追加其 `BranchSummaryTextEntry` 正文，并发射
//! `AgentEvent::AutoCompactionEnd`。
//! 重试与失败路径分别发 `AutoCompactionStart` / `CompactionError`。

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
    append_entry, entry_id, read_entries_tail, BranchSummaryEntry, TranscriptEntry,
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

const MAX_PREHEAT_RETRIES: u32 = 3;
/// 生成摘要时随调用携带的可选运行时资料，避免入口继续膨胀位置参数。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SummaryRequestOptions<'a> {
    pub cache_key: Option<&'a str>,
    pub resolved_output_limit: Option<u32>,
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
    /// The foreground appends its cut marker before spawn. The spawned task only computes the
    /// summary (and optional crash cache), so it can never race foreground transcript appends.
    /// It retries generation at most three times and emits AutoCompactionEnd on success.
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
        config: &ContextConfig,
        emitter: Arc<ScopedEventEmitter>,
        control: Option<ControlSnapshot>,
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

        // This is the only moment where appending a boundary marker also puts it exactly at the
        // semantic cut: the snapshot's covered end is still the transcript tail. Never defer
        // this write into the background task, where intervening tool rows would make it a costly
        // mid-file insertion.
        if !transcript_path.as_os_str().is_empty() {
            let covered_end_is_tail = match read_entries_tail(transcript_path, 1) {
                Ok(entries) => entries
                    .last()
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
            if !covered_end_is_tail {
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

        let transcript_path = transcript_path.to_path_buf();
        let compaction_model = config.compaction_model.clone();
        let running_start_id = covered_start_id.clone();
        let running_end_id = covered_end_id.clone();
        let ratio_before = usage_ratio;
        let cache_key = cache_key.unwrap_or_default();

        let existing_summary = find_last_summary(&snapshot);

        let eb = emitter.clone();
        let handle = tokio::spawn(async move {
            let started = Instant::now();
            let mut last_error = String::new();

            for attempt in 1..=MAX_PREHEAT_RETRIES {
                match generate_summary_with_output_limit(
                    &snapshot,
                    existing_summary.as_deref(),
                    &*llm,
                    &compaction_model,
                    control.as_ref(),
                    SummaryRequestOptions {
                        cache_key: (!cache_key.is_empty()).then_some(cache_key.as_str()),
                        resolved_output_limit,
                        transcript_path: (!transcript_path.as_os_str().is_empty())
                            .then_some(transcript_path.as_path()),
                    },
                )
                .await
                {
                    Ok(summary_text) => {
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
                            transcript_compaction_entry_id: Some(batch_compaction_id.clone()),
                            estimated_covered_tokens_before: Some(est_covered_tok),
                            estimated_summary_tokens: Some(est_summary_tok),
                            estimated_tokens_saved: Some(est_saved),
                            preheat_elapsed_ms: elapsed_ms,
                        };

                        if let Err(error) = write_preheat_cache(&transcript_path, &result) {
                            warn!(
                                transcript = %transcript_path.display(),
                                %error,
                                "failed to cache completed preheat; applying in this process can still proceed"
                            );
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

                        return Ok(result);
                    }
                    Err(e) => {
                        last_error = e.to_string();
                        warn!(
                            "preheat attempt {}/{} failed: {}",
                            attempt, MAX_PREHEAT_RETRIES, last_error
                        );
                        // T2-P0-002 Phase D：失败重试加指数退避（500ms / 1s / 2s）。
                        // attempt < MAX_PREHEAT_RETRIES 时才睡，避免最后一次 Err 后做无意义的等待。
                        // `500 << (attempt - 1)` ⇒ attempt=1→500ms, attempt=2→1000ms（attempt=3 不睡）。
                        // 该 await 位于 tokio::spawn 的异步上下文，不会阻塞调用线程；测试用
                        // `tokio::time::pause` + `advance` 控制虚拟时钟（详见 tests/preheat_backoff.rs）。
                        if attempt < MAX_PREHEAT_RETRIES {
                            let backoff = std::time::Duration::from_millis(500u64 << (attempt - 1));
                            tokio::time::sleep(backoff).await;
                        }
                    }
                }
            }

            let _ = eb.emit(AgentEvent::CompactionError {
                exhausted_after_retries: true,
                attempts: MAX_PREHEAT_RETRIES,
                error: last_error.clone(),
                source: "preheat".to_string(),
                ratio: Some(ratio_before),
            });

            Err(AppError::Llm(format!(
                "preheat exhausted after {} retries: {}",
                MAX_PREHEAT_RETRIES, last_error
            )))
        });

        self.state = PreheatState::Running {
            handle,
            covered_start_id: running_start_id,
            covered_end_id: running_end_id,
            covered_count,
            started_at: Instant::now(),
        };

        true
    }

    /// ExhaustedPending → Running（条件：ratio >= 0.50）。
    /// 内部先转 Idle 再调 try_start。
    #[allow(clippy::too_many_arguments)]
    pub fn try_restart_if_pending(
        &mut self,
        usage_ratio: f64,
        messages: &[ChatMessage],
        transcript_path: &std::path::Path,
        cache_key: Option<String>,
        llm: Arc<dyn LlmProvider>,
        resolved_output_limit: Option<u32>,
        config: &ContextConfig,
        emitter: Arc<ScopedEventEmitter>,
        control: Option<ControlSnapshot>,
    ) -> bool {
        if !self.is_exhausted_pending() {
            return false;
        }
        self.state = PreheatState::Idle;
        self.try_start(
            usage_ratio,
            messages,
            transcript_path,
            cache_key,
            llm,
            resolved_output_limit,
            config,
            emitter,
            control,
        )
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
    let batch_text = messages_to_text(snapshot);

    let prompt = if let Some(existing) = previous_summary {
        // 回灌前先剥掉机器区：上一版的控制态已经过期，用户原话下面会重新拼。
        UPDATE_SUMMARIZATION_PROMPT.replace("{existing_summary}", &machine_block::strip(existing))
    } else {
        SUMMARIZATION_PROMPT.to_string()
    };

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

    if text.is_empty() {
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

pub(super) fn messages_to_text(messages: &[ChatMessage]) -> String {
    let mut buf = String::new();
    for m in messages {
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
