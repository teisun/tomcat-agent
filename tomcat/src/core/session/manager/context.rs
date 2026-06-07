//! init_context_state helpers and context assembly functions.

use std::path::PathBuf;
use std::time::Instant;

use chrono::{NaiveDate, Utc};

use crate::core::llm::{ChatMessage, ChatMessageRole, MessageKind};
use crate::core::session::{
    append_message_chain::collect_recent_chat_messages_from_tail,
    find_dangling_tail_tool_call_ids,
};
use crate::core::session::resume_index::{
    load_or_rebuild_resume_index, rebuild_resume_index, ResumeAnchor, ResumeIndex,
    ResumeIndexIoStats, ResumeIndexSource,
};
use crate::core::session::transcript::{
    read_entries_tail_with_stats, BranchSummaryEntry, TranscriptEntry, TranscriptReadStats,
};
use crate::infra::config::{compute_context_budget_chars, ContextConfig, ResumeHydrationMode};
use crate::infra::error::AppError;

use super::session_impl::generate_entry_id;
use super::session_impl::SessionManager;
use crate::core::compaction::preheat::Preheat;

use super::types::{
    estimate_msg_chars, CompactionResult, ContextState, PlanEventRef, SessionContextObservation,
};

const DEFAULT_CONTEXT_CAP: usize = 10;
const MAX_PLAN_SCAN: usize = 5000;
pub(crate) const INTERRUPTED_TOOL_RESULT_TEXT: &str = "[interrupted]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HydrateTraceMode {
    Full,
    Tail,
}

impl HydrateTraceMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Full => "Full",
            Self::Tail => "Tail",
        }
    }
}

#[derive(Debug, Clone)]
struct HydrateLoadOutcome {
    entries: Vec<TranscriptEntry>,
    latest_plan_event: Option<PlanEventRef>,
    io_stats: ResumeIndexIoStats,
    trace_mode: HydrateTraceMode,
    plan_source: &'static str,
    fallback: &'static str,
}

#[derive(Debug, Clone)]
struct ResumeTrace {
    mode: HydrateTraceMode,
    entries_scanned: usize,
    bytes_scanned: u64,
    boundary_hit: bool,
    plan_source: &'static str,
    fallback: &'static str,
    elapsed_ms: u128,
}

impl ResumeTrace {
    fn emit_if_enabled(&self) {
        let enabled = std::env::var("TOMCAT_RESUME_TRACE")
            .map(|raw| raw != "0" && !raw.is_empty())
            .unwrap_or(false);
        if !enabled {
            return;
        }
        eprintln!(
            "TOMCAT_RESUME_TRACE mode={} entries_scanned={} bytes_scanned={} boundary_hit={} plan_source={} fallback={} elapsed_ms={}",
            self.mode.as_str(),
            self.entries_scanned,
            self.bytes_scanned,
            self.boundary_hit,
            self.plan_source,
            self.fallback,
            self.elapsed_ms
        );
    }
}

fn add_transcript_stats(io_stats: &mut ResumeIndexIoStats, read_stats: TranscriptReadStats) {
    io_stats.add_read_stats(read_stats);
}

fn legacy_read_cap() -> usize {
    super::BRANCH_MAX_ENTRIES.max(MAX_PLAN_SCAN)
}

fn boundary_exists(entries: &[TranscriptEntry]) -> bool {
    entries.iter().any(
        |entry| matches!(entry, TranscriptEntry::BranchSummary(ce) if ce.is_boundary == Some(true)),
    )
}

fn choose_recent_turn_anchor(index: &ResumeIndex) -> Option<ResumeAnchor> {
    if index.recent_turn_starts.is_empty() {
        return None;
    }
    if index.recent_turn_starts.len() >= DEFAULT_CONTEXT_CAP {
        return index
            .recent_turn_starts
            .get(index.recent_turn_starts.len() - DEFAULT_CONTEXT_CAP)
            .cloned();
    }
    index.recent_turn_starts.first().cloned()
}

fn choose_today_anchor(index: &ResumeIndex, today: NaiveDate) -> Option<ResumeAnchor> {
    index
        .latest_day_first_entry
        .as_ref()
        .and_then(|anchor| (anchor.date == today.to_string()).then(|| anchor.first_entry.clone()))
}

fn earlier_anchor(lhs: Option<ResumeAnchor>, rhs: Option<ResumeAnchor>) -> Option<ResumeAnchor> {
    match (lhs, rhs) {
        (Some(left), Some(right)) => {
            if left.ordinal <= right.ordinal {
                Some(left)
            } else {
                Some(right)
            }
        }
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

fn later_anchor(lhs: Option<ResumeAnchor>, rhs: Option<ResumeAnchor>) -> Option<ResumeAnchor> {
    match (lhs, rhs) {
        (Some(left), Some(right)) => {
            if left.ordinal >= right.ordinal {
                Some(left)
            } else {
                Some(right)
            }
        }
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

pub(super) fn compute_turn_window_start(
    index: &ResumeIndex,
    today: NaiveDate,
) -> Option<ResumeAnchor> {
    let today_anchor = choose_today_anchor(index, today);
    let recent_turn_anchor = choose_recent_turn_anchor(index);
    earlier_anchor(today_anchor, recent_turn_anchor)
}

pub(super) fn compute_slice_start_anchor(
    index: &ResumeIndex,
    today: NaiveDate,
) -> Option<ResumeAnchor> {
    later_anchor(
        index.latest_boundary.clone(),
        compute_turn_window_start(index, today),
    )
}

pub(super) fn compute_tail_count(total_entries: usize, slice_start_ordinal: usize) -> usize {
    total_entries.saturating_sub(slice_start_ordinal)
}

fn full_hydration_entries(
    session: &SessionManager,
    path: &std::path::Path,
) -> Result<HydrateLoadOutcome, AppError> {
    let read_cap = legacy_read_cap();
    let (mut entries, read_stats) = read_entries_tail_with_stats(path, read_cap)?;
    let mut io_stats = ResumeIndexIoStats::default();
    add_transcript_stats(&mut io_stats, read_stats);
    if heal_dangling_tail_tool_call(session, &entries)? {
        let (reloaded, reloaded_stats) = read_entries_tail_with_stats(path, read_cap)?;
        entries = reloaded;
        add_transcript_stats(&mut io_stats, reloaded_stats);
    }
    let latest_plan_event = extract_latest_plan_event(&entries);
    if entries.len() >= MAX_PLAN_SCAN {
        tracing::warn!(
            scanned = entries.len(),
            max_plan_scan = MAX_PLAN_SCAN,
            "init_context_state scanned at least MAX_PLAN_SCAN transcript entries"
        );
    }
    Ok(HydrateLoadOutcome {
        entries,
        latest_plan_event: latest_plan_event.clone(),
        io_stats,
        trace_mode: HydrateTraceMode::Full,
        plan_source: if latest_plan_event.is_some() {
            "scan"
        } else {
            "none"
        },
        fallback: "none",
    })
}

fn targeted_hydration_entries_with_load(
    session: &SessionManager,
    path: &std::path::Path,
    today: NaiveDate,
    load: crate::core::session::resume_index::ResumeIndexLoad,
) -> Result<HydrateLoadOutcome, AppError> {
    let index = load.index.clone();
    let latest_plan_event = index.latest_plan_event_ref();
    let slice_start_anchor = compute_slice_start_anchor(&index, today);
    let slice_start_ordinal = slice_start_anchor
        .as_ref()
        .map(|anchor| anchor.ordinal)
        .unwrap_or(0);
    if slice_start_ordinal > index.total_entries {
        let (_, rebuild_stats) = rebuild_resume_index(path)?;
        let mut io_stats = load.stats;
        io_stats.bytes_scanned += rebuild_stats.bytes_scanned;
        io_stats.entries_scanned += rebuild_stats.entries_scanned;
        let mut fallback = full_hydration_entries(session, path)?;
        fallback.io_stats.bytes_scanned += io_stats.bytes_scanned;
        fallback.io_stats.entries_scanned += io_stats.entries_scanned;
        fallback.fallback = "full+rebuild";
        return Ok(fallback);
    }
    let k = compute_tail_count(index.total_entries, slice_start_ordinal);
    let (mut entries, tail_stats) = read_entries_tail_with_stats(path, k.max(1))?;
    let mut io_stats = load.stats;
    add_transcript_stats(&mut io_stats, tail_stats);

    if let Some(anchor) = slice_start_anchor.as_ref() {
        let edge_matches = entries
            .first()
            .is_some_and(|entry| anchor.matches_entry(entry));
        if !edge_matches {
            let (_, rebuild_stats) = rebuild_resume_index(path)?;
            io_stats.bytes_scanned += rebuild_stats.bytes_scanned;
            io_stats.entries_scanned += rebuild_stats.entries_scanned;

            let mut fallback = full_hydration_entries(session, path)?;
            fallback.io_stats.bytes_scanned += io_stats.bytes_scanned;
            fallback.io_stats.entries_scanned += io_stats.entries_scanned;
            fallback.fallback = "full+rebuild";
            return Ok(fallback);
        }
    }

    if heal_dangling_tail_tool_call(session, &entries)? {
        let reloaded = targeted_hydration_entries(session, path, today)?;
        let mut reloaded = reloaded;
        reloaded.io_stats.bytes_scanned += io_stats.bytes_scanned;
        reloaded.io_stats.entries_scanned += io_stats.entries_scanned;
        return Ok(reloaded);
    }

    Ok(HydrateLoadOutcome {
        entries: std::mem::take(&mut entries),
        latest_plan_event: latest_plan_event.clone(),
        io_stats,
        trace_mode: HydrateTraceMode::Tail,
        plan_source: if latest_plan_event.is_some() {
            "sidecar"
        } else {
            "none"
        },
        fallback: if load.source == ResumeIndexSource::Rebuilt {
            "rebuild"
        } else {
            "none"
        },
    })
}

fn targeted_hydration_entries(
    session: &SessionManager,
    path: &std::path::Path,
    today: NaiveDate,
) -> Result<HydrateLoadOutcome, AppError> {
    let load = load_or_rebuild_resume_index(path)?;
    targeted_hydration_entries_with_load(session, path, today, load)
}

fn entry_timestamp(entry: &TranscriptEntry) -> &str {
    match entry {
        TranscriptEntry::Message(e) => &e.timestamp,
        TranscriptEntry::ModelChange(e) => &e.timestamp,
        TranscriptEntry::ThinkingLevelChange(e) => &e.timestamp,
        TranscriptEntry::ThinkingTrace(e) => &e.timestamp,
        TranscriptEntry::BranchSummary(e) => &e.timestamp,
        TranscriptEntry::Label(e) => &e.timestamp,
        TranscriptEntry::SessionInfo(e) => &e.timestamp,
        TranscriptEntry::Custom(e) => &e.timestamp,
    }
}

pub(super) fn is_user_message(entry: &TranscriptEntry) -> bool {
    if let TranscriptEntry::Message(me) = entry {
        me.message.get("role").and_then(|r| r.as_str()) == Some("user")
    } else {
        false
    }
}

pub(super) fn parse_date(ts: &str) -> Option<NaiveDate> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.date_naive())
}

/// Phase 1: 反向预扫描 entries，返回应该开始折叠的起始 index。
/// 保证 entries[fold_start..] 包含足够 entries 来产生：
///   - 所有 today 的 turns
///   - 不足 min_turns 时的 backfill turns
///   - boundary 之后的全部内容
pub(super) fn compute_fold_start(
    entries: &[TranscriptEntry],
    today: NaiveDate,
    min_turns: usize,
) -> usize {
    let boundary = entries.iter().rposition(
        |e| matches!(e, TranscriptEntry::BranchSummary(ce) if ce.is_boundary == Some(true)),
    );
    let effective_start = boundary.unwrap_or(0);

    let today_start = entries[effective_start..]
        .iter()
        .position(|e| parse_date(entry_timestamp(e)) == Some(today))
        .map(|i| effective_start + i);

    let today_user_msgs = today_start.map_or(0, |start| {
        entries[start..]
            .iter()
            .filter(|e| is_user_message(e))
            .count()
    });

    if today_user_msgs >= min_turns {
        if let Some(b) = boundary {
            if today_start.is_some_and(|ts| ts > b) {
                return b;
            }
        }
        return today_start.unwrap_or(effective_start);
    }

    let need_extra = min_turns - today_user_msgs;
    let scan_end = today_start.unwrap_or(entries.len());
    let mut extra_found = 0;

    for i in (effective_start..scan_end).rev() {
        if is_user_message(&entries[i]) {
            extra_found += 1;
        }
        if extra_found >= need_extra {
            return i;
        }
    }

    effective_start
}

fn branch_summary_pending_from_entry(ce: &BranchSummaryEntry) -> Option<CompactionResult> {
    if ce.is_boundary != Some(false) {
        return None;
    }
    Some(CompactionResult {
        summary_text: ce.summary.clone()?,
        covered_start_id: ce.covered_start_id.clone()?,
        covered_end_id: ce.covered_end_id.clone()?,
        covered_count: ce.covered_count?,
        transcript_compaction_entry_id: ce.id.clone(),
        estimated_covered_tokens_before: ce.estimated_covered_tokens_before,
        estimated_summary_tokens: ce.estimated_summary_tokens,
        estimated_tokens_saved: ce.estimated_tokens_saved,
        preheat_elapsed_ms: 0,
    })
}

/// Phase 2: 将 entries 折叠为 ChatMessage 列表（msg_id / timestamp 已填充）。
/// boundary compaction 仍会清除之前的 messages。
/// `pending_preheat`：切片内最后一条未应用 preheat（`is_boundary=false` 且字段齐全）。
pub(super) struct FoldEntriesOutcome {
    pub messages: Vec<ChatMessage>,
    /// 折叠后全量 messages 的字符估计（未经过 `filter_messages_by_day`）；供调试或后续与 selected 对齐用。
    #[allow(dead_code)]
    pub total_chars: usize,
    pub pending_preheat: Option<CompactionResult>,
}

/// PR-RA / T2-P0-016 / T2-P0-017 PR-命名：检测 transcript 中遗留的旧工具名
/// （`read_file` / `write_file` / `edit_file` / `execute_bash`）。
///
/// 仅 **`tracing::warn!` 一次**（每个旧名按 process 去重）；**不**重写为短名，
/// **不**重定向执行。旧对话历史保留原 wire；新一轮 LLM 调用旧名会走
/// `tool_exec` 的 unknown 分支（与短名生态保持单轨审计，避免双名漂移）。
fn warn_if_legacy_tool_name(tool_calls: &[serde_json::Value]) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED_READ: AtomicBool = AtomicBool::new(false);
    static WARNED_WRITE: AtomicBool = AtomicBool::new(false);
    static WARNED_EDIT: AtomicBool = AtomicBool::new(false);
    static WARNED_BASH: AtomicBool = AtomicBool::new(false);
    for tc in tool_calls {
        let name = tc
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("");
        match name {
            "read_file"
                if WARNED_READ
                    .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok() =>
            {
                tracing::warn!(
                    tool = "read_file",
                    "legacy tool name: read_file → read (no redirect; transcript replay only)"
                );
            }
            "write_file"
                if WARNED_WRITE
                    .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok() =>
            {
                tracing::warn!(
                    tool = "write_file",
                    "legacy tool name: write_file → write (no redirect; transcript replay only)"
                );
            }
            "edit_file"
                if WARNED_EDIT
                    .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok() =>
            {
                tracing::warn!(
                    tool = "edit_file",
                    "legacy tool name: edit_file → edit (no redirect; transcript replay only)"
                );
            }
            "execute_bash"
                if WARNED_BASH
                    .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok() =>
            {
                tracing::warn!(
                    tool = "execute_bash",
                    "legacy tool name: execute_bash → bash (no redirect; transcript replay only)"
                );
            }
            _ => {}
        }
    }
}

fn chat_message_from_entry(
    me: &crate::core::session::transcript::MessageEntry,
) -> Option<ChatMessage> {
    if me
        .message
        .get("superseded")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return None;
    }
    let mut msg: ChatMessage = serde_json::from_value(me.message.clone()).ok()?;

    if let Some(ref arr) = msg.tool_calls {
        warn_if_legacy_tool_name(arr);
    }

    // Detect compaction summary injected as user message (via older paths)
    // These are plain user messages from transcript — no special marking needed here.
    // The CompactionSummary kind is only set for BranchSummary entries below.
    msg.msg_id = me.id.clone().or_else(|| Some(generate_entry_id()));
    msg.kind = MessageKind::Normal;
    msg.timestamp = Some(me.timestamp.clone());
    Some(msg)
}

fn extract_latest_plan_event(entries: &[TranscriptEntry]) -> Option<PlanEventRef> {
    entries.iter().rev().find_map(|entry| match entry {
        TranscriptEntry::Custom(custom) => PlanEventRef::from_custom_event(&custom.extra),
        _ => None,
    })
}

fn fold_entries_to_messages(
    entries: &[TranscriptEntry],
    system_text_len: usize,
) -> FoldEntriesOutcome {
    let mut messages: Vec<ChatMessage> = Vec::new();
    let mut total_chars = system_text_len;
    let mut pending_preheat: Option<CompactionResult> = None;

    for entry in entries {
        match entry {
            TranscriptEntry::BranchSummary(ce) => {
                // is_boundary=false → preheat record: skip during reload
                if ce.is_boundary == Some(false) {
                    if let Some(r) = branch_summary_pending_from_entry(ce) {
                        pending_preheat = Some(r);
                    }
                    continue;
                }

                if ce.is_boundary == Some(true) {
                    pending_preheat = None;
                }

                // is_boundary=true → discard prefix (boundary switch)
                // is_boundary=None → legacy entry, don't clear (backward compat)
                if ce.is_boundary == Some(true) {
                    messages.clear();
                    total_chars = system_text_len;
                }

                if let Some(ref summary) = ce.summary {
                    let mut summary_msg = ChatMessage::compaction_summary(summary.as_str());
                    summary_msg.msg_id = ce.id.clone().or_else(|| Some(generate_entry_id()));
                    summary_msg.timestamp = Some(ce.timestamp.clone());
                    total_chars += estimate_msg_chars(&summary_msg);
                    messages.push(summary_msg);
                }
            }
            TranscriptEntry::Message(me) => {
                if let Some(msg) = chat_message_from_entry(me) {
                    total_chars += estimate_msg_chars(&msg);
                    messages.push(msg);
                }
            }
            _ => {}
        }
    }

    FoldEntriesOutcome {
        messages,
        total_chars,
        pending_preheat,
    }
}

fn heal_dangling_tail_tool_call(
    session: &SessionManager,
    entries: &[TranscriptEntry],
) -> Result<bool, AppError> {
    let recent = collect_recent_chat_messages_from_tail(entries);
    let Some(tool_call_ids) = find_dangling_tail_tool_call_ids(&recent) else {
        return Ok(false);
    };

    tracing::warn!(
        tool_call_ids = ?tool_call_ids,
        "hydrate detected dangling tail tool_call block; appending synthetic interrupted tool results"
    );

    for tool_call_id in tool_call_ids {
        session.append_message(serde_json::json!({
            "role": "tool",
            "tool_call_id": tool_call_id,
            "content": INTERRUPTED_TOOL_RESULT_TEXT,
        }))?;
    }

    Ok(true)
}

/// Returns true if the message is a "turn start" — i.e., starts a new logical turn.
fn is_turn_start(m: &ChatMessage) -> bool {
    (m.role == ChatMessageRole::User && m.kind != MessageKind::Steering)
        || m.kind == MessageKind::CompactionSummary
}

/// Phase 3: 按天筛选 messages + 不足 min_turns 向前补齐。
pub(super) fn filter_messages_by_day(
    all_messages: Vec<ChatMessage>,
    today: NaiveDate,
    min_turns: usize,
) -> Vec<ChatMessage> {
    let today_start = all_messages
        .iter()
        .position(|m| parse_date(m.timestamp.as_deref().unwrap_or("")) == Some(today))
        .unwrap_or(all_messages.len());

    let today_turns = all_messages[today_start..]
        .iter()
        .filter(|m| is_turn_start(m))
        .count();

    if today_turns >= min_turns || today_start == 0 {
        return all_messages[today_start..].to_vec();
    }

    // Need to backfill (min_turns - today_turns) turns from before today.
    let need_extra = min_turns - today_turns;
    let pre_today = &all_messages[..today_start];

    // Collect indices of turn starts in pre-today portion.
    let turn_start_indices: Vec<usize> = pre_today
        .iter()
        .enumerate()
        .filter(|(_, m)| is_turn_start(m))
        .map(|(i, _)| i)
        .collect();

    // Take the last `need_extra` turn starts; the earliest one is our new start.
    let backfill_start = if turn_start_indices.len() <= need_extra {
        0
    } else {
        turn_start_indices[turn_start_indices.len() - need_extra]
    };

    all_messages[backfill_start..].to_vec()
}

// Keep old name as alias for tests that may use it.
#[allow(dead_code)]
pub(super) fn filter_turns_by_day(
    all_messages: Vec<ChatMessage>,
    today: NaiveDate,
    min_turns: usize,
) -> Vec<ChatMessage> {
    filter_messages_by_day(all_messages, today, min_turns)
}

fn observability_from_session(session: &SessionManager) -> Result<(u32, usize, usize), AppError> {
    Ok(session
        .get_session(session.current_session_key())?
        .map(|e| {
            (
                e.compaction_count.unwrap_or(0),
                e.compaction_tokens_freed.unwrap_or(0) as usize,
                e.tool_result_chars_persisted.unwrap_or(0) as usize,
            )
        })
        .unwrap_or((0, 0, 0)))
}

fn empty_context_state(
    system_text: &str,
    budget: usize,
    token_budget: usize,
    session_obs: SessionContextObservation,
) -> ContextState {
    ContextState {
        messages: Vec::new(),
        estimate_context_chars: system_text.len(),
        context_budget_chars: budget,
        context_budget_tokens: token_budget,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: Preheat::new(),
        session_obs,
        live: super::types::ContextLiveMetrics::default(),
    }
}

/// 从 transcript 加载历史，以 ChatMessage 列表初始化 ContextState。
/// 识别已有 Compaction entry 折叠为 CompactionSummary 消息，避免重复压缩。
/// 按天筛选：优先取当天所有 turns，不足 DEFAULT_CONTEXT_CAP 则向前补齐。
pub fn init_context_state(
    session: &SessionManager,
    config: &ContextConfig,
    system_text: &str,
) -> Result<ContextState, AppError> {
    let started = Instant::now();
    let budget = compute_context_budget_chars(config);
    let token_budget = config
        .context_window
        .saturating_sub(config.max_output_tokens);
    let (cc, ctf, trcp) = observability_from_session(session)?;
    let session_obs = SessionContextObservation {
        compaction_count: cc,
        compaction_tokens_freed: ctf,
        tool_result_chars_persisted: trcp,
    };

    let path = match session.current_transcript_path()? {
        Some(p) => p,
        None => {
            return Ok(empty_context_state(
                system_text,
                budget,
                token_budget,
                session_obs,
            ))
        }
    };
    let today = Utc::now().date_naive();
    let metadata = std::fs::metadata(&path).map_err(AppError::Io)?;

    let load_outcome = match config.resume_hydration_mode {
        ResumeHydrationMode::Full => {
            let mut outcome = full_hydration_entries(session, &path)?;
            outcome.fallback = "config_full";
            outcome
        }
        ResumeHydrationMode::Tail => targeted_hydration_entries(session, &path, today)?,
        ResumeHydrationMode::Auto => {
            let threshold = config.resume_lazy_threshold.max(1);
            match load_or_rebuild_resume_index(&path) {
                Ok(load) if load.index.total_entries >= threshold || metadata.len() == 0 => {
                    targeted_hydration_entries_with_load(session, &path, today, load)?
                }
                Ok(load) => {
                    let mut outcome = full_hydration_entries(session, &path)?;
                    if load.source == ResumeIndexSource::Rebuilt {
                        outcome.fallback = "rebuild";
                    } else {
                        outcome.fallback = "threshold_full";
                    }
                    outcome
                }
                Err(_) => full_hydration_entries(session, &path)?,
            }
        }
    };

    let entries = load_outcome.entries;
    let latest_plan_event = load_outcome.latest_plan_event;
    let fold_start = compute_fold_start(&entries, today, DEFAULT_CONTEXT_CAP);
    let fold_out = fold_entries_to_messages(&entries[fold_start..], system_text.len());
    let selected = filter_messages_by_day(fold_out.messages, today, DEFAULT_CONTEXT_CAP);

    let total_chars = system_text.len() + selected.iter().map(estimate_msg_chars).sum::<usize>();

    let mut preheat = Preheat::new();
    if let Some(p) = fold_out.pending_preheat {
        let end = p.covered_end_id.as_str();
        if selected.iter().any(|m| m.msg_id.as_deref() == Some(end)) {
            preheat.restore_completed(p);
        }
    }

    ResumeTrace {
        mode: load_outcome.trace_mode,
        entries_scanned: load_outcome.io_stats.entries_scanned,
        bytes_scanned: load_outcome.io_stats.bytes_scanned,
        boundary_hit: boundary_exists(&entries),
        plan_source: load_outcome.plan_source,
        fallback: load_outcome.fallback,
        elapsed_ms: started.elapsed().as_millis(),
    }
    .emit_if_enabled();

    Ok(ContextState {
        messages: selected,
        estimate_context_chars: total_chars,
        context_budget_chars: budget,
        context_budget_tokens: token_budget,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: path,
        latest_plan_event,
        preheat,
        session_obs,
        live: super::types::ContextLiveMetrics::default(),
    })
}

/// Messages from ContextState, ready for LLM (no system prompt).
pub fn build_context_from_state(state: &ContextState) -> Vec<ChatMessage> {
    state.messages.clone()
}
