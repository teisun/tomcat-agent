//! Layer 2: LLM 一次性摘要 compactable zone（按 m 值保护最近 turns）。

use std::path::Path;

use crate::core::llm::{ChatMessage, ChatRequest, LlmProvider};
use crate::core::session::manager::{estimate_turn_chars, ContextState, TurnEntry};
use crate::core::session::transcript::{append_entry, CompactionEntry, TranscriptEntry};
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;

use super::cascade::is_context_overflow_error;
use super::truncation::floor_char_boundary;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

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
// Layer 2: LLM-driven compaction (single-shot by m value)
// ---------------------------------------------------------------------------

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
        id: format!(
            "compact_{}",
            chrono::Utc::now().timestamp_micros()
        ),
        summary: summary_text,
        timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
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

        let summary =
            match generate_or_update_summary(llm, config, &batch_text, existing_summary.as_deref())
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

        let new_turn = TurnEntry::SummaryTurn {
            id: format!(
                "compact_loop_{}",
                chrono::Utc::now().timestamp_micros()
            ),
            summary,
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        };
        state.estimate_context_chars += summary_chars;
        state.user_turns_list.insert(0, new_turn);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

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
        messages: vec![ChatMessage::system(&prompt), ChatMessage::user(batch_text)],
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
        return Err(AppError::internal(
            "compactable zone too small for PTL retry",
        ));
    }
    let retry_end = compactable_end - 1;

    let mut range_start = retry_end / 2;
    for _attempt in 0..2 {
        if range_start >= retry_end {
            break;
        }
        let sub_batch = &state.user_turns_list[range_start..retry_end];
        let prev = find_last_summary(sub_batch);
        match generate_or_update_summary(llm, config, &turns_to_text(sub_batch), prev.as_deref())
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
