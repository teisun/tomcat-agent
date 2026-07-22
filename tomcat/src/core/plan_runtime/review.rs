//! `reviewer` 共享的 `<review>` 输出格式与纯解析 helper。
//!
//! 这里**只**保留 kind 无关的格式层：finding / `<review>` 解析 / tool 白名单解析 /
//! AgentLoop 产物提取，不再承载 Plan-vs-Code 的语义分叉。计划评审与代码评审的
//! prompt、allowed tools、summary 语义、dispatcher 逻辑全部下沉到独立模块。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::agent_loop::AgentRunResult;
use crate::core::llm::ChatMessage;

/// 单条 finding（reviewer.md §5.3）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub severity: String,
    pub area: String,
    pub note: String,
}

/// `<review>` 块的中性解析结果。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedReview {
    #[serde(default)]
    pub verdict: Option<String>,
    pub summary: String,
    #[serde(default)]
    pub changes_summary: String,
    pub applied_changes: bool,
    #[serde(default)]
    pub findings: Vec<Finding>,
}

/// 严格解析 `<review>...</review>` 块。失败返回 None；多块 → 取最后一个。
pub fn parse_review_block(text: &str) -> Option<ParsedReview> {
    let last_block = find_last_review_block(text)?;
    let mut summary = None;
    let mut changes_summary = None;
    let mut applied = None;
    let mut verdict = None;
    let mut findings: Vec<Finding> = Vec::new();
    let mut in_findings = false;

    for raw_line in last_block.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("summary:") {
            in_findings = false;
            summary = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("verdict:") {
            in_findings = false;
            let normalized = rest.trim().to_ascii_lowercase();
            if !matches!(normalized.as_str(), "pass" | "fail" | "partial" | "aborted") {
                return None;
            }
            verdict = Some(normalized);
        } else if let Some(rest) = line.strip_prefix("changes_summary:") {
            in_findings = false;
            changes_summary = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("applied_changes:") {
            in_findings = false;
            applied = match rest.trim().to_ascii_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => return None,
            };
        } else if line == "findings:" || line.starts_with("findings:") {
            in_findings = true;
        } else if in_findings {
            if let Some(item) = parse_finding_line(line) {
                findings.push(item);
            }
        }
    }

    Some(ParsedReview {
        verdict,
        summary: summary?,
        changes_summary: changes_summary?,
        applied_changes: applied?,
        findings,
    })
}

/// 从 BUILTIN_TOOL_CATALOG 中筛出 `allowed` 名单内的工具，输出 OpenAI function 定义。
pub fn resolve_internal_tools(allowed: &[&str]) -> Vec<Value> {
    use crate::core::tools::contract::catalog::BUILTIN_TOOL_CATALOG;
    BUILTIN_TOOL_CATALOG
        .iter()
        .filter(|entry| allowed.contains(&entry.name))
        .map(|entry| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": entry.name,
                    "description": entry.description,
                    "parameters": (entry.parameters)(),
                }
            })
        })
        .collect()
}

/// reviewer 最终消息体——优先取 `final_text`（reasoning_loop 累计），fallback 到
/// `new_messages` 中最后一条 Assistant 文本。
pub fn extract_review_text(result: &AgentRunResult) -> String {
    if !result.final_text.trim().is_empty() {
        return result.final_text.clone();
    }
    use crate::core::llm::ChatMessageRole;
    for msg in result.new_messages.iter().rev() {
        if matches!(msg.role, ChatMessageRole::Assistant) {
            if let Some(text) = msg.text_content() {
                if !text.trim().is_empty() {
                    return text.to_string();
                }
            }
        }
    }
    String::new()
}

pub fn count_assistant_turns(messages: &[ChatMessage]) -> u32 {
    use crate::core::llm::ChatMessageRole;
    messages
        .iter()
        .filter(|m| matches!(m.role, ChatMessageRole::Assistant))
        .count() as u32
}

/// 解析 `- { severity: ..., area: "...", note: "..." }` 这种 YAML-flow 风格的行。
pub fn parse_finding_line(line: &str) -> Option<Finding> {
    let trimmed = line.trim_start_matches('-').trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let body = &trimmed[1..trimmed.len() - 1];
    let mut severity = None;
    let mut area = None;
    let mut note = None;
    for part in split_top_level_commas(body) {
        let (k, v) = part.split_once(':')?;
        let key = k.trim().trim_matches('"');
        let val = v
            .trim()
            .trim_matches(|c: char| c == '"' || c == '\'')
            .to_string();
        match key {
            "severity" => severity = Some(val),
            "area" => area = Some(val),
            "note" => note = Some(val),
            _ => {}
        }
    }
    Some(Finding {
        severity: severity.unwrap_or_else(|| "suggestion".into()),
        area: area.unwrap_or_default(),
        note: note?,
    })
}

pub fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = s.as_bytes();
    let mut in_quote: Option<u8> = None;
    for (i, &b) in bytes.iter().enumerate() {
        match in_quote {
            Some(q) if q == b => in_quote = None,
            Some(_) => {}
            None => {
                if b == b'"' || b == b'\'' {
                    in_quote = Some(b);
                } else if b == b',' {
                    out.push(&s[start..i]);
                    start = i + 1;
                }
            }
        }
    }
    out.push(&s[start..]);
    out
}

pub fn find_last_review_block(text: &str) -> Option<&str> {
    let start_tag = "<review>";
    let end_tag = "</review>";
    let mut last_start = None;
    let mut search_from = 0;
    while let Some(s) = text[search_from..].find(start_tag) {
        last_start = Some(search_from + s);
        search_from = search_from + s + start_tag.len();
    }
    let start = last_start?;
    let body_start = start + start_tag.len();
    let end_rel = text[body_start..].find(end_tag)?;
    Some(&text[body_start..body_start + end_rel])
}
