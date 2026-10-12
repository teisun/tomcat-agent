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
///
/// `reference` 是运行时在解析本轮 `<review>` 后按出现顺序分配的 `F01` / `F02`…
/// 局部引用。它只用于主 Agent 在本轮里无歧义地申辩，绝不试图跨轮保持稳定：
/// reviewer 改写一句话就不再是同一个文本，内容哈希式「稳定 id」反而会制造假精确。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    #[serde(default)]
    pub reference: String,
    pub severity: String,
    /// P1 的证据类别。运行时只接受明确声明的 `user_defect` 或 `plan_mismatch`，
    /// 避免把自由散文中的「似乎不符」误升级成阻塞项。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<String>,
    /// `basis=plan_mismatch` 时必须填写，引用被违反的计划具体行/条目。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_ref: Option<String>,
    pub area: String,
    pub note: String,
}

impl Finding {
    pub fn new(severity: String, area: String, note: String) -> Self {
        Self {
            reference: String::new(),
            severity,
            basis: None,
            plan_ref: None,
            area,
            note,
        }
    }

    pub fn with_reference(mut self, reference: impl Into<String>) -> Self {
        self.reference = reference.into();
        self
    }

    fn with_evidence(mut self, basis: Option<String>, plan_ref: Option<String>) -> Self {
        self.basis = basis;
        self.plan_ref = plan_ref;
        self
    }

    pub fn tier(&self) -> SeverityTier {
        match self.severity.trim().to_ascii_lowercase().as_str() {
            "p0" | "blocker" | "critical" => SeverityTier::P0,
            "p1" | "major" => SeverityTier::P1,
            // `minor` / old vocabulary / malformed text must never accidentally turn
            // into a blocking finding. The reviewer prompt's explicit contract is P0/P1/P2.
            _ => SeverityTier::P2,
        }
    }

    pub fn blocks(&self) -> bool {
        matches!(self.tier(), SeverityTier::P0 | SeverityTier::P1)
    }

    fn has_valid_p1_evidence(&self, plan_text: &str) -> bool {
        match self.basis.as_deref().map(str::trim) {
            Some("user_defect") => true,
            Some("plan_mismatch") => self.plan_ref.as_deref().is_some_and(|reference| {
                let reference = reference.trim();
                !reference.is_empty() && plan_text.contains(reference)
            }),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeverityTier {
    P0,
    P1,
    P2,
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

    for (index, finding) in findings.iter_mut().enumerate() {
        finding.reference = format!("F{:02}", index + 1);
    }

    Some(ParsedReview {
        verdict,
        summary: summary?,
        changes_summary: changes_summary?,
        applied_changes: applied?,
        findings,
    })
}

/// 把 code-reviewer 无法自证的 P1 降为 P2。
///
/// plan reviewer 也复用 `<review>` 解析器，但它的 P1 判据不同，故这一步必须由
/// code-review 入口显式调用，不能混入通用解析。
pub(crate) fn downgrade_invalid_code_review_p1_findings(findings: &mut [Finding], plan_text: &str) {
    for finding in findings {
        if finding.tier() == SeverityTier::P1 && !finding.has_valid_p1_evidence(plan_text) {
            // P1 必须带可机械核验的依据。特别是计划不符，只有明确指出被违反
            // 的计划条目才可阻塞；未说明依据的自由散文一律降为 P2。
            finding.severity = "P2".to_string();
        }
    }
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
    let mut basis = None;
    let mut plan_ref = None;
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
            "basis" => basis = Some(val),
            "plan_ref" => plan_ref = Some(val),
            "area" => area = Some(val),
            "note" => note = Some(val),
            _ => {}
        }
    }
    Some(
        Finding::new(
            severity.unwrap_or_else(|| "suggestion".into()),
            area.unwrap_or_default(),
            note?,
        )
        .with_evidence(basis, plan_ref),
    )
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
