//! 上下文 Compaction 四层防护算法 V2。
//!
//! - Layer 0: 超大 tool result 落盘 + preview 占位符（保全信息）
//! - Layer 1: compactable zone 内 tool result > 20K 占位符替换（零 LLM 开销）
//! - Layer 2: LLM 一次性摘要 compactable zone（按 m 值保护最近 turns）
//! - Layer 3: 强制删除最旧 turn 到 ratio < 0.50 兜底
//!
//! 由 ratio 水位线驱动级联降压：每层执行后重算 ratio，降压成功即停。

use std::path::Path;

use crate::core::session::manager::{estimate_turn_chars, ContextState, TurnEntry};
use crate::core::session::transcript::{append_entry, CompactionEntry, TranscriptEntry};
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const TRUNCATION_SUFFIX: &str = "\n\n[truncated — original content exceeded limit]";

const TOOL_RESULT_PLACEHOLDER: &str = "[Previous tool result replaced to save context space]";

const LAYER1_TOOL_RESULT_THRESHOLD: usize = 20_000;

const LAYER0_PREVIEW_CHARS: usize = 500;

pub const SUMMARIZATION_PROMPT: &str = r#"You are a context compaction assistant. Summarize the following conversation segment into a structured format. Preserve all critical information needed for the AI assistant to continue working effectively.

Output format:
## Goal
What the user is trying to accomplish.

## Constraints
Any rules, preferences, or constraints mentioned.

## Progress
What has been done so far (key actions, tool calls, results).

## Key Decisions
Important decisions made and their rationale.

## Critical Context
File paths, variable names, error messages, or other specific details that must be preserved."#;

pub const UPDATE_SUMMARIZATION_PROMPT: &str = r#"You are a context compaction assistant. You have an existing summary and a new conversation segment. Merge them into a single updated summary, keeping the same structured format. Drop information that is no longer relevant, and add new information from the recent segment.

Existing summary:
{existing_summary}

Output format:
## Goal
## Constraints
## Progress
## Key Decisions
## Critical Context"#;

// ---------------------------------------------------------------------------
// Layer 0: Single tool result truncation (legacy fallback)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct TruncationInfo {
    pub original_chars: usize,
    pub truncated_chars: usize,
}

fn floor_char_boundary(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut p = pos;
    while p > 0 && !s.is_char_boundary(p) {
        p -= 1;
    }
    p
}

/// Layer 0 fallback：若 `content` 超过 `max_chars` 则就地截断。
pub fn truncate_tool_result_if_needed(
    content: &mut String,
    max_chars: usize,
) -> Option<TruncationInfo> {
    if content.len() <= max_chars {
        return None;
    }
    let original_len = content.len();
    let safe_max = floor_char_boundary(content, max_chars);
    let safe_zone_start = floor_char_boundary(content, max_chars * 70 / 100);
    let cut_pos = content[safe_zone_start..safe_max]
        .rfind('\n')
        .map(|i| safe_zone_start + i)
        .unwrap_or(safe_max);
    content.truncate(cut_pos);
    content.push_str(TRUNCATION_SUFFIX);
    Some(TruncationInfo {
        original_chars: original_len,
        truncated_chars: content.len(),
    })
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

/// Layer 0 V2：超大 tool result 落盘 + preview 占位符。
/// 遍历最后一个 UserTurn 的 messages，满足条件 A（单条 >= threshold）
/// 或条件 B（单 turn 合计 >= aggregate threshold）时落盘。
pub fn layer0_persist_large_results(
    state: &mut ContextState,
    config: &ContextConfig,
    work_dir: &Path,
    session_id: &str,
) -> Vec<PersistedResult> {
    let mut results = Vec::new();
    let single_max = config.layer0_single_result_max_chars;
    let agg_max = config.layer0_turn_aggregate_max_chars;

    let last_turn = match state.user_turns_list.last_mut() {
        Some(TurnEntry::UserTurn { messages }) => messages,
        _ => return results,
    };

    let total_tool_chars: usize = last_turn
        .iter()
        .filter_map(|m| {
            if let crate::core::agent_loop::AgentMessage::ToolResult { content, .. } = m {
                Some(content.len())
            } else {
                None
            }
        })
        .sum();

    let needs_aggregate = total_tool_chars >= agg_max;

    for msg in last_turn.iter_mut() {
        if let crate::core::agent_loop::AgentMessage::ToolResult {
            tool_call_id,
            content,
            ..
        } = msg
        {
            let should_persist =
                content.len() >= single_max || (needs_aggregate && content.len() > LAYER0_PREVIEW_CHARS);

            if !should_persist {
                continue;
            }

            let persist_dir = work_dir
                .join("agents")
                .join(session_id)
                .join("tool-results");

            if std::fs::create_dir_all(&persist_dir).is_err() {
                continue;
            }

            let file_path = persist_dir.join(format!("{}.txt", tool_call_id));
            if std::fs::write(&file_path, content.as_bytes()).is_err() {
                continue;
            }

            let original_len = content.len();
            let path_str = file_path.to_string_lossy().to_string();

            let preview_end = floor_char_boundary(content, LAYER0_PREVIEW_CHARS);
            let preview = &content[..preview_end];
            let replacement = format!(
                "[Tool result persisted: {} ({} chars)]\nPreview: {}...",
                path_str, original_len, preview
            );

            let new_len = replacement.len();
            *content = replacement;

            let freed = original_len.saturating_sub(new_len);
            state.estimate_context_chars = state.estimate_context_chars.saturating_sub(freed);

            results.push(PersistedResult {
                tool_call_id: tool_call_id.clone(),
                tool_name: String::new(),
                original_chars: original_len,
                persisted_path: path_str,
            });
        }
    }
    results
}

// ---------------------------------------------------------------------------
// Layer 1: Tool result placeholder replacement
// ---------------------------------------------------------------------------

/// Layer 1：从 compactable zone（排除最近 `m` 个 turns）中，
/// 将 > LAYER1_TOOL_RESULT_THRESHOLD 的 tool result 替换为占位符。
pub fn compact_tool_results(state: &mut ContextState, m: usize) -> usize {
    let len = state.user_turns_list.len();
    if len <= m {
        return 0;
    }
    let compactable_end = len - m;
    let mut total_reduced = 0usize;

    for turn in state.user_turns_list[..compactable_end].iter_mut() {
        if let TurnEntry::UserTurn { messages } = turn {
            for msg in messages.iter_mut() {
                if let crate::core::agent_loop::AgentMessage::ToolResult { content, .. } = msg {
                    if content.len() <= LAYER1_TOOL_RESULT_THRESHOLD {
                        continue;
                    }
                    if content.starts_with("[Tool result persisted:")
                        || content == TOOL_RESULT_PLACEHOLDER
                    {
                        continue;
                    }
                    let old_len = content.len();
                    let reduced = old_len - TOOL_RESULT_PLACEHOLDER.len();
                    *content = TOOL_RESULT_PLACEHOLDER.to_string();
                    state.estimate_context_chars =
                        state.estimate_context_chars.saturating_sub(reduced);
                    total_reduced += reduced;
                }
            }
        }
    }
    total_reduced
}

// ---------------------------------------------------------------------------
// Layer 2: LLM-driven compaction (single-shot by m value)
// ---------------------------------------------------------------------------

use crate::core::llm::{ChatMessage, ChatRequest, LlmProvider};

/// Layer 2：一次性将 compactable zone [0..compactable_end) 压缩为结构化摘要。
pub async fn run_compaction(
    state: &mut ContextState,
    llm: &dyn LlmProvider,
    config: &ContextConfig,
    session_path: &Path,
    m: usize,
) -> Result<(), AppError> {
    if state.compaction_consecutive_failures >= 3 {
        return Ok(());
    }

    let len = state.user_turns_list.len();
    if len <= m {
        return Ok(());
    }
    let compactable_end = len - m;
    if compactable_end == 0 {
        return Ok(());
    }

    let existing_summary = find_last_summary(&state.user_turns_list[..compactable_end]);
    let batch_text = turns_to_text(&state.user_turns_list[..compactable_end]);
    let old_batch_chars: usize = state.user_turns_list[..compactable_end]
        .iter()
        .map(estimate_turn_chars)
        .sum();

    let (summary_text, actual_start, actual_end) =
        match generate_or_update_summary(llm, config, &batch_text, existing_summary.as_deref())
            .await
        {
            Ok(s) if !s.is_empty() && s.len() < old_batch_chars => (s, 0, compactable_end),
            Ok(_) => {
                state.compaction_consecutive_failures += 1;
                return Ok(());
            }
            Err(e) if is_context_overflow_error(&e.to_string()) => {
                match retry_with_half_range(state, llm, config, compactable_end).await {
                    Ok(result) => result,
                    Err(_) => {
                        state.compaction_consecutive_failures += 1;
                        return Ok(());
                    }
                }
            }
            Err(_) => {
                state.compaction_consecutive_failures += 1;
                return Ok(());
            }
        };

    let summary_chars = summary_text.len();
    let is_boundary = actual_start == 0;

    let compaction_entry = TranscriptEntry::Compaction(CompactionEntry {
        id: None,
        parent_id: None,
        timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        summary: Some(summary_text.clone()),
        covered_start_id: None,
        covered_end_id: None,
        covered_count: Some(actual_end - actual_start),
        is_boundary: Some(is_boundary),
    });
    let _ = append_entry(session_path, &compaction_entry);

    let removed_chars: usize = state.user_turns_list[actual_start..actual_end]
        .iter()
        .map(estimate_turn_chars)
        .sum();
    state.user_turns_list.drain(actual_start..actual_end);
    state.estimate_context_chars = state.estimate_context_chars.saturating_sub(removed_chars);

    let new_turn = TurnEntry::SummaryTurn {
        summary: summary_text,
    };
    state.estimate_context_chars += summary_chars;
    state.user_turns_list.insert(actual_start, new_turn);

    state.compaction_consecutive_failures = 0;
    state.invalidate_api_usage();
    Ok(())
}

/// Legacy Layer 2：循环调用 LLM 将最旧 turns 压缩为结构化摘要。
/// 保留供 fallback 和向后兼容。
pub async fn run_compaction_loop(
    state: &mut ContextState,
    llm: &dyn LlmProvider,
    config: &ContextConfig,
    session_path: &Path,
) -> Result<(), AppError> {
    loop {
        if !state.is_over_budget() {
            break;
        }
        let len = state.user_turns_list.len();
        if len <= config.keep_recent_turns {
            break;
        }
        let compactable_end = len - config.keep_recent_turns;
        if compactable_end == 0 {
            break;
        }

        let batch_size = config.compaction_turns.min(compactable_end);
        if batch_size == 0 {
            break;
        }

        let existing_summary = find_last_summary(&state.user_turns_list[..batch_size]);
        let batch_text = turns_to_text(&state.user_turns_list[..batch_size]);
        let old_batch_chars: usize = state.user_turns_list[..batch_size]
            .iter()
            .map(estimate_turn_chars)
            .sum();

        let summary = match generate_or_update_summary(
            llm,
            config,
            &batch_text,
            existing_summary.as_deref(),
        )
        .await
        {
            Ok(s) if !s.is_empty() && s.len() < old_batch_chars => s,
            Ok(_) => break,
            Err(_) => break,
        };

        let summary_chars = summary.len();

        let compaction_entry = TranscriptEntry::Compaction(CompactionEntry {
            id: None,
            parent_id: None,
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            summary: Some(summary.clone()),
            covered_start_id: None,
            covered_end_id: None,
            covered_count: Some(batch_size),
            is_boundary: None,
        });
        let _ = append_entry(session_path, &compaction_entry);

        state.user_turns_list.drain(..batch_size);
        state.estimate_context_chars = state.estimate_context_chars.saturating_sub(old_batch_chars);

        let new_turn = TurnEntry::SummaryTurn { summary };
        state.estimate_context_chars += summary_chars;
        state.user_turns_list.insert(0, new_turn);
    }
    Ok(())
}

fn find_last_summary(turns: &[TurnEntry]) -> Option<String> {
    turns.iter().rev().find_map(|t| {
        if let TurnEntry::SummaryTurn { summary } = t {
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
            TurnEntry::UserTurn { messages } => {
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
            TurnEntry::SummaryTurn { summary } => {
                buf.push_str("[Previous Summary]\n");
                buf.push_str(summary);
                buf.push('\n');
            }
        }
    }
    buf
}

async fn generate_or_update_summary(
    llm: &dyn LlmProvider,
    config: &ContextConfig,
    batch_text: &str,
    existing_summary: Option<&str>,
) -> Result<String, AppError> {
    let prompt = if let Some(existing) = existing_summary {
        UPDATE_SUMMARIZATION_PROMPT.replace("{existing_summary}", existing)
    } else {
        SUMMARIZATION_PROMPT.to_string()
    };

    let req = ChatRequest {
        model: config.compaction_model.clone(),
        messages: vec![
            ChatMessage::system(&prompt),
            ChatMessage::user(batch_text),
        ],
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
    Ok(text)
}

// ---------------------------------------------------------------------------
// PTL retry: retry with newer half on context overflow
// ---------------------------------------------------------------------------

async fn retry_with_half_range(
    state: &mut ContextState,
    llm: &dyn LlmProvider,
    config: &ContextConfig,
    compactable_end: usize,
) -> Result<(String, usize, usize), AppError> {
    if compactable_end <= 1 {
        return Err(AppError::internal("compactable zone too small for PTL retry"));
    }
    let retry_end = compactable_end - 1;

    let mut range_start = retry_end / 2;
    for _attempt in 0..2 {
        if range_start >= retry_end {
            break;
        }
        let sub_batch = &state.user_turns_list[range_start..retry_end];
        let prev = find_last_summary(sub_batch);
        match generate_or_update_summary(
            llm,
            config,
            &turns_to_text(sub_batch),
            prev.as_deref(),
        )
        .await
        {
            Ok(text) if !text.is_empty() => return Ok((text, range_start, retry_end)),
            Err(e) if is_context_overflow_error(&e.to_string()) => {
                range_start = range_start + (retry_end - range_start) / 2;
            }
            _ => return Err(AppError::internal("PTL retry failed")),
        }
    }
    Err(AppError::internal("PTL retry exhausted"))
}

// ---------------------------------------------------------------------------
// Layer 3: Force drop oldest to target ratio
// ---------------------------------------------------------------------------

/// Layer 3 V2：强制删除最旧 turn 直到 ratio < 0.50。
pub fn force_drop_oldest_to_target(state: &mut ContextState) {
    while state.usage_ratio() >= 0.50 && !state.user_turns_list.is_empty() {
        let removed = state.user_turns_list.remove(0);
        let chars = estimate_turn_chars(&removed);
        state.estimate_context_chars = state.estimate_context_chars.saturating_sub(chars);
    }
    state.invalidate_api_usage();
}

/// Layer 3 legacy：强制删除最旧 turn 直到回预算。
pub fn force_drop_oldest(state: &mut ContextState) {
    while state.is_over_budget() && !state.user_turns_list.is_empty() {
        let removed = state.user_turns_list.remove(0);
        let chars = estimate_turn_chars(&removed);
        state.estimate_context_chars = state.estimate_context_chars.saturating_sub(chars);
    }
}

// ---------------------------------------------------------------------------
// Cascade params: ratio watermark logic
// ---------------------------------------------------------------------------

/// Cascade 参数：由 ratio 水位线决定。
#[derive(Debug, Clone)]
pub struct CascadeParams {
    pub should_cascade: bool,
    pub m: usize,
    pub block_tool_calls: bool,
    pub target_layer3: bool,
}

/// 根据当前 ratio 和 buffer 安全网决定 cascade 参数。
pub fn determine_cascade_params(
    state: &ContextState,
    config: &ContextConfig,
) -> CascadeParams {
    let ratio = state.usage_ratio();
    let input_budget = config.context_window.saturating_sub(config.max_output_tokens);
    let remaining = input_budget.saturating_sub(state.estimated_token_count());

    let buffer_cap = |val: usize| val.min(input_budget * 3 / 10);
    let autocompact_buf = buffer_cap(config.autocompact_buffer_tokens);
    let warning_buf = buffer_cap(config.warning_buffer_tokens);

    if ratio >= 1.0 {
        CascadeParams {
            should_cascade: true,
            m: 1,
            block_tool_calls: true,
            target_layer3: true,
        }
    } else if ratio >= 0.98 {
        CascadeParams {
            should_cascade: true,
            m: 1,
            block_tool_calls: true,
            target_layer3: false,
        }
    } else if ratio >= 0.92 || remaining < autocompact_buf {
        CascadeParams {
            should_cascade: true,
            m: 2,
            block_tool_calls: false,
            target_layer3: false,
        }
    } else if ratio >= 0.85 || remaining < warning_buf {
        CascadeParams {
            should_cascade: true,
            m: 3,
            block_tool_calls: false,
            target_layer3: false,
        }
    } else if ratio >= 0.70 {
        CascadeParams {
            should_cascade: true,
            m: 5,
            block_tool_calls: false,
            target_layer3: false,
        }
    } else {
        CascadeParams {
            should_cascade: false,
            m: 5,
            block_tool_calls: false,
            target_layer3: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Cascade result
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CascadeResult {
    pub layers_executed: Vec<u8>,
    pub ratio_before: f64,
    pub ratio_after: f64,
    pub block_tool_calls: bool,
    pub persisted_results: Vec<PersistedResult>,
}

// ---------------------------------------------------------------------------
// Compaction cascade V2: L0 → L1 → L2 → L3
// ---------------------------------------------------------------------------

/// V2 级联压缩：ratio 水位线驱动，逐层执行、每层后重算 ratio、降压成功即停。
pub async fn run_compaction_cascade_v2(
    state: &mut ContextState,
    llm: &dyn LlmProvider,
    config: &ContextConfig,
    transcript_path: &Path,
    work_dir: &Path,
    session_id: &str,
) -> CascadeResult {
    let ratio_before = state.usage_ratio();
    let mut layers_executed = Vec::new();

    // Layer 0: persist large results (always runs)
    let persisted_results = layer0_persist_large_results(state, config, work_dir, session_id);
    if !persisted_results.is_empty() {
        layers_executed.push(0);
    }

    // Check if cascade needed
    let mut params = determine_cascade_params(state, config);
    if !params.should_cascade {
        return CascadeResult {
            layers_executed,
            ratio_before,
            ratio_after: state.usage_ratio(),
            block_tool_calls: params.block_tool_calls,
            persisted_results,
        };
    }

    // Layer 1: placeholder replacement
    let reduced = compact_tool_results(state, params.m);
    if reduced > 0 {
        layers_executed.push(1);
    }
    params = determine_cascade_params(state, config);
    if !params.should_cascade {
        return CascadeResult {
            layers_executed,
            ratio_before,
            ratio_after: state.usage_ratio(),
            block_tool_calls: params.block_tool_calls,
            persisted_results,
        };
    }

    // Layer 2: LLM summarization (subject to circuit breaker)
    if state.compaction_consecutive_failures < 3 {
        let _ = run_compaction(state, llm, config, transcript_path, params.m).await;
        layers_executed.push(2);
        params = determine_cascade_params(state, config);
        if !params.should_cascade {
            return CascadeResult {
                layers_executed,
                ratio_before,
                ratio_after: state.usage_ratio(),
                block_tool_calls: params.block_tool_calls,
                persisted_results,
            };
        }
    }

    // Layer 3: force drop (only when ratio >= 1.0 or circuit breaker skip)
    if params.target_layer3 || state.compaction_consecutive_failures >= 3 {
        force_drop_oldest_to_target(state);
        layers_executed.push(3);
    }

    CascadeResult {
        layers_executed,
        ratio_before,
        ratio_after: state.usage_ratio(),
        block_tool_calls: params.block_tool_calls,
        persisted_results,
    }
}

/// Legacy 三层级联压缩（向后兼容，不使用 ratio 水位线）。
pub async fn run_compaction_cascade(
    state: &mut ContextState,
    llm: &dyn LlmProvider,
    config: &ContextConfig,
    transcript_path: &Path,
) {
    if state.is_over_budget() {
        compact_tool_results(state, config.keep_recent_turns);
    }
    if state.is_over_budget() {
        let _ = run_compaction_loop(state, llm, config, transcript_path).await;
    }
    if state.is_over_budget() {
        force_drop_oldest(state);
    }
}

// ---------------------------------------------------------------------------
// Helper: context overflow detection
// ---------------------------------------------------------------------------

/// 检测 LLM 错误消息是否表示 context overflow。
pub fn is_context_overflow_error(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("context")
        && (lower.contains("length") || lower.contains("token") || lower.contains("limit"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::agent_loop::AgentMessage;

    fn make_state(chars: usize, budget_chars: usize, budget_tokens: usize) -> ContextState {
        ContextState {
            user_turns_list: vec![],
            estimate_context_chars: chars,
            context_budget_chars: budget_chars,
            context_budget_tokens: budget_tokens,
            last_api_usage: None,
            post_usage_appended_chars: 0,
            compaction_consecutive_failures: 0,
        }
    }

    #[test]
    fn floor_char_boundary_ascii() {
        let s = "hello world";
        assert_eq!(floor_char_boundary(s, 5), 5);
        assert_eq!(floor_char_boundary(s, 100), s.len());
        assert_eq!(floor_char_boundary(s, 0), 0);
    }

    #[test]
    fn floor_char_boundary_multibyte() {
        let s = "你好世界"; // 4 chars, 12 bytes
        assert_eq!(floor_char_boundary(s, 3), 3);
        assert_eq!(floor_char_boundary(s, 4), 3);
        assert_eq!(floor_char_boundary(s, 5), 3);
        assert_eq!(floor_char_boundary(s, 6), 6);
    }

    #[test]
    fn truncate_noop_when_under_limit() {
        let mut s = "short".to_string();
        let info = truncate_tool_result_if_needed(&mut s, 1000);
        assert!(info.is_none());
        assert_eq!(s, "short");
    }

    #[test]
    fn truncate_works_on_large_content() {
        let mut s = "a\n".repeat(300_000);
        let info = truncate_tool_result_if_needed(&mut s, 400_000);
        assert!(info.is_some());
        let info = info.unwrap();
        assert!(info.truncated_chars < 400_000 + TRUNCATION_SUFFIX.len() + 10);
        assert!(s.ends_with(TRUNCATION_SUFFIX));
    }

    #[test]
    fn truncate_chinese_content_no_panic() {
        let mut s = "你好\n".repeat(200_000);
        let info = truncate_tool_result_if_needed(&mut s, 400_000);
        assert!(info.is_some());
        assert!(s.ends_with(TRUNCATION_SUFFIX));
    }

    #[test]
    fn truncate_exact_boundary() {
        let mut s = "x".repeat(400_000);
        let info = truncate_tool_result_if_needed(&mut s, 400_000);
        assert!(info.is_none());
    }

    #[test]
    fn compact_tool_results_reduces_budget() {
        let mut state = make_state(11_000, 5_000, 1_250);
        state.user_turns_list = vec![
            TurnEntry::UserTurn {
                messages: vec![
                    AgentMessage::User {
                        text: "q".to_string(),
                    },
                    AgentMessage::ToolResult {
                        tool_call_id: "c1".into(),
                        content: "x".repeat(25_000),
                        is_error: false,
                    },
                ],
            },
            TurnEntry::UserTurn {
                messages: vec![AgentMessage::User {
                    text: "q2".to_string(),
                }],
            },
        ];
        let reduced = compact_tool_results(&mut state, 1);
        assert!(reduced > 0);
    }

    #[test]
    fn compact_tool_results_protects_recent() {
        let tool_content = "x".repeat(25_000);
        let mut state = make_state(25_000, 5_000, 1_250);
        state.user_turns_list = vec![TurnEntry::UserTurn {
            messages: vec![AgentMessage::ToolResult {
                tool_call_id: "c1".into(),
                content: tool_content.clone(),
                is_error: false,
            }],
        }];
        let reduced = compact_tool_results(&mut state, 1);
        assert_eq!(reduced, 0);
    }

    #[test]
    fn compact_tool_results_skips_small() {
        let mut state = make_state(5_000, 3_000, 750);
        state.user_turns_list = vec![
            TurnEntry::UserTurn {
                messages: vec![AgentMessage::ToolResult {
                    tool_call_id: "c1".into(),
                    content: "x".repeat(1_000),
                    is_error: false,
                }],
            },
            TurnEntry::UserTurn {
                messages: vec![AgentMessage::User {
                    text: "q".to_string(),
                }],
            },
        ];
        let reduced = compact_tool_results(&mut state, 1);
        assert_eq!(reduced, 0);
    }

    #[test]
    fn force_drop_oldest_recovers_budget() {
        let mut state = make_state(6000, 2000, 500);
        state.user_turns_list = vec![
            TurnEntry::SummaryTurn {
                summary: "x".repeat(5000),
            },
            TurnEntry::UserTurn {
                messages: vec![AgentMessage::User {
                    text: "q".to_string(),
                }],
            },
        ];
        force_drop_oldest(&mut state);
        assert!(!state.is_over_budget());
    }

    #[test]
    fn force_drop_oldest_to_target_below_half() {
        let mut state = make_state(4000, 4000, 1000);
        state.user_turns_list = vec![
            TurnEntry::UserTurn {
                messages: vec![AgentMessage::User {
                    text: "x".repeat(2000),
                }],
            },
            TurnEntry::UserTurn {
                messages: vec![AgentMessage::User {
                    text: "y".repeat(1000),
                }],
            },
            TurnEntry::UserTurn {
                messages: vec![AgentMessage::User {
                    text: "z".repeat(500),
                }],
            },
        ];
        force_drop_oldest_to_target(&mut state);
        assert!(state.usage_ratio() < 0.50);
    }

    #[test]
    fn is_context_overflow_error_matches() {
        assert!(is_context_overflow_error(
            "context length exceeded: 500000 tokens"
        ));
        assert!(is_context_overflow_error(
            "maximum context token limit reached"
        ));
        assert!(!is_context_overflow_error("API error 429: rate limit"));
    }

    #[test]
    fn context_state_on_message_appended() {
        let mut state = make_state(100, 1000, 250);
        state.on_message_appended(500);
        assert_eq!(state.estimate_context_chars, 600);
        assert_eq!(state.post_usage_appended_chars, 500);
        assert!(!state.is_over_budget());
        state.on_message_appended(500);
        assert!(state.is_over_budget());
    }

    #[test]
    fn context_state_on_new_user_turn() {
        let mut state = make_state(0, 1000, 250);
        let turn = TurnEntry::UserTurn {
            messages: vec![AgentMessage::User {
                text: "hello".to_string(),
            }],
        };
        state.on_new_user_turn(turn);
        assert_eq!(state.user_turns_list.len(), 1);
        assert_eq!(state.estimate_context_chars, 5);
    }

    #[test]
    fn determine_cascade_params_below_threshold() {
        let state = make_state(100, 1000, 1000);
        let config = ContextConfig::default();
        let params = determine_cascade_params(&state, &config);
        assert!(!params.should_cascade);
    }

    #[test]
    fn determine_cascade_params_at_070() {
        let mut state = make_state(0, 0, 1000);
        state.update_api_usage(700, 0);
        let config = ContextConfig::default();
        let params = determine_cascade_params(&state, &config);
        assert!(params.should_cascade);
        assert_eq!(params.m, 5);
        assert!(!params.block_tool_calls);
    }

    #[test]
    fn determine_cascade_params_at_098() {
        let mut state = make_state(0, 0, 1000);
        state.update_api_usage(980, 0);
        let config = ContextConfig::default();
        let params = determine_cascade_params(&state, &config);
        assert!(params.should_cascade);
        assert_eq!(params.m, 1);
        assert!(params.block_tool_calls);
        assert!(!params.target_layer3);
    }

    #[test]
    fn determine_cascade_params_at_100() {
        let mut state = make_state(0, 0, 1000);
        state.update_api_usage(1000, 0);
        let config = ContextConfig::default();
        let params = determine_cascade_params(&state, &config);
        assert!(params.should_cascade);
        assert!(params.target_layer3);
    }

    #[test]
    fn determine_cascade_params_zero_budget() {
        let state = make_state(100, 100, 0);
        let config = ContextConfig::default();
        let params = determine_cascade_params(&state, &config);
        assert!(params.should_cascade);
        assert!(params.target_layer3);
    }

    #[test]
    fn layer0_persist_creates_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = make_state(50_000, 100_000, 25_000);
        let big_content = "x".repeat(40_000);
        state.user_turns_list = vec![TurnEntry::UserTurn {
            messages: vec![AgentMessage::ToolResult {
                tool_call_id: "tc_1".into(),
                content: big_content,
                is_error: false,
            }],
        }];
        let config = ContextConfig::default();
        let results = layer0_persist_large_results(&mut state, &config, dir.path(), "test_session");
        assert_eq!(results.len(), 1);
        assert!(std::path::Path::new(&results[0].persisted_path).exists());
        assert!(state.estimate_context_chars < 50_000);
        if let TurnEntry::UserTurn { messages } = &state.user_turns_list[0] {
            if let AgentMessage::ToolResult { content, .. } = &messages[0] {
                assert!(content.starts_with("[Tool result persisted:"));
            }
        }
    }

    #[test]
    fn layer0_persist_skips_small() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = make_state(1_000, 100_000, 25_000);
        state.user_turns_list = vec![TurnEntry::UserTurn {
            messages: vec![AgentMessage::ToolResult {
                tool_call_id: "tc_2".into(),
                content: "small".to_string(),
                is_error: false,
            }],
        }];
        let config = ContextConfig::default();
        let results = layer0_persist_large_results(&mut state, &config, dir.path(), "test_session");
        assert!(results.is_empty());
    }

    #[test]
    fn circuit_breaker_skips_layer2() {
        let mut state = make_state(100, 100, 100);
        state.compaction_consecutive_failures = 3;
        assert!(state.compaction_consecutive_failures >= 3);
    }

    // --- V2 新增测试 ---

    #[test]
    fn estimated_token_count_with_usage() {
        let mut state = make_state(0, 0, 1000);
        state.update_api_usage(500, 100);
        assert_eq!(state.estimated_token_count(), 600);
        state.on_message_appended(400);
        assert_eq!(state.estimated_token_count(), 700);
    }

    #[test]
    fn estimated_token_count_fallback_without_usage() {
        let state = make_state(4000, 10000, 1000);
        assert_eq!(state.estimated_token_count(), 1000);
    }

    #[test]
    fn usage_ratio_various_levels() {
        let mut state = make_state(0, 0, 1000);
        state.update_api_usage(700, 0);
        let r = state.usage_ratio();
        assert!((r - 0.70).abs() < 0.001);

        state.update_api_usage(850, 0);
        assert!((state.usage_ratio() - 0.85).abs() < 0.001);
    }

    #[test]
    fn usage_ratio_zero_budget_returns_max() {
        let state = make_state(100, 100, 0);
        assert_eq!(state.usage_ratio(), f64::MAX);
    }

    #[test]
    fn invalidate_api_usage_resets_to_fallback() {
        let mut state = make_state(2000, 10000, 1000);
        state.update_api_usage(800, 0);
        assert_eq!(state.estimated_token_count(), 800);
        state.invalidate_api_usage();
        assert_eq!(state.estimated_token_count(), 500);
    }

    #[test]
    fn compact_tool_results_skips_already_persisted() {
        let mut state = make_state(30_000, 5_000, 1_250);
        state.user_turns_list = vec![
            TurnEntry::UserTurn {
                messages: vec![AgentMessage::ToolResult {
                    tool_call_id: "c1".into(),
                    content: "[Tool result persisted: /tmp/foo.txt (50000 chars)]\nPreview: ...".to_string(),
                    is_error: false,
                }],
            },
            TurnEntry::UserTurn {
                messages: vec![AgentMessage::User {
                    text: "q".to_string(),
                }],
            },
        ];
        let reduced = compact_tool_results(&mut state, 1);
        assert_eq!(reduced, 0, "already persisted results should not be replaced");
    }

    #[test]
    fn compact_tool_results_skips_placeholder() {
        let mut state = make_state(30_000, 5_000, 1_250);
        state.user_turns_list = vec![
            TurnEntry::UserTurn {
                messages: vec![AgentMessage::ToolResult {
                    tool_call_id: "c1".into(),
                    content: TOOL_RESULT_PLACEHOLDER.to_string(),
                    is_error: false,
                }],
            },
            TurnEntry::UserTurn {
                messages: vec![AgentMessage::User {
                    text: "q".to_string(),
                }],
            },
        ];
        let reduced = compact_tool_results(&mut state, 1);
        assert_eq!(reduced, 0, "already replaced results should not be re-replaced");
    }

    #[test]
    fn determine_cascade_params_at_085() {
        let mut state = make_state(0, 0, 1000);
        state.update_api_usage(860, 0);
        let config = ContextConfig::default();
        let params = determine_cascade_params(&state, &config);
        assert!(params.should_cascade);
        assert_eq!(params.m, 3);
        assert!(!params.block_tool_calls);
    }

    #[test]
    fn determine_cascade_params_at_092() {
        let mut state = make_state(0, 0, 1000);
        state.update_api_usage(930, 0);
        let config = ContextConfig::default();
        let params = determine_cascade_params(&state, &config);
        assert!(params.should_cascade);
        assert_eq!(params.m, 2);
        assert!(!params.block_tool_calls);
    }

    #[test]
    fn layer0_persist_aggregate_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = make_state(200_000, 500_000, 125_000);
        let medium = "x".repeat(20_000);
        state.user_turns_list = vec![TurnEntry::UserTurn {
            messages: vec![
                AgentMessage::ToolResult {
                    tool_call_id: "tc_a".into(),
                    content: medium.clone(),
                    is_error: false,
                },
                AgentMessage::ToolResult {
                    tool_call_id: "tc_b".into(),
                    content: medium.clone(),
                    is_error: false,
                },
                AgentMessage::ToolResult {
                    tool_call_id: "tc_c".into(),
                    content: medium.clone(),
                    is_error: false,
                },
                AgentMessage::ToolResult {
                    tool_call_id: "tc_d".into(),
                    content: medium.clone(),
                    is_error: false,
                },
                AgentMessage::ToolResult {
                    tool_call_id: "tc_e".into(),
                    content: medium.clone(),
                    is_error: false,
                },
                AgentMessage::ToolResult {
                    tool_call_id: "tc_f".into(),
                    content: medium.clone(),
                    is_error: false,
                },
                AgentMessage::ToolResult {
                    tool_call_id: "tc_g".into(),
                    content: medium.clone(),
                    is_error: false,
                },
                AgentMessage::ToolResult {
                    tool_call_id: "tc_h".into(),
                    content: medium.clone(),
                    is_error: false,
                },
            ],
        }];
        let config = ContextConfig {
            layer0_turn_aggregate_max_chars: 150_000,
            ..Default::default()
        };
        let results = layer0_persist_large_results(&mut state, &config, dir.path(), "test_session");
        assert!(!results.is_empty(), "aggregate threshold should trigger persistence");
    }

    #[test]
    fn layer0_persist_file_readable() {
        let dir = tempfile::tempdir().unwrap();
        let original = "hello world content for persistence test ".repeat(1000);
        let mut state = make_state(original.len(), 100_000, 25_000);
        state.user_turns_list = vec![TurnEntry::UserTurn {
            messages: vec![AgentMessage::ToolResult {
                tool_call_id: "tc_read".into(),
                content: original.clone(),
                is_error: false,
            }],
        }];
        let config = ContextConfig::default();
        let results = layer0_persist_large_results(&mut state, &config, dir.path(), "sess1");
        assert_eq!(results.len(), 1);
        let content = std::fs::read_to_string(&results[0].persisted_path).unwrap();
        assert_eq!(content, original, "persisted file should contain original content");
    }

    #[test]
    fn force_drop_oldest_to_target_invalidates_usage() {
        let mut state = make_state(4000, 4000, 1000);
        state.update_api_usage(900, 0);
        state.user_turns_list = vec![
            TurnEntry::UserTurn {
                messages: vec![AgentMessage::User {
                    text: "x".repeat(3000),
                }],
            },
            TurnEntry::UserTurn {
                messages: vec![AgentMessage::User {
                    text: "y".repeat(500),
                }],
            },
        ];
        force_drop_oldest_to_target(&mut state);
        assert!(state.last_api_usage.is_none(), "usage should be invalidated after force drop");
    }

    #[test]
    fn is_context_overflow_comprehensive() {
        assert!(is_context_overflow_error("context length exceeded"));
        assert!(is_context_overflow_error("maximum context token limit"));
        assert!(is_context_overflow_error("Context limit exceeded"));
        assert!(!is_context_overflow_error("rate limit exceeded"));
        assert!(!is_context_overflow_error("authentication failed"));
    }
}
