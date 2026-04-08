//! init_context_state helpers and context assembly functions.

use std::path::PathBuf;

use chrono::{NaiveDate, Utc};

use crate::core::agent_loop::{AgentMessage, ToolCallInfo};
use crate::core::session::transcript::{read_entries_tail, CompactionEntry, TranscriptEntry};
use crate::infra::config::{compute_context_budget_chars, ContextConfig};
use crate::infra::error::AppError;

use super::session_impl::generate_entry_id;
use super::session_impl::SessionManager;
use crate::core::compaction::preheat::Preheat;

use super::types::{estimate_turn_chars, CompactionResult, ContextState, TurnEntry};

const DEFAULT_CONTEXT_CAP: usize = 10;

fn entry_timestamp(entry: &TranscriptEntry) -> &str {
    match entry {
        TranscriptEntry::Message(e) => &e.timestamp,
        TranscriptEntry::Compaction(e) => &e.timestamp,
        TranscriptEntry::ModelChange(e) => &e.timestamp,
        TranscriptEntry::ThinkingLevelChange(e) => &e.timestamp,
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
        |e| matches!(e, TranscriptEntry::Compaction(ce) if ce.is_boundary == Some(true)),
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

fn compaction_pending_from_entry(ce: &CompactionEntry) -> Option<CompactionResult> {
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

/// Phase 2: 将 entries 折叠为带 timestamp 的 TurnEntry 列表。
/// boundary compaction 仍会清除之前的 turns。
/// `pending_preheat`：切片内最后一条未应用 preheat（`is_boundary=false` 且字段齐全）。
pub(super) struct FoldEntriesOutcome {
    pub turns: Vec<TurnEntry>,
    /// 折叠后全量 turns 的字符估计（未经过 `filter_turns_by_day`）；供调试或后续与 `selected` 对齐用。
    #[allow(dead_code)]
    pub total_chars: usize,
    pub pending_preheat: Option<CompactionResult>,
}

fn fold_entries_to_turns(
    entries: &[TranscriptEntry],
    system_text_len: usize,
) -> FoldEntriesOutcome {
    let mut turns: Vec<TurnEntry> = Vec::new();
    let mut current_turn_msgs: Vec<AgentMessage> = Vec::new();
    let mut current_turn_ts = String::new();
    // 与 transcript Message 首条 user 的 id 对齐，供 preheat covered_* reload 后仍可匹配。
    let mut current_turn_id: Option<String> = None;
    let mut total_chars = system_text_len;
    let mut pending_preheat: Option<CompactionResult> = None;

    for entry in entries {
        match entry {
            TranscriptEntry::Compaction(ce) => {
                // is_boundary=false → preheat record: skip during reload
                if ce.is_boundary == Some(false) {
                    if let Some(r) = compaction_pending_from_entry(ce) {
                        pending_preheat = Some(r);
                    }
                    continue;
                }

                if ce.is_boundary == Some(true) {
                    pending_preheat = None;
                }

                if !current_turn_msgs.is_empty() {
                    let turn = TurnEntry::UserTurn {
                        id: current_turn_id.take().unwrap_or_else(generate_entry_id),
                        messages: std::mem::take(&mut current_turn_msgs),
                        timestamp: std::mem::take(&mut current_turn_ts),
                    };
                    total_chars += estimate_turn_chars(&turn);
                    turns.push(turn);
                }

                // is_boundary=true → discard prefix (boundary switch)
                // is_boundary=None → legacy entry, don't clear (backward compat)
                if ce.is_boundary == Some(true) {
                    turns.clear();
                    total_chars = system_text_len;
                    current_turn_id = None;
                }

                if let Some(ref summary) = ce.summary {
                    total_chars += summary.len();
                    turns.push(TurnEntry::SummaryTurn {
                        id: ce.id.clone().unwrap_or_else(generate_entry_id),
                        summary: summary.clone(),
                        timestamp: ce.timestamp.clone(),
                    });
                }
            }
            TranscriptEntry::Message(me) => {
                let role = me.message.get("role").and_then(|r| r.as_str());
                let content = me
                    .message
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("");

                if role == Some("user") && !current_turn_msgs.is_empty() {
                    let turn = TurnEntry::UserTurn {
                        id: current_turn_id.take().unwrap_or_else(generate_entry_id),
                        messages: std::mem::take(&mut current_turn_msgs),
                        timestamp: std::mem::take(&mut current_turn_ts),
                    };
                    total_chars += estimate_turn_chars(&turn);
                    turns.push(turn);
                }

                if role == Some("user") {
                    current_turn_ts = me.timestamp.clone();
                    current_turn_id = Some(me.id.clone().unwrap_or_else(generate_entry_id));
                }

                let agent_msg = match role {
                    Some("user") => AgentMessage::User {
                        text: content.to_string(),
                    },
                    Some("assistant") => {
                        let tool_calls = me
                            .message
                            .get("tool_calls")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| {
                                        let obj = v.as_object()?;
                                        let id = obj.get("id")?.as_str()?.to_string();
                                        let func = obj.get("function")?.as_object()?;
                                        let name = func.get("name")?.as_str()?.to_string();
                                        let arguments = func
                                            .get("arguments")
                                            .and_then(|a| a.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        Some(ToolCallInfo {
                                            id,
                                            name,
                                            arguments,
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        AgentMessage::Assistant {
                            text: content.to_string(),
                            tool_calls,
                        }
                    }
                    Some("tool") => AgentMessage::ToolResult {
                        tool_call_id: me
                            .message
                            .get("tool_call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        content: content.to_string(),
                        is_error: false,
                    },
                    _ => continue,
                };
                current_turn_msgs.push(agent_msg);
            }
            _ => {}
        }
    }

    if !current_turn_msgs.is_empty() {
        let turn = TurnEntry::UserTurn {
            id: current_turn_id.take().unwrap_or_else(generate_entry_id),
            messages: std::mem::take(&mut current_turn_msgs),
            timestamp: current_turn_ts,
        };
        total_chars += estimate_turn_chars(&turn);
        turns.push(turn);
    }

    FoldEntriesOutcome {
        turns,
        total_chars,
        pending_preheat,
    }
}

/// Phase 3: 按天筛选 turns + 不足 min_turns 向前补齐。
pub(super) fn filter_turns_by_day(
    all_turns: Vec<TurnEntry>,
    today: NaiveDate,
    min_turns: usize,
) -> Vec<TurnEntry> {
    let today_start = all_turns
        .iter()
        .position(|t| parse_date(t.timestamp()) == Some(today));

    let mut selected = match today_start {
        Some(i) => all_turns[i..].to_vec(),
        None => vec![],
    };

    if selected.len() < min_turns {
        let before = today_start.unwrap_or(all_turns.len());
        let need = min_turns - selected.len();
        let extra: Vec<_> = all_turns[..before]
            .iter()
            .rev()
            .take(need)
            .cloned()
            .collect();
        let mut result: Vec<_> = extra.into_iter().rev().collect();
        result.append(&mut selected);
        selected = result;
    }

    selected
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

/// 从 transcript 加载历史，按 user turn 分组初始化 ContextState。
/// 识别已有 Compaction entry 折叠为 SummaryTurn，避免重复压缩。
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
    let session_obs = super::types::SessionContextObservation {
        compaction_count: cc,
        compaction_tokens_freed: ctf,
        tool_result_chars_persisted: trcp,
    };

    let path = match session.current_transcript_path()? {
        Some(p) => p,
        None => {
            return Ok(ContextState {
                user_turns_list: Vec::new(),
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

    let entries = read_entries_tail(&path, super::BRANCH_MAX_ENTRIES)?;
    let today = Utc::now().date_naive();

    let fold_start = compute_fold_start(&entries, today, DEFAULT_CONTEXT_CAP);
    let fold_out = fold_entries_to_turns(&entries[fold_start..], system_text.len());
    let selected = filter_turns_by_day(fold_out.turns, today, DEFAULT_CONTEXT_CAP);

    let total_chars = system_text.len() + selected.iter().map(estimate_turn_chars).sum::<usize>();

    let mut preheat = Preheat::new();
    if let Some(p) = fold_out.pending_preheat {
        if selected.iter().any(|t| t.id() == p.covered_end_id) {
            preheat.restore_completed(p);
        }
    }

    Ok(ContextState {
        user_turns_list: selected,
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

/// 将 ContextState 中的 turns 展平为 AgentMessage 列表（不含 system prompt）。
pub fn build_context_from_state(state: &ContextState) -> Vec<AgentMessage> {
    let mut out = Vec::new();
    for turn in &state.user_turns_list {
        match turn {
            TurnEntry::UserTurn { messages, .. } => out.extend(messages.iter().cloned()),
            TurnEntry::SummaryTurn { summary, .. } => {
                out.push(AgentMessage::CompactionSummary {
                    summary: summary.clone(),
                });
            }
        }
    }
    out
}
