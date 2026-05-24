use super::super::mode::PlanMode;
use super::super::safety::{
    enforce_write_path_policy, reviewer_body_diff_guard, ReviewDiffDenied, SubagentKind,
    WritePathDenied,
};

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
    assert!(matches!(err, WritePathDenied::PlanModeOnlyPlanFiles { .. }));
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
    assert!(matches!(err, WritePathDenied::ExecModePlanFilesReadOnly { .. }));
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
    assert!(matches!(err, WritePathDenied::ReviewerOnlyPlanFiles));
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
    assert!(matches!(err, WritePathDenied::CodeReviewerReadOnly));
}

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
    assert!(matches!(err, ReviewDiffDenied::FrontmatterTouched));
}
