use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::prompts::{load as load_prompt, render as render_prompt, PromptKey};

use super::review::{Finding, ParsedReview};

pub const CODE_REVIEWER_ALLOWED_TOOLS: &[&str] = &["read", "search_files", "list_dir", "bash"];

pub fn code_reviewer_allowed_tools_with_policy(expose_skills: bool) -> Vec<&'static str> {
    let mut tools = CODE_REVIEWER_ALLOWED_TOOLS.to_vec();
    if expose_skills {
        tools.push("load_skill");
    }
    tools
}

pub fn code_review_system_prompt_text() -> &'static str {
    load_prompt(PromptKey::ReviewerCode)
}

pub fn build_code_review_prompt(
    plan_id: &str,
    plan_text: &str,
    plan_path: &Path,
    workspace_root: Option<&Path>,
    diff_stat: &str,
    changed_files: &[String],
) -> String {
    let plan_path = crate::infra::platform::format_home_path(plan_path);
    let workspace_hint = workspace_root
        .map(|path| {
            format!(
                "         - Project/workspace root (start repo inspection here first): `{}`\n\
                 - Access note: reads and bash still follow runtime authorization / permission rules.\n",
                crate::infra::platform::format_home_path(path)
            )
        })
        .unwrap_or_default();
    let diff_section = if diff_stat.trim().is_empty() {
        "         Runtime git diff summary: unavailable (git diff injection failed or found no tracked changes).\n".to_string()
    } else {
        format!(
            "         Runtime git diff summary (`git diff --stat HEAD`):\n\
             ```text\n{diff_stat}\n```\n"
        )
    };
    let changed_files_section = if changed_files.is_empty() {
        "         Runtime changed files list: unavailable.\n".to_string()
    } else {
        let joined = changed_files
            .iter()
            .take(80)
            .map(|path| format!("         - `{path}`"))
            .collect::<Vec<_>>()
            .join("\n");
        let suffix = if changed_files.len() > 80 {
            format!(
                "\n         - ... {} more file(s) omitted",
                changed_files.len() - 80
            )
        } else {
            String::new()
        };
        format!("         Runtime changed files list:\n{joined}{suffix}\n")
    };
    render_prompt(
        PromptKey::ReviewerCodeBrief,
        &[
            ("plan_id", plan_id),
            ("plan_path", &plan_path),
            ("workspace_hint", &workspace_hint),
            ("diff_section", &diff_section),
            ("changed_files_section", &changed_files_section),
            ("plan_text", plan_text),
        ],
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeReviewSummary {
    pub aborted: bool,
    #[serde(default)]
    pub verdict: Option<String>,
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

impl CodeReviewSummary {
    pub fn placeholder_pending() -> Self {
        Self {
            aborted: true,
            verdict: Some("aborted".into()),
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
            verdict: Some("aborted".into()),
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
            verdict: parsed.verdict,
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

    pub fn normalize_for_result(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();

        if self.aborted {
            if self.verdict.as_deref() != Some("aborted") {
                self.verdict = Some("aborted".into());
                warnings.push("code review 中止，verdict 已规范化为 aborted".into());
            }
            self.applied_changes = false;
            return warnings;
        }

        match self.verdict.clone() {
            Some(verdict)
                if matches!(verdict.as_str(), "pass" | "fail" | "partial" | "aborted") => {}
            Some(other) => {
                self.verdict = Some("aborted".into());
                warnings.push(format!(
                    "code review verdict `{other}` 非法，已规范化为 aborted"
                ));
            }
            None => {
                self.verdict = Some("partial".into());
                warnings.push("code review 未返回 verdict，已规范化为 partial".into());
            }
        }

        if self.applied_changes {
            self.applied_changes = false;
            warnings.push("code reviewer 不允许直接改动，applied_changes 已重置为 false".into());
        }
        if self.changes_summary.trim().is_empty() {
            self.changes_summary = "none".into();
            warnings.push("code review 未返回 changes_summary，已规范化为 none".into());
        }

        warnings
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "aborted": self.aborted,
            "verdict": self.verdict,
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

pub fn collect_git_diff_context(workspace_root: &std::path::Path) -> (String, Vec<String>) {
    use std::collections::BTreeSet;

    let diff_stat = run_git_capture(workspace_root, &["diff", "--stat", "--no-ext-diff", "HEAD"])
        .unwrap_or_default();

    let mut changed_files = BTreeSet::new();
    for line in run_git_lines(
        workspace_root,
        &["diff", "--name-only", "--no-ext-diff", "HEAD"],
    ) {
        if !line.is_empty() {
            changed_files.insert(line);
        }
    }
    for line in run_git_lines(
        workspace_root,
        &["ls-files", "--others", "--exclude-standard"],
    ) {
        if !line.is_empty() {
            changed_files.insert(line);
        }
    }

    (diff_stat, changed_files.into_iter().collect())
}

fn run_git_capture(workspace_root: &std::path::Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_git_lines(workspace_root: &std::path::Path, args: &[&str]) -> Vec<String> {
    run_git_capture(workspace_root, args)
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
