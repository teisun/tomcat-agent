//! Layer 1 异步预热：后台 tokio task 生成 LLM 摘要 + 写入 transcript。

use std::sync::Arc;
use std::time::Instant;

use crate::core::llm::{ChatMessage, ChatRequest, LlmProvider};
use crate::core::session::manager::{
    generate_entry_id, CompactionResult, CompactionSummary, ContextState, TurnEntry,
};
use crate::core::session::transcript::{append_entry, CompactionEntry, TranscriptEntry};
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;

use super::truncation::floor_char_boundary;

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
// maybe_start_preheat
// ---------------------------------------------------------------------------

/// 检查 ratio >= 0.50 且无正在进行的预热，启动异步预热 task。
/// 返回 true 表示成功启动；false 表示条件不满足或已有预热在进行。
pub fn maybe_start_preheat(
    state: &mut ContextState,
    llm: Arc<dyn LlmProvider>,
    config: &ContextConfig,
) -> bool {
    if state.usage_ratio() < 0.50 {
        return false;
    }
    if state.compaction_summary.is_some() {
        return false;
    }
    if state.user_turns_list.is_empty() {
        return false;
    }

    let snapshot = state.user_turns_list.clone();
    let first_id = snapshot.first().map(|t| t.id().to_string()).unwrap();
    let last_id = snapshot.last().map(|t| t.id().to_string()).unwrap();
    let covered_count = snapshot.len();
    let transcript_path = state.transcript_path.clone();
    let compaction_model = config.compaction_model.clone();

    let existing_summary = find_last_summary(&snapshot);

    let handle = tokio::spawn(async move {
        let summary_text = generate_summary(&snapshot, existing_summary.as_deref(), &*llm, &compaction_model).await?;

        let entry_id = generate_entry_id();
        let compaction_entry = TranscriptEntry::Compaction(CompactionEntry {
            id: Some(entry_id),
            parent_id: None,
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            summary: Some(summary_text.clone()),
            covered_start_id: Some(first_id.clone()),
            covered_end_id: Some(last_id.clone()),
            covered_count: Some(covered_count),
            is_boundary: Some(false),
        });

        if !transcript_path.as_os_str().is_empty() {
            let _ = append_entry(&transcript_path, &compaction_entry);
        }

        Ok(CompactionResult {
            summary_text,
            covered_start_id: first_id,
            covered_end_id: last_id,
            covered_count,
        })
    });

    state.compaction_summary = Some(CompactionSummary {
        task_handle: handle,
        covered_start_id: state.user_turns_list.first().unwrap().id().to_string(),
        covered_end_id: state.user_turns_list.last().unwrap().id().to_string(),
        started_at: Instant::now(),
    });

    true
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
