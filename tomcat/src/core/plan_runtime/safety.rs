//! `plan_id` 安全校验（plan §P0.5 / §8 D-? 路径穿越防御）+ 写工具路径策略守卫（B12 / 2026-05）。
//!
//! 写路径策略（plan-runtime.md §4.1 R6 / §5.6）：
//! - **PLAN**：`write/edit/hashline_edit/delete` **仅允许** `~/.tomcat/plans/*.plan.md`；
//!   离开此目录的任何写一律拒。
//! - **EXEC**：`~/.tomcat/plans/*` **全拒**（含 plan 文件正文与 frontmatter）；推进任务仅走 `update_plan`。
//! - **CHAT / Pending / Completed**：plan 文件经 plan 工具间接写；外部路径按常规权限。
//! - **Reviewer subagent**：`edit` 仅允许作用于 `~/.tomcat/plans/*.plan.md`，且 raw edit
//!   不能改 frontmatter（在 tool_exec 的 edit 分支内做 diff 检查）。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::mode::PlanMode;
use super::PlanRuntimeError;

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
    #[error(
        "PLAN 模式下 write/edit/delete 仅允许写入 ~/.tomcat/plans/*.plan.md；目标 {target:?} 不在白名单内"
    )]
    PlanModeOnlyPlanFiles { target: PathBuf },
    #[error(
        "EXEC 模式下 ~/.tomcat/plans/* 全部禁写（含正文与 frontmatter）；推进任务请使用 update_plan 工具。目标 {target:?}"
    )]
    ExecModePlanFilesReadOnly { target: PathBuf },
    #[error("reviewer 子 Agent 只能写 ~/.tomcat/plans/*.plan.md（frontmatter raw edit 仍由 edit 守卫具体检查）")]
    ReviewerOnlyPlanFiles,
    #[error("code reviewer 是严格只读子 Agent；禁止调用任何 write/edit/delete 类工具")]
    CodeReviewerReadOnly,
    #[error("无法解析 ~/.tomcat/plans/ 目录：{0}")]
    PlansDirUnavailable(String),
}

/// 当前调用方是否为 reviewer 子 Agent；用于 [`enforce_write_path_policy`] 的额外段守卫。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentKind {
    /// 主 Agent / 用户 chat / dispatch_agent leaf
    Other,
    /// reviewer subagent（`SubagentType::Reviewer`）
    Reviewer,
    /// code reviewer subagent（`ReviewKind::Code`）
    CodeReviewer,
}

/// 在 `tool_exec` 的 `write` / `edit` / `hashline_edit` / `delete` 分支首行调用。
///
/// 失败返回 `WritePathDenied`；调用方应转成 `ToolError`，给 LLM 明确提示。
///
/// 这里只做**路径维度**的拒绝；reviewer 的 frontmatter raw-edit 守卫由 edit 分支再做
/// diff 检查（因为它需要新旧两份内容，无法在路径层判断）。
pub fn enforce_write_path_policy(
    mode: &PlanMode,
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

    // Reviewer：只能写 plan 文件（且段位再由 edit guard 检查）。
    if subagent == SubagentKind::Reviewer && !is_plan_file {
        return Err(WritePathDenied::ReviewerOnlyPlanFiles);
    }

    match mode {
        PlanMode::Planning if !is_plan_file => Err(WritePathDenied::PlanModeOnlyPlanFiles {
            target: canon_target,
        }),
        PlanMode::Executing { .. } if in_plans_dir => {
            Err(WritePathDenied::ExecModePlanFilesReadOnly {
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
    #[error("reviewer 不能 raw 修改 plan 文件 frontmatter；请用 update_plan 修改结构化字段")]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 全套件共享一把 HOME 锁——`safety::tests` 修改 HOME 后还原；与 plan_runtime::tools::tests
    /// 的 home_lock 行为一致，避免污染 permission/cli config_keys 等其他 suite。
    fn home_lock() -> &'static std::sync::Mutex<()> {
        crate::test_support::home_env_lock()
    }

    fn orig_home() -> &'static Option<String> {
        static O: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        O.get_or_init(|| std::env::var("HOME").ok())
    }

    struct HomeGuard {
        path: std::path::PathBuf,
    }
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
            match orig_home() {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    fn setup_home() -> HomeGuard {
        let p = std::env::temp_dir().join(format!(
            "tomcat_safety_test_home_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(p.join(".tomcat/plans")).unwrap();
        let _ = orig_home();
        std::env::set_var("HOME", &p);
        HomeGuard { path: p }
    }

    #[test]
    fn plan_mode_rejects_writes_outside_plans_dir() {
        let _g = home_lock().lock().unwrap();
        let _home = setup_home();
        let outside = std::path::PathBuf::from("/tmp/foo.txt");
        let err = enforce_write_path_policy(&PlanMode::Planning, SubagentKind::Other, &outside)
            .expect_err("PLAN 期写 plans/ 外路径应拒");
        matches!(err, WritePathDenied::PlanModeOnlyPlanFiles { .. });
    }

    #[test]
    fn plan_mode_allows_writes_inside_plans_dir() {
        let _g = home_lock().lock().unwrap();
        let home = setup_home();
        let target = home.path.join(".tomcat/plans/foo.plan.md");
        enforce_write_path_policy(&PlanMode::Planning, SubagentKind::Other, &target)
            .expect("PLAN 期写 plans/ 内 .md 应放行");
    }

    #[test]
    fn exec_mode_rejects_writes_to_any_plans_dir_file() {
        let _g = home_lock().lock().unwrap();
        let home = setup_home();
        let target = home.path.join(".tomcat/plans/foo.plan.md");
        let err = enforce_write_path_policy(
            &PlanMode::Executing {
                plan_id: "foo".into(),
            },
            SubagentKind::Other,
            &target,
        )
        .expect_err("EXEC 期写任何 plans/ 路径都应拒");
        matches!(err, WritePathDenied::ExecModePlanFilesReadOnly { .. });
    }

    #[test]
    fn exec_mode_allows_writes_outside_plans_dir() {
        let _g = home_lock().lock().unwrap();
        let _home = setup_home();
        let outside = std::path::PathBuf::from("/tmp/foo.txt");
        enforce_write_path_policy(
            &PlanMode::Executing {
                plan_id: "foo".into(),
            },
            SubagentKind::Other,
            &outside,
        )
        .expect("EXEC 期写 plans/ 外路径应放行");
    }

    #[test]
    fn chat_mode_does_not_restrict() {
        let _g = home_lock().lock().unwrap();
        let _home = setup_home();
        let outside = std::path::PathBuf::from("/tmp/foo.txt");
        enforce_write_path_policy(&PlanMode::Chat, SubagentKind::Other, &outside)
            .expect("CHAT 期不做路径限制");
    }

    #[test]
    fn reviewer_subagent_must_target_plan_files() {
        let _g = home_lock().lock().unwrap();
        let _home = setup_home();
        let outside = std::path::PathBuf::from("/tmp/foo.txt");
        let err = enforce_write_path_policy(&PlanMode::Chat, SubagentKind::Reviewer, &outside)
            .expect_err("reviewer 不能写 plans/ 外路径");
        matches!(err, WritePathDenied::ReviewerOnlyPlanFiles);
    }

    #[test]
    fn reviewer_subagent_allows_tilde_expanded_plan_file() {
        let _g = home_lock().lock().unwrap();
        let _home = setup_home();
        let target = std::path::PathBuf::from("~/.tomcat/plans/foo.plan.md");
        enforce_write_path_policy(&PlanMode::Chat, SubagentKind::Reviewer, &target)
            .expect("reviewer 应接受 ~ 展开的 plans 路径");
    }

    #[test]
    fn code_reviewer_is_always_read_only() {
        let _g = home_lock().lock().unwrap();
        let _home = setup_home();
        let target = std::path::PathBuf::from("/tmp/foo.txt");
        let err = enforce_write_path_policy(&PlanMode::Chat, SubagentKind::CodeReviewer, &target)
            .expect_err("code reviewer 任意写路径都应拒绝");
        matches!(err, WritePathDenied::CodeReviewerReadOnly);
    }

    // ─── reviewer_body_diff_guard ───────────────────────────────────────

    const SAMPLE_PLAN: &str =
        "---\nplan_id: x\ngoal: sample\n---\n## Goal\n\ngoal\n\n## Draft\n\ndraft\n\n## Notes\n\nnote\n\n## Todos Board\n\nboard\n";

    #[test]
    fn reviewer_body_diff_guard_allows_goal_change() {
        let new = SAMPLE_PLAN.replace("## Goal\n\ngoal", "## Goal\n\nupdated goal");
        reviewer_body_diff_guard(SAMPLE_PLAN, &new).expect("改正文应放行");
    }

    #[test]
    fn reviewer_body_diff_guard_allows_todos_board_change() {
        let new = SAMPLE_PLAN.replace("board", "rewritten board");
        reviewer_body_diff_guard(SAMPLE_PLAN, &new).expect("改正文应放行");
    }

    #[test]
    fn reviewer_body_diff_guard_rejects_frontmatter_change() {
        let new = SAMPLE_PLAN.replace("plan_id: x", "plan_id: y");
        let err = reviewer_body_diff_guard(SAMPLE_PLAN, &new).expect_err("改 frontmatter 应被拒");
        matches!(err, ReviewDiffDenied::FrontmatterTouched);
    }
}
