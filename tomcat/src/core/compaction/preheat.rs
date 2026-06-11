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
//! 摘要生成成功后调用方在 `apply.rs` 中通过 [`crate::core::session::transcript::insert_entry_after_message_id`]
//! 把 `BranchSummaryEntry` 写回 JSONL，并发射 `AgentEvent::AutoCompactionEnd`。
//! 重试与失败路径分别发 `AutoCompactionStart` / `CompactionError`。

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
use crate::infra::event_bus::ScopedEventEmitter;
use crate::infra::events::AgentEvent;

use super::truncation::floor_char_boundary;

const MAX_PREHEAT_RETRIES: u32 = 3;

// ---------------------------------------------------------------------------
// Prompt templates (T2-P0-002 Phase B — 9 节模板，唯一来源：
//   - docs/reports/compaction-prompt-cc-vs-pi.md §5.3 (BASE)
//   - docs/reports/compaction-prompt-cc-vs-pi.md §5.4 (UPDATE)
//
// 设计要点（详见 docs/reports/compaction-prompt-cc-vs-pi.md §5.5 与 §5.7.1）：
//   1. 首行固定 `Respond with text only. Do not call any tools.` —— 防止部分 provider
//      在摘要场景误调工具，与 generate_summary 中 ChatRequest.tools = None 形成双保险。
//   2. 指令区追加 `First reason internally, then output the final summary.` —— Two-pass
//      decision freeze（关闭 #T-044）的替代策略，让模型走内部隐式推理，避免双轮草稿翻倍 token。
//   3. 9 节结构 + Recent User Messages 保留最近 10 条用户原话 + Next Steps verbatim 引用，
//      让下一轮 LLM 能从摘要直接接力，不再丢任务上下文。
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

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or "(none)" if none were mentioned]

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

## Recent User Messages
- [Verbatim or near-verbatim quote of the 10 most recent non-tool user messages, to preserve task intent]

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
- UPDATE "Next Steps" and "Recent User Messages" to reflect the latest state
- REMOVE information that is no longer relevant to free space
- The complete updated summary should be under ~8K tokens
- When the old summary is already large, compress older details to stay within budget
- PRESERVE exact file paths, function names, and error messages

Use the EXACT same format as the original summary (Goal / Constraints & Preferences / Progress / Errors Encountered / Key Decisions / Recent User Messages / Next Steps / Critical Context)."#;

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
        emitter: Arc<ScopedEventEmitter>,
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

        let eb = emitter.clone();
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
                                error: None,
                                attempts: None,
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
                            let _ = eb.emit(AgentEvent::AutoCompactionEnd {
                                elapsed_ms,
                                summary_chars: result.summary_text.len(),
                                covered_count,
                                ratio_after: ratio_before,
                                estimated_covered_tokens_before: est_covered_tok,
                                estimated_summary_tokens: est_summary_tok,
                                estimated_tokens_saved: est_saved,
                            });
                        }

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

            // T2-P0-002 Phase D：3 次重试全部失败 → 在 transcript 落一条
            // BranchSummaryEntry { summary: None, error, attempts } 失败锚点。
            // 同时承接 #T-040：LLM 因 batch 过长返回 `context_length_exceeded` 也走同一路径，
            // 不再为「超大消息」单独引入硬截断（详见计划 §6.C 决议段 + 报告 §5.7）。
            // reload 时 `session::manager::context::fold_entries_to_messages` 凭 `summary == None`
            // 跳过该行，不会重建假摘要 ChatMessage（详见 transcript.rs 字段注释）。
            let failure_entry = TranscriptEntry::BranchSummary(BranchSummaryEntry {
                id: Some(batch_compaction_id.clone()),
                parent_id: None,
                timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                summary: None,
                covered_start_id: Some(covered_start_id.clone()),
                covered_end_id: Some(covered_end_id.clone()),
                covered_count: Some(covered_count),
                is_boundary: Some(false),
                preheat_compaction_id: Some(batch_compaction_id.clone()),
                estimated_covered_tokens_before: None,
                estimated_summary_tokens: None,
                estimated_tokens_saved: None,
                error: Some(last_error.clone()),
                attempts: Some(MAX_PREHEAT_RETRIES),
            });
            if !transcript_path.as_os_str().is_empty() {
                if let Err(e) =
                    insert_entry_after_message_id(&transcript_path, &covered_end_id, &failure_entry)
                {
                    warn!(
                        "preheat failure-trail insert_entry_after_message_id failed: {}",
                        e
                    );
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
        emitter: Arc<ScopedEventEmitter>,
    ) -> bool {
        if !self.is_exhausted_pending() {
            return false;
        }
        self.state = PreheatState::Idle;
        self.try_start(usage_ratio, messages, transcript_path, llm, config, emitter)
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

    // Compaction MUST NOT carry tools — 显式 `tools: None` 与 prompt 首行
    // `Respond with text only. Do not call any tools.` 形成双保险：
    //   - 即使某些 provider 忽略 prompt 指令，请求体里没有 tool schema 也无法触发 tool_call；
    //   - 若未来通过 `..Default::default()` 引入新字段，显式赋值能保证 compaction 路径不被默认 tool 推送污染。
    // 详见 docs/reports/compaction-prompt-cc-vs-pi.md §5.6 / §5.7.1，以及计划 §6.B 子项 2。
    let req = ChatRequest {
        model: compaction_model.to_string(),
        messages: vec![ChatMessage::system(&prompt), ChatMessage::user(&batch_text)],
        stream: Some(false),
        tools: None,
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
#[path = "tests/preheat_test.rs"]
mod tests;
