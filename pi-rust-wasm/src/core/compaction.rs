//! 上下文 Compaction 四层防护算法。
//!
//! - Layer 0: 单条 tool result 超限截断
//! - Layer 1: compactable zone 内 tool result 占位符替换（零 LLM 开销）
//! - Layer 2: LLM 循环 Compaction（结构化摘要）
//! - Layer 3: 强制删除最旧 turn 兜底

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
// Layer 0: Single tool result truncation
// ---------------------------------------------------------------------------

/// 截断后的诊断信息。
#[derive(Debug)]
pub struct TruncationInfo {
    pub original_chars: usize,
    pub truncated_chars: usize,
}

/// 向前回退到最近的 char boundary（避免在多字节 UTF-8 字符中间截断导致 panic）。
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

/// Layer 0：若 `content` 超过 `max_chars` 则就地截断。
/// 在 70%~100% 区间寻找最近换行符截断，确保 Unicode 安全。
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
// Layer 1: Tool result placeholder replacement
// ---------------------------------------------------------------------------

/// Layer 1：从 compactable zone（排除最近 `keep_recent` 个 turns）中，
/// 将最旧的 tool result 逐条替换为占位符，释放空间。返回实际减少的字符数。
pub fn compact_tool_results(state: &mut ContextState, keep_recent: usize) -> usize {
    let len = state.user_turns_list.len();
    if len <= keep_recent {
        return 0;
    }
    let compactable_end = len - keep_recent;
    let budget = state.context_budget_chars;
    let mut estimate = state.estimate_context_chars;
    let mut total_reduced = 0usize;

    for turn in state.user_turns_list[..compactable_end].iter_mut() {
        if estimate <= budget {
            break;
        }
        if let TurnEntry::UserTurn { messages } = turn {
            for msg in messages.iter_mut() {
                if let crate::core::agent_loop::AgentMessage::ToolResult { content, .. } = msg {
                    if content.len() <= TOOL_RESULT_PLACEHOLDER.len() {
                        continue;
                    }
                    let old_len = content.len();
                    let reduced = old_len - TOOL_RESULT_PLACEHOLDER.len();
                    *content = TOOL_RESULT_PLACEHOLDER.to_string();
                    estimate = estimate.saturating_sub(reduced);
                    total_reduced += reduced;
                }
            }
        }
    }
    state.estimate_context_chars = estimate;
    total_reduced
}

// ---------------------------------------------------------------------------
// Layer 2: LLM-driven compaction loop
// ---------------------------------------------------------------------------

use crate::core::llm::{ChatMessage, ChatRequest, LlmProvider};

/// Layer 2：循环调用 LLM 将最旧 turns 压缩为结构化摘要。
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
            timestamp: chrono::Utc::now()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            summary: Some(summary.clone()),
            covered_start_id: None,
            covered_end_id: None,
            covered_count: Some(batch_size),
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
                                &content[..200]
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
// Layer 3: Force drop oldest
// ---------------------------------------------------------------------------

/// Layer 3：强制删除最旧 turn 直到回预算。纯防御性兜底，几乎不可达。
pub fn force_drop_oldest(state: &mut ContextState) {
    while state.is_over_budget() && !state.user_turns_list.is_empty() {
        let removed = state.user_turns_list.remove(0);
        let chars = estimate_turn_chars(&removed);
        state.estimate_context_chars = state.estimate_context_chars.saturating_sub(chars);
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
        assert_eq!(floor_char_boundary(s, 3), 3); // end of '你'
        assert_eq!(floor_char_boundary(s, 4), 3); // mid '好', back to 3
        assert_eq!(floor_char_boundary(s, 5), 3); // mid '好', back to 3
        assert_eq!(floor_char_boundary(s, 6), 6); // end of '好'
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
        let mut s = "a\n".repeat(300_000); // 600K chars
        let info = truncate_tool_result_if_needed(&mut s, 400_000);
        assert!(info.is_some());
        let info = info.unwrap();
        assert!(info.truncated_chars < 400_000 + TRUNCATION_SUFFIX.len() + 10);
        assert!(s.ends_with(TRUNCATION_SUFFIX));
    }

    #[test]
    fn truncate_chinese_content_no_panic() {
        let mut s = "你好\n".repeat(200_000); // lots of multi-byte
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
        let mut state = ContextState {
            user_turns_list: vec![
                TurnEntry::UserTurn {
                    messages: vec![
                        AgentMessage::User {
                            text: "q".to_string(),
                        },
                        AgentMessage::ToolResult {
                            tool_call_id: "c1".into(),
                            content: "x".repeat(10_000),
                            is_error: false,
                        },
                    ],
                },
                TurnEntry::UserTurn {
                    messages: vec![AgentMessage::User {
                        text: "q2".to_string(),
                    }],
                },
            ],
            estimate_context_chars: 11_000,
            context_budget_chars: 5_000,
        };
        let reduced = compact_tool_results(&mut state, 1);
        assert!(reduced > 0);
    }

    #[test]
    fn compact_tool_results_protects_recent() {
        let tool_content = "x".repeat(10_000);
        let mut state = ContextState {
            user_turns_list: vec![TurnEntry::UserTurn {
                messages: vec![AgentMessage::ToolResult {
                    tool_call_id: "c1".into(),
                    content: tool_content.clone(),
                    is_error: false,
                }],
            }],
            estimate_context_chars: 10_000,
            context_budget_chars: 5_000,
        };
        let reduced = compact_tool_results(&mut state, 1);
        assert_eq!(reduced, 0);
    }

    #[test]
    fn force_drop_oldest_recovers_budget() {
        let mut state = ContextState {
            user_turns_list: vec![
                TurnEntry::SummaryTurn {
                    summary: "x".repeat(5000),
                },
                TurnEntry::UserTurn {
                    messages: vec![AgentMessage::User {
                        text: "q".to_string(),
                    }],
                },
            ],
            estimate_context_chars: 6000,
            context_budget_chars: 2000,
        };
        force_drop_oldest(&mut state);
        assert!(!state.is_over_budget());
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
        let mut state = ContextState {
            user_turns_list: vec![],
            estimate_context_chars: 100,
            context_budget_chars: 1000,
        };
        state.on_message_appended(500);
        assert_eq!(state.estimate_context_chars, 600);
        assert!(!state.is_over_budget());
        state.on_message_appended(500);
        assert!(state.is_over_budget());
    }

    #[test]
    fn context_state_on_new_user_turn() {
        let mut state = ContextState {
            user_turns_list: vec![],
            estimate_context_chars: 0,
            context_budget_chars: 1000,
        };
        let turn = TurnEntry::UserTurn {
            messages: vec![AgentMessage::User {
                text: "hello".to_string(),
            }],
        };
        state.on_new_user_turn(turn);
        assert_eq!(state.user_turns_list.len(), 1);
        assert_eq!(state.estimate_context_chars, 5);
    }
}
