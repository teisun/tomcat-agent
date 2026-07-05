//! Utility 模型摘要：thinking 折叠标题与会话标题。

use std::time::Duration;

use crate::core::llm::{ChatMessage, ChatRequest, LlmProvider};
use crate::infra::error::AppError;

use super::tool_summary::one_line_summary;

const UTILITY_TIMEOUT: Duration = Duration::from_secs(8);

/// 单条工具快照，供 turn 摘要 prompt 使用。
#[derive(Debug, Clone)]
pub struct ToolSnapshot {
    pub tool_name: String,
    pub summary: String,
}

impl ToolSnapshot {
    pub fn from_tool_call(tool_name: &str, args: &serde_json::Value) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            summary: one_line_summary(tool_name, args),
        }
    }
}

/// 生成 thinking 折叠块标题（≤10 词、过去时）；失败时回退规则摘要。
pub async fn generate_turn_summary(
    thinking_text: Option<&str>,
    tools: &[ToolSnapshot],
    llm: &dyn LlmProvider,
    model: &str,
) -> String {
    if tools.is_empty() && thinking_text.is_none_or(|t| t.trim().is_empty()) {
        return String::new();
    }
    let prompt = build_turn_summary_prompt(thinking_text, tools);
    match tokio::time::timeout(UTILITY_TIMEOUT, call_utility(&prompt, llm, model)).await {
        Ok(Ok(title)) if !title.trim().is_empty() => sanitize_title(title, 10),
        _ => fallback_turn_summary(tools),
    }
}

/// 生成会话标题（3–6 词）；失败返回 `Err`，上层保留规则占位。
pub async fn generate_session_title(
    first_user_text: &str,
    llm: &dyn LlmProvider,
    model: &str,
) -> Result<String, AppError> {
    let prompt = build_session_title_prompt(first_user_text);
    match tokio::time::timeout(UTILITY_TIMEOUT, call_utility(&prompt, llm, model)).await {
        Ok(Ok(title)) if !title.trim().is_empty() => Ok(sanitize_title(title, 6)),
        Ok(Ok(_)) => Err(AppError::internal("LLM returned empty session title")),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(AppError::internal("session title generation timed out")),
    }
}

/// 规则回退：按工具类型计数拼自然语言摘要。
pub fn fallback_turn_summary(tools: &[ToolSnapshot]) -> String {
    if tools.is_empty() {
        return String::new();
    }
    if tools.len() == 1 {
        let t = &tools[0];
        return match t.tool_name.as_str() {
            "read" | "read_file" | "grep" | "search_files" => format!("Read {}", t.summary),
            "write" | "write_file" | "edit" | "edit_file" | "str_replace" => {
                "Edited file".to_string()
            }
            "bash" | "shell" | "execute_command" => {
                let cmd = t.summary.strip_prefix("command=").unwrap_or(&t.summary);
                format!("Ran {cmd}")
            }
            "ask_question" => "Asked question".to_string(),
            "create_plan" => "Created plan".to_string(),
            "update_plan" => "Updated plan".to_string(),
            "todos" => "Updated todos".to_string(),
            "web_search" => "Searched web".to_string(),
            "web_fetch" => "Fetched url".to_string(),
            "search_workspace" => "Searched workspace".to_string(),
            other => format!("Used {}", other.replace('_', " ")),
        };
    }

    let read_count = tools
        .iter()
        .filter(|t| {
            matches!(
                t.tool_name.as_str(),
                "read" | "read_file" | "grep" | "search_files"
            )
        })
        .count();
    let edit_count = tools
        .iter()
        .filter(|t| {
            matches!(
                t.tool_name.as_str(),
                "write" | "write_file" | "edit" | "edit_file" | "str_replace"
            )
        })
        .count();
    let bash_count = tools
        .iter()
        .filter(|t| matches!(t.tool_name.as_str(), "bash" | "shell" | "execute_command"))
        .count();

    if read_count > 0 && edit_count == 0 && bash_count == 0 {
        return format!("Reviewed {read_count} files");
    }
    if edit_count > 0 && read_count == 0 && bash_count == 0 {
        return format!("Edited {edit_count} files");
    }
    if bash_count > 0 && read_count == 0 && edit_count == 0 {
        return if bash_count == 1 {
            fallback_turn_summary(std::slice::from_ref(&tools[0]))
        } else {
            format!("Executed {bash_count} commands")
        };
    }

    format!("Used {} tools", tools.len())
}

async fn call_utility(
    prompt: &str,
    llm: &dyn LlmProvider,
    model: &str,
) -> Result<String, AppError> {
    let req = ChatRequest {
        model: model.to_string(),
        messages: vec![ChatMessage::user(prompt)],
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
        .trim()
        .to_string();
    if text.is_empty() {
        return Err(AppError::internal("LLM returned empty summary"));
    }
    Ok(text)
}

fn build_turn_summary_prompt(thinking_text: Option<&str>, tools: &[ToolSnapshot]) -> String {
    let thinking = thinking_text.unwrap_or("").trim();
    let mut tools_block = String::new();
    for t in tools {
        tools_block.push_str(&format!("- {}: {}\n", t.tool_name, t.summary));
    }
    format!(
        "Generate a short title summarizing this AI assistant thinking block and its tool usage.\n\
         Requirements:\n\
         - At most 10 words\n\
         - Past tense\n\
         - First word must be a past tense verb (Reviewed, Updated, Created, Ran, Searched, etc.)\n\
         - For multiple file reads use \"Reviewed N files\"\n\
         - No quotes, no trailing punctuation\n\
         - Output only the title\n\n\
         Thinking:\n{thinking}\n\n\
         Tools:\n{tools_block}"
    )
}

fn build_session_title_prompt(first_user_text: &str) -> String {
    format!(
        "Generate a short chat title from the user's first message.\n\
         Requirements:\n\
         - 3-6 words\n\
         - Sentence case\n\
         - No quotes or trailing punctuation\n\
         - Output only the title\n\n\
         Message:\n{first_user_text}"
    )
}

fn sanitize_title(mut title: String, max_words: usize) -> String {
    title = title
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();
    while title.ends_with('.') || title.ends_with('!') || title.ends_with('?') {
        title.pop();
    }
    let words: Vec<&str> = title.split_whitespace().collect();
    if words.len() > max_words {
        words[..max_words].join(" ")
    } else {
        words.join(" ")
    }
}
