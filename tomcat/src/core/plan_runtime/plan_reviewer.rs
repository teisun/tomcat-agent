use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::prompts::{load as load_prompt, render as render_prompt, PromptKey};

use super::review::{Finding, ParsedReview};

/// 生产路径 plan reviewer 改稿权固定开启；`false` 仅供 Mock 单测。
pub const REVIEWER_ALLOW_REVIEW_EDIT: bool = true;

pub const PLAN_REVIEWER_ALLOWED_TOOLS: &[&str] = &[
    "read",
    "search_files",
    "list_dir",
    "todos",
    "update_plan",
    "edit",
];

pub fn plan_reviewer_allowed_tools_with_policy(expose_skills: bool) -> Vec<&'static str> {
    let mut tools = PLAN_REVIEWER_ALLOWED_TOOLS.to_vec();
    if expose_skills {
        tools.push("load_skill");
    }
    tools
}

pub fn reviewer_system_prompt_text() -> &'static str {
    load_prompt(PromptKey::ReviewerPlan)
}

/// 构造 plan reviewer 子 Agent 的 initial user prompt。
pub fn build_review_prompt(
    plan_id: &str,
    plan_text: &str,
    plan_path: &Path,
    workspace_root: Option<&Path>,
) -> String {
    let plan_path = crate::infra::platform::format_home_path(plan_path);
    let workspace_hint = workspace_root
        .map(|path| {
            format!(
                "         - Project/workspace root (start repo inspection here first): `{}`\n\
                 - Access note: this is the startup workspace snapshot; reads may still require runtime authorization (`workspace_roots` or a session grant) before they succeed.\n",
                crate::infra::platform::format_home_path(path)
            )
        })
        .unwrap_or_default();
    render_prompt(
        PromptKey::ReviewerPlanBrief,
        &[
            ("plan_id", plan_id),
            ("plan_path", &plan_path),
            ("workspace_hint", &workspace_hint),
            ("plan_text", plan_text),
        ],
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanReviewSummary {
    pub aborted: bool,
    pub summary: String,
    #[serde(default)]
    pub changes_summary: String,
    pub applied_changes: bool,
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub reviewer_turns_used: u32,
    #[serde(default)]
    pub reviewer_turns_limit: u32,
    #[serde(default)]
    pub reviewer_stop_reason: String,
    #[serde(default)]
    pub child_session_id: String,
}

impl PlanReviewSummary {
    pub fn placeholder_pending() -> Self {
        Self {
            aborted: true,
            summary: "reviewer 子 Agent 将在 P4 接入；当前阶段返回 aborted 占位".into(),
            changes_summary: "none".into(),
            applied_changes: false,
            findings: Vec::new(),
            reviewer_turns_used: 0,
            reviewer_turns_limit: 0,
            reviewer_stop_reason: "not_dispatched".into(),
            child_session_id: String::new(),
        }
    }

    pub fn aborted_with(reason: impl Into<String>) -> Self {
        Self {
            aborted: true,
            summary: reason.into(),
            changes_summary: "none".into(),
            applied_changes: false,
            findings: Vec::new(),
            reviewer_turns_used: 0,
            reviewer_turns_limit: 0,
            reviewer_stop_reason: "aborted".into(),
            child_session_id: String::new(),
        }
    }

    pub fn from_parsed(parsed: ParsedReview) -> Self {
        Self {
            aborted: false,
            summary: parsed.summary,
            changes_summary: parsed.changes_summary,
            applied_changes: parsed.applied_changes,
            findings: parsed.findings,
            reviewer_turns_used: 0,
            reviewer_turns_limit: 0,
            reviewer_stop_reason: "completed".into(),
            child_session_id: String::new(),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "aborted": self.aborted,
            "summary": self.summary,
            "changes_summary": self.changes_summary,
            "applied_changes": self.applied_changes,
            "findings": self.findings,
            "reviewer_turns_used": self.reviewer_turns_used,
            "reviewer_turns_limit": self.reviewer_turns_limit,
            "reviewer_stop_reason": self.reviewer_stop_reason,
            "child_session_id": self.child_session_id,
        })
    }
}
