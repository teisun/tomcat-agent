//! `plan_id` 安全校验（plan §P0.5 / §8 D-? 路径穿越防御）+ 写工具路径策略守卫（B12 / 2026-05）。
//!
//! 写路径策略（plan-runtime.md §4.1 R6 / §5.6）：
//! - **PLAN**：`write/edit/hashline_edit/delete` **仅允许** `~/.tomcat/plans/*.plan.md`；
//!   离开此目录的任何写一律拒。
//! - **EXEC**：`~/.tomcat/plans/*` **全拒**（含 plan 文件正文与 frontmatter）；推进任务仅走 `update_plan`。
//! - **CHAT / Pending / Completed**：plan 文件经 plan 工具间接写；外部路径按常规权限。
//! - **Reviewer subagent**：`edit` 仅 `## Review` 段（在 tool_exec 的 edit 分支内做 diff 段位检查）。

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
    #[error("reviewer 子 Agent 只能写 ~/.tomcat/plans/*.plan.md（且仅 ## Review 段，由 edit 守卫具体检查）")]
    ReviewerOnlyPlanFiles,
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
}

/// 在 `tool_exec` 的 `write` / `edit` / `hashline_edit` / `delete` 分支首行调用。
///
/// 失败返回 `WritePathDenied`；调用方应转成 `ToolError`，给 LLM 明确提示。
///
/// 这里只做**路径维度**的拒绝；reviewer 的「仅 `## Review` 段」由 edit 分支再做 diff 检查（
/// 因为它需要新旧两份内容，无法在路径层判断）。
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
    let canon_target: PathBuf = if let Ok(canon) = target_path.canonicalize() {
        canon
    } else if let Some(parent) = target_path.parent() {
        let canon_parent = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
        match target_path.file_name() {
            Some(name) => canon_parent.join(name),
            None => target_path.to_path_buf(),
        }
    } else {
        target_path.to_path_buf()
    };

    let in_plans_dir = canon_target.starts_with(&canon_plans);
    let is_plan_file = in_plans_dir && canon_target.extension() == Some(OsStr::new("md"));

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

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    /// 全套件共享一把 HOME 锁——`safety::tests` 修改 HOME 后还原；与 plan_runtime::tools::tests
    /// 的 home_lock 行为一致，避免污染 permission/cli config_keys 等其他 suite。
    fn home_lock() -> &'static Mutex<()> {
        static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
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
        let _g = home_lock().lock();
        let _home = setup_home();
        let outside = std::path::PathBuf::from("/tmp/foo.txt");
        let err = enforce_write_path_policy(&PlanMode::Planning, SubagentKind::Other, &outside)
            .expect_err("PLAN 期写 plans/ 外路径应拒");
        matches!(err, WritePathDenied::PlanModeOnlyPlanFiles { .. });
    }

    #[test]
    fn plan_mode_allows_writes_inside_plans_dir() {
        let _g = home_lock().lock();
        let home = setup_home();
        let target = home.path.join(".tomcat/plans/foo.plan.md");
        enforce_write_path_policy(&PlanMode::Planning, SubagentKind::Other, &target)
            .expect("PLAN 期写 plans/ 内 .md 应放行");
    }

    #[test]
    fn exec_mode_rejects_writes_to_any_plans_dir_file() {
        let _g = home_lock().lock();
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
        let _g = home_lock().lock();
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
        let _g = home_lock().lock();
        let _home = setup_home();
        let outside = std::path::PathBuf::from("/tmp/foo.txt");
        enforce_write_path_policy(&PlanMode::Chat, SubagentKind::Other, &outside)
            .expect("CHAT 期不做路径限制");
    }

    #[test]
    fn reviewer_subagent_must_target_plan_files() {
        let _g = home_lock().lock();
        let _home = setup_home();
        let outside = std::path::PathBuf::from("/tmp/foo.txt");
        let err = enforce_write_path_policy(&PlanMode::Chat, SubagentKind::Reviewer, &outside)
            .expect_err("reviewer 不能写 plans/ 外路径");
        matches!(err, WritePathDenied::ReviewerOnlyPlanFiles);
    }
}
