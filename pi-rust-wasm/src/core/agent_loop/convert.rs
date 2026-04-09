use tracing::info;

use crate::core::compaction::is_context_overflow_error;
use crate::core::llm::{ChatMessage, ChatMessageRole};
use crate::infra::error::AppError;

use super::types::{AgentMessage, LoopError, ToolCallInfo};

fn err_snippet(s: &str) -> String {
    s.chars().take(200).collect()
}

/// 将 Agent 消息列表转为 LLM 使用的 ChatMessage 序列。
pub fn convert_to_llm_format(messages: &[AgentMessage]) -> Vec<ChatMessage> {
    messages
        .iter()
        .map(|m| match m {
            AgentMessage::User { text } => ChatMessage::user(text.as_str()),
            AgentMessage::Steering { text, .. } => ChatMessage::user(text.as_str()),
            AgentMessage::CompactionSummary { summary } => ChatMessage::user(summary.as_str()),
            AgentMessage::System { text } => ChatMessage::system(text.as_str()),
            AgentMessage::Assistant { text, tool_calls } => {
                if tool_calls.is_empty() {
                    ChatMessage::assistant(text.as_str())
                } else {
                    let tc_json: Vec<serde_json::Value> = tool_calls
                        .iter()
                        .map(|tc| {
                            serde_json::json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": tc.arguments
                                }
                            })
                        })
                        .collect();
                    ChatMessage::assistant_with_tool_calls(
                        if text.is_empty() {
                            None
                        } else {
                            Some(text.as_str())
                        },
                        tc_json,
                    )
                }
            }
            AgentMessage::ToolResult {
                tool_call_id,
                content,
                ..
            } => ChatMessage::tool(tool_call_id, content),
        })
        .collect()
}

/// 从 Session 加载的 ChatMessage 转为 AgentMessage（用于 chat 拼装 initial_messages）。
pub fn agent_messages_from_chat(messages: &[ChatMessage]) -> Vec<AgentMessage> {
    messages
        .iter()
        .map(|m| match &m.role {
            ChatMessageRole::User => AgentMessage::User {
                text: m.text_content().unwrap_or("").to_string(),
            },
            ChatMessageRole::System => AgentMessage::System {
                text: m.text_content().unwrap_or("").to_string(),
            },
            ChatMessageRole::Assistant => {
                let text = m.text_content().unwrap_or("").to_string();
                let tool_calls = m
                    .tool_calls
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
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
                    .collect();
                AgentMessage::Assistant { text, tool_calls }
            }
            ChatMessageRole::Tool => AgentMessage::ToolResult {
                tool_call_id: m.tool_call_id.as_deref().unwrap_or("").to_string(),
                content: m.text_content().unwrap_or("").to_string(),
                is_error: false,
            },
        })
        .collect()
}

pub(super) fn classify_error(err: &AppError) -> LoopError {
    let s = err.to_string();
    let snippet = err_snippet(&s);
    if s.contains("401") {
        info!(
            target: "pi_wasm_chat_diag",
            phase = "classify_error",
            branch = "fatal_401",
            snippet = %snippet
        );
        return LoopError::Fatal(s);
    }
    // HTTP 400 + context_length_exceeded 等：须为 Retryable，Attempt loop 才能走 L3 截断。
    if is_context_overflow_error(&s) {
        info!(
            target: "pi_wasm_chat_diag",
            phase = "classify_error",
            branch = "retryable_context_overflow",
            snippet = %snippet
        );
        return LoopError::Retryable(s);
    }
    if s.contains("400") {
        info!(
            target: "pi_wasm_chat_diag",
            phase = "classify_error",
            branch = "fatal_400_generic",
            snippet = %snippet
        );
        return LoopError::Fatal(s);
    }
    if s.contains("429")
        || s.contains("500")
        || s.contains("502")
        || s.contains("503")
        || s.contains("请求失败")
        || s.contains("超时")
    {
        info!(
            target: "pi_wasm_chat_diag",
            phase = "classify_error",
            branch = "retryable_rate_or_server",
            snippet = %snippet
        );
        return LoopError::Retryable(s);
    }
    if s.contains("context") && (s.contains("length") || s.contains("token")) {
        info!(
            target: "pi_wasm_chat_diag",
            phase = "classify_error",
            branch = "retryable_context_heuristic",
            snippet = %snippet
        );
        return LoopError::Retryable(s);
    }
    info!(
        target: "pi_wasm_chat_diag",
        phase = "classify_error",
        branch = "fatal_default",
        snippet = %snippet
    );
    LoopError::Fatal(s)
}

/// MVP：保留首条 System（若有）+ 最近 keep_recent 条。
/// Deprecated: 由 token-aware ContextState + 四层防护替代（TASK-17）。
#[deprecated(note = "Use ContextState + compaction layers instead")]
pub fn compact_messages(messages: &mut Vec<AgentMessage>, keep_recent: usize) {
    if messages.len() <= keep_recent + 1 {
        return;
    }
    let system_take = matches!(messages.first(), Some(AgentMessage::System { .. })) as usize;
    let rest = messages.len() - system_take;
    if rest <= keep_recent {
        return;
    }
    let drop = rest - keep_recent;
    let start = system_take + drop;
    messages.drain(system_take..start);
}
