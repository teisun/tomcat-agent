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
///
/// 若最终标题落到无上下文的裸计数（"Used N tools"），再补一次"目的"从句，
/// 拼成用户要求的 "Used N tools for <purpose>"；补句失败才保留裸计数。
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
    let title = match tokio::time::timeout(UTILITY_TIMEOUT, call_utility(&prompt, llm, model)).await
    {
        Ok(Ok(title)) if !title.trim().is_empty() => sanitize_title(title, 10),
        _ => fallback_turn_summary(tools),
    };
    if is_bare_tool_count(&title) {
        if let Some(clause) = generate_purpose_clause(thinking_text, tools, llm, model).await {
            return format!("Used {} tools for {clause}", tools.len());
        }
    }
    title
}

/// 追加一次聚焦"目的"的短调用，返回小写名词/动名词短语（≤6 词）；失败返回 `None`。
async fn generate_purpose_clause(
    thinking_text: Option<&str>,
    tools: &[ToolSnapshot],
    llm: &dyn LlmProvider,
    model: &str,
) -> Option<String> {
    let prompt = build_purpose_clause_prompt(thinking_text, tools);
    let raw = match tokio::time::timeout(UTILITY_TIMEOUT, call_utility(&prompt, llm, model)).await {
        Ok(Ok(text)) if !text.trim().is_empty() => text,
        _ => return None,
    };
    let clause = sanitize_purpose_clause(raw);
    if clause.is_empty() {
        None
    } else {
        Some(clause)
    }
}

/// 判断标题是否是无上下文的裸计数："Used <N> tool" / "Used <N> tools"。
pub(super) fn is_bare_tool_count(title: &str) -> bool {
    let Some(rest) = title.trim().strip_prefix("Used ") else {
        return false;
    };
    let mut parts = rest.split_whitespace();
    let (Some(count), Some(noun), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !count.is_empty()
        && count.chars().all(|c| c.is_ascii_digit())
        && matches!(noun, "tool" | "tools")
}

/// 清洗"目的"从句：去引号/尾标点、剥离多余前缀、限词数。
pub(super) fn sanitize_purpose_clause(raw: String) -> String {
    let mut clause = sanitize_title(raw, 6);
    // 模型偶尔会带上 "for "/"to " 前缀或裸动词，剥掉让 "Used N tools for {clause}" 读起来自然。
    for prefix in ["for ", "For ", "to ", "To "] {
        if let Some(stripped) = clause.strip_prefix(prefix) {
            clause = stripped.to_string();
            break;
        }
    }
    clause.trim().to_string()
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

/// 生成单条 shell 命令的"目的"短句（祈使句 2–6 词、无标点）；失败回落 `Run <首个命令名>`。
///
/// 供 bash 卡片标题异步升级用（不阻塞命令执行）。
pub async fn generate_command_summary(
    command: &str,
    output_excerpt: Option<&str>,
    llm: &dyn LlmProvider,
    model: &str,
) -> String {
    let command = command.trim();
    if command.is_empty() {
        return String::new();
    }
    let prompt = build_command_summary_prompt(command, output_excerpt);
    match tokio::time::timeout(UTILITY_TIMEOUT, call_utility(&prompt, llm, model)).await {
        Ok(Ok(title)) if !title.trim().is_empty() => sanitize_title(title, 6),
        _ => fallback_command_summary(command),
    }
}

/// 命令目的的规则回退：`Run <首个命令名>`（如 `Run git`）；无法识别时 `Ran command`。
pub fn fallback_command_summary(command: &str) -> String {
    match first_command_binary(command) {
        Some(bin) => format!("Run {bin}"),
        None => "Ran command".to_string(),
    }
}

/// 提取命令里第一个"真正的可执行名"：跳过 `VAR=...` 环境赋值与 `sudo`，
/// 取第一个非空段的首 token（去掉 `./` 前缀）。
fn first_command_binary(command: &str) -> Option<String> {
    for segment in split_command_segments(command) {
        for token in segment.split_whitespace() {
            if token == "sudo" || token == "command" || token == "then" || token == "do" {
                continue;
            }
            // 形如 FOO=bar 的环境赋值（有 `=` 且左侧是合法变量名）跳过；`--flag=value` 不算。
            if let Some((lhs, _)) = token.split_once('=') {
                if is_env_var_name(lhs) {
                    continue;
                }
            }
            // 去掉 `./` 前缀与目录路径，只留 basename（与客户端 commandBinaries 一致）。
            let name = token
                .trim_start_matches("./")
                .rsplit('/')
                .next()
                .unwrap_or(token);
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// 是否是合法环境变量名（用于识别 `FOO=bar` 前缀）。
fn is_env_var_name(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !candidate.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// 把命令按 `&& || | ; 换行` 切成段（用于取每段首个可执行名）。
fn split_command_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let bytes: Vec<char> = command.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let next = bytes.get(i + 1).copied();
        let is_double = matches!((c, next), ('&', Some('&')) | ('|', Some('|')));
        if is_double {
            segments.push(std::mem::take(&mut current));
            i += 2;
            continue;
        }
        if matches!(c, '|' | ';' | '\n') {
            segments.push(std::mem::take(&mut current));
            i += 1;
            continue;
        }
        current.push(c);
        i += 1;
    }
    segments.push(current);
    segments
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
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
         - NEVER answer with only a bare tool count like \"Used 4 tools\"; always describe what was done. \
           If the tools are mixed and no single verb fits, use the form \"Used N tools for <short purpose>\".\n\
         - No quotes, no trailing punctuation\n\
         - Output only the title\n\n\
         Thinking:\n{thinking}\n\n\
         Tools:\n{tools_block}"
    )
}

/// "目的"从句 prompt：只要一个小写名词/动名词短语，用于拼 "Used N tools for <clause>"。
fn build_purpose_clause_prompt(thinking_text: Option<&str>, tools: &[ToolSnapshot]) -> String {
    let thinking = thinking_text.unwrap_or("").trim();
    let mut tools_block = String::new();
    for t in tools {
        tools_block.push_str(&format!("- {}: {}\n", t.tool_name, t.summary));
    }
    format!(
        "In 2-6 words, describe the PURPOSE of this batch of tool calls.\n\
         Requirements:\n\
         - Lowercase\n\
         - A noun or gerund phrase, e.g. \"finding coffee shops in Shenzhen\" or \"the plan preview layout\"\n\
         - Do NOT start with a verb like Used/Ran/Reviewed and do NOT include the word \"tools\"\n\
         - Do NOT start with \"for\" or \"to\"\n\
         - No quotes, no trailing punctuation\n\
         - Output only the phrase\n\n\
         Thinking:\n{thinking}\n\n\
         Tools:\n{tools_block}"
    )
}

/// bash 卡片标题 prompt：只要一句祈使目的短语（2–6 词），不复述命令本身。
fn build_command_summary_prompt(command: &str, output_excerpt: Option<&str>) -> String {
    let output = output_excerpt.unwrap_or("").trim();
    format!(
        "In 2-6 words, summarize WHY this shell command is run (its goal, not how it works).\n\
         Requirements:\n\
         - Imperative mood, e.g. \"Gather git status and recent commits\" or \"Create output directory\"\n\
         - Do NOT repeat the raw command verbatim and do NOT include flags or paths\n\
         - No quotes, no backticks, no trailing punctuation\n\
         - Output only the phrase\n\n\
         Command:\n{command}\n\n\
         Output (may be truncated):\n{output}"
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
