//! init_context_state helpers and context assembly functions.

use std::path::PathBuf;

use chrono::{NaiveDate, Utc};

use crate::core::llm::{ChatMessage, ChatMessageContent, ChatMessageRole, MessageKind};
use crate::core::session::transcript::{read_entries_tail, BranchSummaryEntry, TranscriptEntry};
use crate::infra::config::{compute_context_budget_chars, ContextConfig};
use crate::infra::error::AppError;

use super::session_impl::generate_entry_id;
use super::session_impl::SessionManager;
use crate::core::compaction::preheat::Preheat;

use super::types::{estimate_msg_chars, CompactionResult, ContextState, SessionContextObservation};

const DEFAULT_CONTEXT_CAP: usize = 10;

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
    let role_str = me.message.get("role").and_then(|r| r.as_str())?;
    let role = match role_str {
        "user" => ChatMessageRole::User,
        "assistant" => ChatMessageRole::Assistant,
        "tool" => ChatMessageRole::Tool,
        "system" => ChatMessageRole::System,
        _ => return None,
    };

    let content = me
        .message
        .get("content")
        .and_then(|c| c.as_str())
        .map(|s| ChatMessageContent::Text(s.to_string()));

    let tool_calls = me
        .message
        .get("tool_calls")
        .and_then(|v| v.as_array())
        .map(|arr| arr.to_vec());

    if let Some(ref arr) = tool_calls {
        warn_if_legacy_tool_name(arr);
    }

    let tool_call_id = me
        .message
        .get("tool_call_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut msg = ChatMessage {
        role,
        content,
        name: None,
        tool_calls,
        tool_call_id,
        msg_id: me.id.clone().or_else(|| Some(generate_entry_id())),
        kind: MessageKind::Normal,
        timestamp: Some(me.timestamp.clone()),
    };

    // Detect compaction summary injected as user message (via older paths)
    // These are plain user messages from transcript — no special marking needed here.
    // The CompactionSummary kind is only set for BranchSummary entries below.
    let _ = &mut msg;
    Some(msg)
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

/// 从 transcript 加载历史，以 ChatMessage 列表初始化 ContextState。
/// 识别已有 Compaction entry 折叠为 CompactionSummary 消息，避免重复压缩。
/// 按天筛选：优先取当天所有 turns，不足 DEFAULT_CONTEXT_CAP 则向前补齐。
pub fn init_context_state(
    session: &SessionManager,
    config: &ContextConfig,
    system_text: &str,
) -> Result<ContextState, AppError> {
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
            return Ok(ContextState {
                messages: Vec::new(),
                estimate_context_chars: system_text.len(),
                context_budget_chars: budget,
                context_budget_tokens: token_budget,
                last_api_usage: None,
                post_usage_appended_chars: 0,
                transcript_path: PathBuf::new(),
                preheat: Preheat::new(),
                session_obs,
                live: super::types::ContextLiveMetrics::default(),
            });
        }
    };
    // TODO: 这里读取了 transcript 的最后 2000 条 entries，需要优化为只读取当天 entries。
    let entries = read_entries_tail(&path, super::BRANCH_MAX_ENTRIES)?;
    let today = Utc::now().date_naive();

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

    Ok(ContextState {
        messages: selected,
        estimate_context_chars: total_chars,
        context_budget_chars: budget,
        context_budget_tokens: token_budget,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: path,
        preheat,
        session_obs,
        live: super::types::ContextLiveMetrics::default(),
    })
}

/// Messages from ContextState, ready for LLM (no system prompt).
pub fn build_context_from_state(state: &ContextState) -> Vec<ChatMessage> {
    state.messages.clone()
}
