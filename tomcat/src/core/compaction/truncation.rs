//! Layer 0: 超大 tool result 落盘 + preview 占位符 & Layer 1: 占位符替换。

use std::path::Path;

use tracing::info;

use crate::core::llm::{ChatMessageContent, ChatMessageRole};
use crate::core::session::manager::ContextState;
use crate::infra::config::ContextConfig;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub(crate) const TOOL_RESULT_PLACEHOLDER: &str =
    "[Previous tool result replaced to save context space]";

const LAYER0_PREVIEW_CHARS: usize = 500;
const TOOL_RESULT_PERSISTED_PREFIX: &str = "[Tool result persisted:";

pub(crate) fn floor_char_boundary(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut p = pos;
    while p > 0 && !s.is_char_boundary(p) {
        p -= 1;
    }
    p
}

// ---------------------------------------------------------------------------
// Layer 0 V2: Persist large tool results to disk
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PersistedResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub original_chars: usize,
    pub persisted_path: String,
}

pub(crate) fn is_persisted_tool_result_text(text: &str) -> bool {
    text.starts_with(TOOL_RESULT_PERSISTED_PREFIX)
}

/// 从本来就已落盘的工具结果里找回可再次读取的路径。
///
/// Layer 1 不负责再复制一份结果到磁盘；它只避免把 bash/task_output 已经返回的
/// `Full log:` 或 JSON `logPath` 一并抹掉。read/search 的源路径仍在 tool call 参数，
/// 不应在这里重复塞进上下文。
fn existing_result_path(text: &str) -> Option<&str> {
    if let Some(path) = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("Full log:").map(str::trim))
        .filter(|path| !path.is_empty())
    {
        return Some(path);
    }

    for marker in ["\"logPath\":\"", "\"log_path\":\""] {
        if let Some(rest) = text.split_once(marker).map(|(_, rest)| rest) {
            if let Some((path, _)) = rest.split_once('"') {
                if !path.is_empty() {
                    return Some(path);
                }
            }
        }
    }
    None
}

fn tool_result_placeholder(content: &str) -> String {
    match existing_result_path(content) {
        Some(path) => format!("[Previous tool result replaced; full log at {path}]"),
        None => TOOL_RESULT_PLACEHOLDER.to_string(),
    }
}

pub(crate) fn persist_tool_result_text(
    content: &mut String,
    tool_call_id: &str,
    work_dir: &Path,
    session_id: &str,
    single_max: usize,
) -> Option<(PersistedResult, usize)> {
    if work_dir.as_os_str().is_empty()
        || content.len() < single_max
        || is_persisted_tool_result_text(content)
    {
        return None;
    }

    let persist_dir = work_dir.join("tool-results").join(session_id);
    if std::fs::create_dir_all(&persist_dir).is_err() {
        return None;
    }

    let file_path = persist_dir.join(format!("{}.txt", tool_call_id));
    if std::fs::write(&file_path, content.as_bytes()).is_err() {
        return None;
    }

    let original_len = content.len();
    let path_str = file_path.to_string_lossy().to_string();
    let preview_end = floor_char_boundary(content, LAYER0_PREVIEW_CHARS);
    let preview = &content[..preview_end];
    let replacement = format!(
        "[Tool result persisted: {} ({} chars)]\nPreview: {}...",
        path_str, original_len, preview
    );
    let chars_freed = original_len.saturating_sub(replacement.len());
    *content = replacement;

    Some((
        PersistedResult {
            tool_call_id: tool_call_id.to_string(),
            tool_name: String::new(),
            original_chars: original_len,
            persisted_path: path_str,
        },
        chars_freed,
    ))
}

/// L0 步骤 A+B 的汇总。
///
/// 正常路径仅在 Layer 2 boundary 成功应用后运行；current-tail guard 的保命路径也会
/// 复用此结果，但不能把它误读为每次 timing ⑤ 都会触发的清理。
#[derive(Debug, Clone, Default)]
pub struct Layer0CleanupOutcome {
    pub persisted: Vec<PersistedResult>,
    /// 落盘替换为 preview 后减少的字符数之和。
    pub persist_chars_freed: usize,
    /// compactable zone 占位符替换减少的字符数。
    pub placeholder_chars_freed: usize,
    /// 正文已经从上下文里消失的那些 tool_call_id（落盘 + 占位符两批）。
    /// 调用方据此让对应的 read stamp 失效，避免 dedup 指向一段看不见的内容。
    pub evicted_tool_call_ids: Vec<String>,
}

/// Layer 0 步骤 A：超大 tool result 落盘 + preview 占位符。
/// 只在 `messages[..history_end]` 中扫描最新一个包含 tool 消息的 UserTurn，单条 >=
/// `layer0_single_result_max_chars` 时落盘。
/// 请求前刚追加的 user 消息还没有工具结果；跳过它，才能在 loop 顶部应用 boundary 时仍清理
/// 前一已完成 tool round 的大结果。
pub fn layer0_persist_large_results(
    state: &mut ContextState,
    config: &ContextConfig,
    work_dir: &Path,
    session_id: &str,
    history_end: usize,
) -> (Vec<PersistedResult>, usize) {
    let mut results = Vec::new();
    let mut persist_chars_freed = 0usize;
    let single_max = config.layer0_single_result_max_chars;
    let history_end = history_end.min(state.messages.len());

    // Find the most recent logical turn that actually contains a tool result. A new user
    // message may already be in the outgoing request when a preheated boundary is applied,
    // but it cannot have tool output yet.
    let turn_starts: Vec<usize> = state
        .messages
        .iter()
        .take(history_end)
        .enumerate()
        .filter(|(_, message)| message.starts_logical_turn())
        .map(|(index, _)| index)
        .collect();
    let mut next_turn_start = history_end;
    let mut latest_tool_turn = None;
    for &turn_start in turn_starts.iter().rev() {
        if state.messages[turn_start..next_turn_start]
            .iter()
            .any(|message| message.role == ChatMessageRole::Tool)
        {
            latest_tool_turn = Some((turn_start, next_turn_start));
            break;
        }
        next_turn_start = turn_start;
    }
    let Some((last_turn_start, last_turn_end)) = latest_tool_turn else {
        return (results, persist_chars_freed);
    };

    for msg in state.messages[last_turn_start..last_turn_end].iter_mut() {
        if msg.role != ChatMessageRole::Tool {
            continue;
        }

        let content = match &mut msg.content {
            Some(ChatMessageContent::Text(s)) => s,
            _ => continue,
        };

        let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
        if let Some((result, freed)) =
            persist_tool_result_text(content, &tool_call_id, work_dir, session_id, single_max)
        {
            persist_chars_freed += freed;
            state.estimate_context_chars = state.estimate_context_chars.saturating_sub(freed);
            results.push(result);
        }
    }
    if persist_chars_freed > 0 {
        let persisted_tool_call_ids = results
            .iter()
            .map(|result| result.tool_call_id.as_str())
            .collect::<Vec<_>>();
        info!(
            target: "tomcat_chat_diag",
            phase = "history_rewritten",
            operation = "layer0_persist_large_results",
            chars_freed = persist_chars_freed,
            ?persisted_tool_call_ids,
        );
    }
    (results, persist_chars_freed)
}

// ---------------------------------------------------------------------------
// Layer 1: Tool result placeholder replacement
// ---------------------------------------------------------------------------

/// [`compact_tool_results`] 的结果：省下多少字符，以及正文被抹掉的是哪几次工具调用。
#[derive(Debug, Clone, Default)]
pub struct PlaceholderOutcome {
    pub chars_freed: usize,
    pub tool_call_ids: Vec<String>,
}

/// Layer 1：从 `history_end` 之前的 compactable zone（排除最近
/// `config.keep_recent_turns` 个 turns）中，
/// 将长度 **大于** `ContextConfig::layer0_placeholder_threshold_chars`（默认 10_000）的 tool result 替换为占位符。
pub fn compact_tool_results(
    state: &mut ContextState,
    config: &ContextConfig,
    history_end: usize,
) -> PlaceholderOutcome {
    let threshold = config.layer0_placeholder_threshold_chars;
    let protected_turns = config.keep_recent_turns;
    let history_end = history_end.min(state.messages.len());

    // Find the start of the protected tail turns.
    let protected_start =
        find_protected_turn_start(&state.messages, protected_turns).min(history_end);
    if protected_start == 0 {
        return PlaceholderOutcome::default();
    }

    let mut outcome = PlaceholderOutcome::default();

    for msg in state.messages[..protected_start].iter_mut() {
        if msg.role != ChatMessageRole::Tool {
            continue;
        }

        let content = match &mut msg.content {
            Some(ChatMessageContent::Text(s)) => s,
            _ => continue,
        };

        if content.len() <= threshold {
            continue;
        }
        if is_persisted_tool_result_text(content) || content == TOOL_RESULT_PLACEHOLDER {
            continue;
        }

        let old_len = content.len();
        let replacement = tool_result_placeholder(content);
        let reduced = old_len.saturating_sub(replacement.len());
        *content = replacement;
        state.estimate_context_chars = state.estimate_context_chars.saturating_sub(reduced);
        outcome.chars_freed += reduced;
        if let Some(id) = msg.tool_call_id.clone() {
            outcome.tool_call_ids.push(id);
        }
    }
    if outcome.chars_freed > 0 {
        info!(
            target: "tomcat_chat_diag",
            phase = "history_rewritten",
            operation = "compact_tool_results",
            chars_freed = outcome.chars_freed,
            tool_call_ids = ?outcome.tool_call_ids,
        );
    }
    outcome
}

/// 返回「最后 m 个 turns」的起始消息索引（即第 `(total_turns - m)` 个 turn-start 的位置）。
/// `m == 0` 时表示不保护任何 turn，返回 `messages.len()`；若 `turns <= m`，返回 0（整个列表均受保护）。
fn find_protected_turn_start(messages: &[crate::core::llm::ChatMessage], m: usize) -> usize {
    if m == 0 {
        return messages.len();
    }

    // Collect all turn-start indices in order.
    let turn_starts: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, msg)| msg.starts_logical_turn())
        .map(|(i, _)| i)
        .collect();

    let total_turns = turn_starts.len();
    if total_turns <= m {
        return 0;
    }

    turn_starts[total_turns - m]
}

// ---------------------------------------------------------------------------
// run_layer0_cleanup: Combined L0 persist + L1 placeholder (TASK-20)
// ---------------------------------------------------------------------------

/// TASK-20: L0 步骤 A（最后一历史 turn 落盘）+ 步骤 B（历史 compactable zone 占位符替换）。
///
/// 正常调用方是成功应用 boundary 后的结构性后续步骤；current-tail guard 仅在已确认的
/// 溢出风险下复用它作为保命路径。
pub fn run_layer0_cleanup(
    state: &mut ContextState,
    config: &ContextConfig,
    work_dir: &Path,
    session_id: &str,
    history_end: usize,
) -> Layer0CleanupOutcome {
    let (persisted, persist_chars_freed) =
        layer0_persist_large_results(state, config, work_dir, session_id, history_end);
    let placeholder = compact_tool_results(state, config, history_end);
    let evicted_tool_call_ids = persisted
        .iter()
        .map(|p| p.tool_call_id.clone())
        .chain(placeholder.tool_call_ids.iter().cloned())
        .filter(|id| !id.is_empty())
        .collect();
    Layer0CleanupOutcome {
        persisted,
        persist_chars_freed,
        placeholder_chars_freed: placeholder.chars_freed,
        evicted_tool_call_ids,
    }
}
