//! `plan_id` 安全校验（plan §P0.5 / §8 D-? 路径穿越防御）+ 写工具路径策略守卫（B12 / 2026-05）。
//!
//! 写路径策略（plan-runtime.md §4.1 R6 / §5.6）：
//! - **PLAN**：`write/edit/hashline_edit/delete` **仅允许** `~/.tomcat/plans/*.plan.md`；
//!   离开此目录的任何写一律拒。
//! - **执行中的计划**：`~/.tomcat/plans/*` **全拒**（含 plan 文件正文与 frontmatter）；推进任务仅走 `update_plan`。
//! - **CHAT / Pending / Completed**：plan 文件经 plan 工具间接写；外部路径按常规权限。
//! - **Reviewer subagent**：`edit` 仅允许作用于 `~/.tomcat/plans/*.plan.md`，且 raw edit
//!   不能改 frontmatter（在 tool_exec 的 edit 分支内做 diff 检查）。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::PlanRuntimeError;
use crate::core::session::AgentMode;

/// 校验 `plan_id` 仅含 `a-z 0-9 _ -` 字符，且不为空。
///
/// 失败：
/// - 空串 → `UnsafePlanId("empty")`
/// - 含 `/`、`\\`、`..`、控制字符或空白 → `UnsafePlanId("forbidden char(s)")`
/// - 含其它非 ASCII 字符 → `UnsafePlanId("non-ascii")`
///
/// 目的：`~/.tomcat/plans/<plan_id>.plan.md` 必须落在 `plans/` 子树内，**不**允许 `../etc/passwd` /
/// `subdir/secret` / 控制字符 / 大写盘符等导致路径穿越或 Windows 大小写歧义。
pub fn assert_plan_id_safe(plan_id: &str) -> Result<(), PlanRuntimeError> {
    if plan_id.is_empty() {
        return Err(PlanRuntimeError::UnsafePlanId("empty".into()));
    }
    // 显式拒：路径分隔 / 父引用 / 控制字符 / 空白
    for ch in plan_id.chars() {
        if ch == '/' || ch == '\\' || ch.is_control() || ch.is_whitespace() {
            return Err(PlanRuntimeError::UnsafePlanId(format!(
                "forbidden char {ch:?}"
            )));
        }
    }
    if plan_id.contains("..") {
        return Err(PlanRuntimeError::UnsafePlanId("contains ..".into()));
    }
    if !plan_id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err(PlanRuntimeError::UnsafePlanId(format!(
            "non-[a-z0-9_-] chars in {plan_id:?}"
        )));
    }
    Ok(())
}

/// 在 plan_id 落盘前调用的「最后一道防线」：与 `assert_plan_id_safe` 等价，但语义化命名以便
/// 在 `file_store::write_plan` 等位置形成自文档化的 grep 锚点。
pub fn assert_plan_id_safe_for_disk(plan_id: &str) -> Result<(), PlanRuntimeError> {
    assert_plan_id_safe(plan_id)
}

// ─── B12：写路径策略守卫 ────────────────────────────────────────────────────────

/// 写工具路径策略拒绝原因（[`enforce_write_path_policy`] 返回 Err 时携带）。
#[derive(Debug, thiserror::Error)]
pub enum WritePathDenied {
    #[error("In PLAN mode, write/edit/delete may only target ~/.tomcat/plans/*.plan.md; target {target:?} is not allowed")]
    PlanModeOnlyPlanFiles { target: PathBuf },
    #[error("While a plan is executing, ~/.tomcat/plans/* is read-only (body and frontmatter); use update_plan to advance tasks. Target {target:?}")]
    ExecutingPlanFilesReadOnly { target: PathBuf },
    #[error("A reviewer sub-Agent may only write ~/.tomcat/plans/*.plan.md (the edit guard also checks raw frontmatter edits)")]
    ReviewerOnlyPlanFiles,
    #[error("The code reviewer is a strictly read-only sub-Agent; write/edit/delete tools are forbidden")]
    CodeReviewerReadOnly,
    #[error("Could not resolve ~/.tomcat/plans/ directory: {0}")]
    PlansDirUnavailable(String),
}

/// 当前调用方是否为 reviewer 子 Agent；用于 [`enforce_write_path_policy`] 的额外段守卫。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentKind {
    /// 主 Agent / 用户 chat / dispatch_agent leaf
    Other,
    /// plan reviewer subagent（`SubagentType::PlanReviewer`）
    PlanReviewer,
    /// code reviewer subagent（`SubagentType::CodeReviewer`）
    CodeReviewer,
}

/// 在 `tool_exec` 的 `write` / `edit` / `hashline_edit` / `delete` 分支首行调用。
///
/// 失败返回 `WritePathDenied`；调用方应转成 `ToolError`，给 LLM 明确提示。
///
/// 这里只做**路径维度**的拒绝；reviewer 的 frontmatter raw-edit 守卫由 edit 分支再做
/// diff 检查（因为它需要新旧两份内容，无法在路径层判断）。
pub fn enforce_write_path_policy(
    mode: AgentMode,
    executing: bool,
    subagent: SubagentKind,
    target_path: &Path,
) -> Result<(), WritePathDenied> {
    let plans_dir = super::file_store::plans_dir()
        .map_err(|e| WritePathDenied::PlansDirUnavailable(e.to_string()))?;
    let canon_plans = plans_dir.canonicalize().unwrap_or(plans_dir);
    // 目标文件未必存在（如 PLAN 期 LLM `write` 新文件），canonicalize 会失败；
    // 优先 canonicalize 父目录，再拼回文件名，保证 macOS `/var/folders` →
    // `/private/var/folders` 这种 symlink 边界两侧都用统一形态比较。
    let normalized_target = target_path
        .to_str()
        .and_then(|raw| crate::infra::platform::normalize_path(raw).ok())
        .unwrap_or_else(|| target_path.to_path_buf());
    let canon_target: PathBuf = if let Ok(canon) = normalized_target.canonicalize() {
        canon
    } else if let Some(parent) = normalized_target.parent() {
        let canon_parent = parent
            .canonicalize()
            .unwrap_or_else(|_| parent.to_path_buf());
        match normalized_target.file_name() {
            Some(name) => canon_parent.join(name),
            None => normalized_target,
        }
    } else {
        normalized_target
    };

    let in_plans_dir = canon_target.starts_with(&canon_plans);
    let is_plan_file = in_plans_dir && canon_target.extension() == Some(OsStr::new("md"));

    if subagent == SubagentKind::CodeReviewer {
        return Err(WritePathDenied::CodeReviewerReadOnly);
    }

    // Plan reviewer：只能写 plan 文件（且段位再由 edit guard 检查）。
    if subagent == SubagentKind::PlanReviewer && !is_plan_file {
        return Err(WritePathDenied::ReviewerOnlyPlanFiles);
    }

    match mode {
        AgentMode::Plan if !is_plan_file => Err(WritePathDenied::PlanModeOnlyPlanFiles {
            target: canon_target,
        }),
        AgentMode::Chat if executing && in_plans_dir => {
            Err(WritePathDenied::ExecutingPlanFilesReadOnly {
                target: canon_target,
            })
        }
        _ => Ok(()),
    }
}

/// reviewer 在 plan 文件上做 `edit` 时的「frontmatter 不可 raw 改」守卫。
///
/// 在 `tool_exec` 的 `edit` 分支被调用：传入原文 + 模拟应用 edits 后的新文，
/// 返回 `Err` 表示 frontmatter 有变化。正文其余部分全部允许。
pub fn reviewer_body_diff_guard(old: &str, new: &str) -> Result<(), ReviewDiffDenied> {
    if extract_frontmatter(old) != extract_frontmatter(new) {
        return Err(ReviewDiffDenied::FrontmatterTouched);
    }
    Ok(())
}

/// reviewer 段守卫拒绝原因。
#[derive(Debug, thiserror::Error)]
pub enum ReviewDiffDenied {
    #[error("A reviewer cannot raw-edit plan frontmatter; use update_plan for structured fields")]
    FrontmatterTouched,
}

fn extract_frontmatter(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("---\n") else {
        return "";
    };
    let Some(end) = rest.find("\n---\n") else {
        return "";
    };
    &text[..end + 5]
}
