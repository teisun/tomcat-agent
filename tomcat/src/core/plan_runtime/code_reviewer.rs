use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::prompts::{load as load_prompt, render as render_prompt, PromptKey};

use super::{
    review::{downgrade_invalid_code_review_p1_findings, Finding, ParsedReview},
    DisputedFinding,
};

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

/// 上一轮未清 finding 渲染成 prompt 片段，要求 reviewer 逐条核销。
pub(crate) fn render_open_findings_section(open_findings: &[Finding]) -> String {
    if open_findings.is_empty() {
        return String::new();
    }
    let lines = open_findings
        .iter()
        .map(|finding| {
            let evidence = match finding.basis.as_deref() {
                Some("plan_mismatch") => finding
                    .plan_ref
                    .as_deref()
                    .map(|reference| format!(" [basis=plan_mismatch; plan_ref={reference}]"))
                    .unwrap_or_else(|| " [basis=plan_mismatch]".to_string()),
                Some("user_defect") => " [basis=user_defect]".to_string(),
                Some(other) => format!(" [basis={other}]"),
                None => String::new(),
            };
            format!(
                "         - {} [{}] {}: {}{}",
                finding.reference, finding.severity, finding.area, finding.note, evidence
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "         Open findings from the previous review round (verify each one first):\n\
         {lines}\n\
         - Verify each item against the current code. Report it again ONLY if it is still unfixed.\n\
         - Do not re-report an issue you confirmed fixed.\n"
    )
}

fn render_adjudicated_section(disputed_findings: &[DisputedFinding]) -> String {
    if disputed_findings.is_empty() {
        return String::new();
    }
    let lines = disputed_findings
        .iter()
        .map(|finding| {
            format!(
                "         - [{}] {}: {} — accepted trade-off: {}",
                finding.severity, finding.area, finding.note, finding.reason
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "         Known accepted trade-offs (do NOT re-flag these, including with different wording):\n\
         {lines}\n"
    )
}

/// 构建 code-review 首轮请求的输入集合，避免 prompt 组装继续堆叠位置参数。
pub(crate) struct CodeReviewPromptInput<'a> {
    pub plan_id: &'a str,
    pub plan_text: &'a str,
    pub plan_path: &'a Path,
    pub workspace_root: Option<&'a Path>,
    pub changed_files: &'a [String],
    pub delta_files: &'a [String],
    pub round: u32,
    pub is_incremental: bool,
    pub open_findings: &'a [Finding],
    pub disputed_findings: &'a [DisputedFinding],
}

pub(crate) fn build_code_review_prompt(input: CodeReviewPromptInput<'_>) -> String {
    let CodeReviewPromptInput {
        plan_id,
        plan_text,
        plan_path,
        workspace_root,
        changed_files,
        delta_files,
        round,
        is_incremental,
        open_findings,
        disputed_findings,
    } = input;
    let plan_path = crate::infra::platform::format_home_path(plan_path);
    let workspace_hint = workspace_root
        .map(|path| {
            format!(
                "         - Project/workspace root (start repo inspection here first): `{}`\n\
                 - Path discipline: use the relative changed-file paths below. For bash, leave cwd empty or use `.`; do not guess an absolute root.\n\
                 - Access note: reads and bash still follow runtime authorization / permission rules.\n",
                crate::infra::platform::format_home_path(path)
            )
        })
        .unwrap_or_default();
    let review_scope_section = if is_incremental {
        let delta = render_changed_files(delta_files);
        format!(
            "         Incremental review (round {round}).\n\
             >>> DELTA — the ONLY files to review for NEW problems this round (changed since the previous review round; includes untracked new files):\n\
             {delta}\
             Every other changed file is frozen (already reviewed in an earlier round, unchanged since): do NOT open new issues there. Look at one ONLY if (a) an open finding below points into it, or (b) a DELTA change's impact reaches it (then flag the regression — do NOT re-review its unrelated pre-existing code).\n"
        )
    } else {
        format!(
            "         Runtime changed files list (`git diff --name-only HEAD` ∪ `git ls-files --others --exclude-standard` = tracked changes + untracked new files — the COMPLETE set to review):\n\
             {}",
            render_changed_files(changed_files)
        )
    };
    render_prompt(
        PromptKey::ReviewerCodeBrief,
        &[
            ("plan_id", plan_id),
            ("plan_path", &plan_path),
            ("workspace_hint", &workspace_hint),
            ("review_scope_section", &review_scope_section),
            (
                "open_findings_section",
                &render_open_findings_section(open_findings),
            ),
            (
                "adjudicated_findings_section",
                &render_adjudicated_section(disputed_findings),
            ),
            ("plan_text", plan_text),
        ],
    )
}

fn render_changed_files(files: &[String]) -> String {
    if files.is_empty() {
        return "         - (none)\n".to_string();
    }
    let joined = files
        .iter()
        .take(80)
        .map(|path| format!("         - `{path}`"))
        .collect::<Vec<_>>()
        .join("\n");
    let suffix = if files.len() > 80 {
        format!("\n         - ... {} more file(s) omitted", files.len() - 80)
    } else {
        String::new()
    };
    format!("{joined}{suffix}\n")
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

    pub fn from_parsed(parsed: ParsedReview, plan_text: &str) -> Self {
        let mut findings = parsed.findings;
        downgrade_invalid_code_review_p1_findings(&mut findings, plan_text);
        Self {
            aborted: false,
            verdict: parsed.verdict,
            summary: parsed.summary,
            changes_summary: parsed.changes_summary,
            applied_changes: parsed.applied_changes,
            findings,
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
        let non_blocking_findings = self
            .findings
            .iter()
            .filter(|finding| !finding.blocks())
            .count();
        serde_json::json!({
            "aborted": self.aborted,
            "verdict": self.verdict,
            "summary": self.summary,
            "changes_summary": self.changes_summary,
            "applied_changes": self.applied_changes,
            "findings": self.findings,
            "non_blocking_findings": non_blocking_findings,
            "p2_guidance": (non_blocking_findings > 0).then_some(
                "P2 findings are non-blocking suggestions; do not create a todo or delay close-out unless the user asks to address them."
            ),
            "reviewer_turns_used": self.reviewer_turns_used,
            "reviewer_turns_limit": self.reviewer_turns_limit,
            "reviewer_stop_reason": self.reviewer_stop_reason,
            "child_session_id": self.child_session_id,
        })
    }
}

/// 当前工作树相对 HEAD 的完整变更文件集：已跟踪的差异与未跟踪新文件的并集。
pub async fn collect_git_changed_files(workspace_root: &std::path::Path) -> Vec<String> {
    use std::collections::BTreeSet;

    let mut changed_files = BTreeSet::new();
    for line in run_git_lines(
        workspace_root,
        // `git -C <workspace-subdir>` normally prints paths relative to the
        // repository root. `--relative` keeps them relative to workspace_root,
        // which is the base used below for mtime and reviewer navigation.
        &["diff", "--relative", "--name-only", "--no-ext-diff", "HEAD"],
    )
    .await
    {
        if !line.is_empty() {
            changed_files.insert(line);
        }
    }
    for line in run_git_lines(
        workspace_root,
        &["ls-files", "--others", "--exclude-standard"],
    )
    .await
    {
        if !line.is_empty() {
            changed_files.insert(line);
        }
    }

    changed_files.into_iter().collect()
}

/// 从完整 changed-files 集合筛出上次 reviewer 派发后被修改的文件。
///
/// 删除文件没有可读取的 mtime，保守地保留在增量集，避免删代码的改动逃过复审。
/// mtime 只用于缩小导航范围；它晚于 `since_ms` 才入选，因此误报至多多审一次。
pub(crate) fn changed_files_since(
    workspace_root: &std::path::Path,
    changed_files: &[String],
    since_ms: u128,
) -> Vec<String> {
    changed_files
        .iter()
        .filter(|relative| {
            file_modified_ms(&workspace_root.join(relative))
                .map(|modified_ms| modified_ms > since_ms)
                .unwrap_or(true)
        })
        .cloned()
        .collect()
}

pub(crate) fn unix_timestamp_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn file_modified_ms(path: &std::path::Path) -> Option<u128> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis())
}

/// 当前 Git diff 中的代码文件和其中最新的文件修改时间。
///
/// 不读取 diff 正文、更不计算内容哈希：`git --name-only` + 文件 metadata 足以把
/// 「验收前又改过代码」和「仍是同一份代码」区分开。删除的代码文件没有 mtime，
/// 用当前时间作为保守下界，迫使后续验收在删除之后启动。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeDiffContext {
    pub changed_code_files: Vec<String>,
    pub newest_edit_mtime_ms: Option<u128>,
}

pub async fn collect_code_diff_context(workspace_root: &std::path::Path) -> CodeDiffContext {
    let changed_files = collect_git_changed_files(workspace_root).await;
    let changed_code_files: Vec<String> = changed_files
        .into_iter()
        .filter(|path| is_code_path(path))
        .collect();
    if changed_code_files.is_empty() {
        return CodeDiffContext::default();
    }

    let now_ms = unix_timestamp_ms();
    let newest_edit_mtime_ms = changed_code_files
        .iter()
        .filter_map(|relative| file_modified_ms(&workspace_root.join(relative)))
        .max()
        .or(Some(now_ms));

    CodeDiffContext {
        changed_code_files,
        newest_edit_mtime_ms,
    }
}

fn is_code_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "rs" | "ts"
                    | "tsx"
                    | "js"
                    | "jsx"
                    | "mjs"
                    | "cjs"
                    | "py"
                    | "go"
                    | "java"
                    | "kt"
                    | "kts"
                    | "c"
                    | "cc"
                    | "cpp"
                    | "cxx"
                    | "h"
                    | "hpp"
                    | "cs"
                    | "rb"
                    | "php"
                    | "swift"
                    | "scala"
                    | "sh"
                    | "sql"
                    | "vue"
                    | "svelte"
                    | "css"
                    | "scss"
                    | "less"
                    | "html"
            )
        })
}

async fn run_git_capture(workspace_root: &std::path::Path, args: &[&str]) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .args(args)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn run_git_lines(workspace_root: &std::path::Path, args: &[&str]) -> Vec<String> {
    run_git_capture(workspace_root, args)
        .await
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{collect_git_changed_files, is_code_path};

    #[test]
    fn user_visible_markup_and_styles_trigger_code_review() {
        for path in [
            "gui/src/styles.css",
            "web/app.scss",
            "web/theme.less",
            "web/index.html",
            "WEB/COMPONENT.CSS",
        ] {
            assert!(is_code_path(path), "{path} must trigger the code gate");
        }
    }

    #[test]
    fn non_code_assets_do_not_trigger_code_review() {
        for path in ["README.md", "docs/guide.txt", "assets/logo.png"] {
            assert!(!is_code_path(path), "{path} must not trigger the code gate");
        }
    }

    #[tokio::test]
    async fn changed_files_include_tracked_diff_and_untracked_files() {
        let workspace = tempfile::tempdir().expect("workspace");
        let root = workspace.path();
        std::fs::write(root.join("tracked.rs"), "fn before() {}\n").expect("seed tracked");
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .expect("git init");
        assert!(init.success());
        let add = std::process::Command::new("git")
            .args(["add", "tracked.rs"])
            .current_dir(root)
            .status()
            .expect("git add");
        assert!(add.success());
        let commit = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=tomcat-test",
                "-c",
                "user.email=tomcat-test@example.invalid",
                "commit",
                "-qm",
                "seed",
            ])
            .current_dir(root)
            .status()
            .expect("git commit");
        assert!(commit.success());

        std::fs::write(root.join("tracked.rs"), "fn after() {}\n").expect("modify tracked");
        std::fs::write(root.join("untracked.rs"), "fn fresh() {}\n").expect("add untracked");

        assert_eq!(
            collect_git_changed_files(root).await,
            vec!["tracked.rs".to_string(), "untracked.rs".to_string()]
        );
    }

    #[tokio::test]
    async fn changed_files_are_relative_to_a_nested_workspace_root() {
        let repo = tempfile::tempdir().expect("repo");
        let root = repo.path();
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).expect("nested");
        std::fs::write(nested.join("tracked.rs"), "fn before() {}\n").expect("seed");
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .expect("git init")
            .success());
        assert!(std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(root)
            .status()
            .expect("git add")
            .success());
        assert!(std::process::Command::new("git")
            .args([
                "-c",
                "user.name=tomcat-test",
                "-c",
                "user.email=tomcat-test@example.invalid",
                "commit",
                "-qm",
                "seed",
            ])
            .current_dir(root)
            .status()
            .expect("git commit")
            .success());
        std::fs::write(nested.join("tracked.rs"), "fn after() {}\n").expect("modify");

        assert_eq!(
            collect_git_changed_files(&nested).await,
            vec!["tracked.rs".to_string()]
        );
    }
}
