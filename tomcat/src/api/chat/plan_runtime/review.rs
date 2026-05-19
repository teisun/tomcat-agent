//! # reviewer 子 Agent 同步派发（plan-runtime.md §P4 / tools/reviewer.md）
//!
//! reviewer 通过 [`crate::core::agent_registry::AgentRegistry::spawn_subagent_internal`] 派发；
//! 默认 allowed_tools 严格收紧到 `{read, grep, find, todos}`；当 `[reviewer]
//! default_allow_edit = true` 时附加 `{update_plan, edit}`，并把 raw `edit` 范围
//! 限定在 `## Review` 段。任何模式下都 **不**含 `create_plan` / `bash` /
//! `dispatch_agent` / `checkpoint`。
//!
//! 子 Agent 必须最终消息体里 emit 一个 `<review>` block：
//!
//! ```text
//! <review>
//! summary: <≤600 chars 自由文本>
//! changes_summary: <none|none-but-noted|applied:<short>>
//! applied_changes: <true|false>
//! </review>
//! ```
//!
//! 解析失败 / 超 `max_turns` / 父 cascade abort → `ReviewSummary { aborted: true, .. }`；
//! `create_plan` 视为成功（plan 文件已落盘），仅在 ToolResult.review 中暴露 `aborted=true`。

use serde::{Deserialize, Serialize};

/// 默认 reviewer 系统 prompt（plan §P4）。runtime 在装配 reviewer `AgentLoopConfig`
/// 时把它拼到 system message 尾部；env `TOMCAT_REVIEWER_SYSTEM_PROMPT_OVERRIDE_PATH`
/// 优先级更高。
pub const REVIEWER_SYSTEM_PROMPT: &str = "\n你是 plan reviewer 子 Agent。任务：对刚刚由 create_plan 写入的 PlanFile.frontmatter\n（todos / milestones / goal）做内联评审，挑出风险与改进项。**不**做 verdict gate，\n仅给出 summary。允许工具：默认 {read, grep, find, todos}；启用 allow_review_edit\n时附加 {update_plan, edit}，且 edit 仅能动 `## Review` 段。任何模式不可访问 create_plan /\nbash / dispatch_agent / checkpoint。\n\n输出契约（**最后一条消息**必须以 <review> block 收口）：\n<review>\nsummary: <≤600 chars 自由文本，列举 1-3 个关键风险或改进>\nchanges_summary: <none | none-but-noted | applied:<short>>\napplied_changes: <true | false>\n</review>\n";

/// reviewer 摘要（ToolResult.review 与 transcript.plan.review 共用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSummary {
    /// 是否被中止（parse 失败 / max_turns / parent abort）。
    pub aborted: bool,
    /// 自由文本摘要；aborted=true 时含失败原因。
    pub summary: String,
    /// 改动语义说明（`none` / `none-but-noted` / `applied:<short>`）。
    #[serde(default)]
    pub changes_summary: String,
    /// 是否真有改动（reviewer 通过 update_plan / edit 改了文件）。
    pub applied_changes: bool,
}

impl ReviewSummary {
    /// 用于占位的"未派发"摘要（P2 PR-PLB 阶段）。
    pub fn placeholder_pending() -> Self {
        Self {
            aborted: true,
            summary: "reviewer 子 Agent 将在 P4 接入；当前阶段返回 aborted 占位".into(),
            changes_summary: "none".into(),
            applied_changes: false,
        }
    }

    pub fn aborted_with(reason: impl Into<String>) -> Self {
        Self {
            aborted: true,
            summary: reason.into(),
            changes_summary: "none".into(),
            applied_changes: false,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "aborted": self.aborted,
            "summary": self.summary,
            "changes_summary": self.changes_summary,
            "applied_changes": self.applied_changes,
        })
    }
}

/// 严格解析 `<review>...</review>` 块。失败返回 None；多块 → 取**最后一个**。
///
/// 解析约束：
/// - `summary:` 必填，截断到 600 字符
/// - `changes_summary:` 必填，常见值 `none` / `none-but-noted` / `applied:<x>`
/// - `applied_changes:` 必填，`true` / `false`（大小写不敏感）
pub fn parse_review_block(text: &str) -> Option<ReviewSummary> {
    let last_block = find_last_review_block(text)?;
    let mut summary = None;
    let mut changes_summary = None;
    let mut applied = None;
    for line in last_block.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("summary:") {
            summary = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("changes_summary:") {
            changes_summary = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("applied_changes:") {
            applied = match rest.trim().to_ascii_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => return None,
            };
        }
    }
    let mut summary = summary?;
    if summary.len() > 600 {
        summary.truncate(600);
    }
    let changes_summary = changes_summary?;
    let applied = applied?;
    Some(ReviewSummary {
        aborted: false,
        summary,
        changes_summary,
        applied_changes: applied,
    })
}

fn find_last_review_block(text: &str) -> Option<&str> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_review_block_happy_path() {
        let text = "noise\n<review>\nsummary: ok looks good\nchanges_summary: none\napplied_changes: false\n</review>\ntail";
        let r = parse_review_block(text).unwrap();
        assert!(!r.aborted);
        assert_eq!(r.summary, "ok looks good");
        assert_eq!(r.changes_summary, "none");
        assert!(!r.applied_changes);
    }

    #[test]
    fn parse_review_block_picks_last_block() {
        let text = "<review>\nsummary: old\nchanges_summary: none\napplied_changes: false\n</review>\n<review>\nsummary: new\nchanges_summary: applied:fix\napplied_changes: true\n</review>";
        let r = parse_review_block(text).unwrap();
        assert_eq!(r.summary, "new");
        assert_eq!(r.changes_summary, "applied:fix");
        assert!(r.applied_changes);
    }

    #[test]
    fn parse_review_block_missing_required_field_returns_none() {
        let text = "<review>\nsummary: only summary\n</review>";
        assert!(parse_review_block(text).is_none());
        let text = "<review>\nchanges_summary: none\napplied_changes: false\n</review>";
        assert!(parse_review_block(text).is_none());
    }

    #[test]
    fn parse_review_block_invalid_applied_changes_returns_none() {
        let text = "<review>\nsummary: x\nchanges_summary: none\napplied_changes: maybe\n</review>";
        assert!(parse_review_block(text).is_none());
    }

    #[test]
    fn parse_review_block_unclosed_returns_none() {
        let text = "<review>\nsummary: x\nchanges_summary: none\napplied_changes: false";
        assert!(parse_review_block(text).is_none());
    }

    #[test]
    fn parse_review_block_truncates_summary_to_600() {
        let body = "a".repeat(800);
        let text = format!(
            "<review>\nsummary: {body}\nchanges_summary: none\napplied_changes: false\n</review>"
        );
        let r = parse_review_block(&text).unwrap();
        assert_eq!(r.summary.len(), 600);
    }

    #[test]
    fn aborted_summary_serializes_correctly() {
        let s = ReviewSummary::aborted_with("timeout");
        let j = s.to_json();
        assert_eq!(j["aborted"], serde_json::Value::Bool(true));
        assert_eq!(j["summary"], "timeout");
    }

    #[test]
    fn reviewer_system_prompt_contains_constraints() {
        let p = REVIEWER_SYSTEM_PROMPT;
        assert!(p.contains("reviewer"));
        assert!(p.contains("<review>"));
        assert!(p.contains("applied_changes"));
        // 显式声明禁用的工具集
        assert!(p.contains("create_plan") && p.contains("bash"));
    }
}
