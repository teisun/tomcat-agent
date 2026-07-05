use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;

pub(crate) use crate::core::plan_runtime::file_store::{
    plan_path_for_id, read_plan, validate_frontmatter_invariants, write_plan, PlanFile,
    PlanFileFrontmatter, PlanFileState, TodoItem, TodoStatus,
};
pub(crate) use crate::core::plan_runtime::review::{ReviewKind, ReviewSummary};
pub(crate) use crate::core::plan_runtime::todo_runtime::TodosRuntime;
pub(crate) use crate::core::plan_runtime::verify::{VerifyCheck, VerifySummary};
pub(crate) use crate::core::plan_runtime::{
    state::PlanState, PlanRuntime, PlanRuntimeError, ReviewerDispatcher, VerifierDispatcher,
};
pub(crate) use crate::core::session::manager::{PlanEventKind, PlanEventRef};
pub(crate) use crate::core::tools::plan_tool::{
    ask_question, create_plan, shared_todo_ops, todos, update_plan, ToolError,
};

pub fn home_lock() -> &'static crate::test_support::TestLock {
    crate::test_support::home_env_lock()
}

fn orig_home() -> &'static Option<String> {
    static ORIG_HOME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    ORIG_HOME.get_or_init(|| std::env::var("HOME").ok())
}

pub fn setup_isolated_home() -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "tomcat_tools_test_home_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(p.join(".tomcat").join("plans")).unwrap();
    let _ = orig_home();
    std::env::set_var("HOME", &p);
    p
}

pub fn cleanup_home(p: &std::path::Path) {
    let _ = std::fs::remove_dir_all(p);
    match orig_home() {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
}

pub fn fresh_planning_plan(rt: &PlanRuntime) -> String {
    rt.set_max_code_review_rounds(0);
    rt.enter_planning().unwrap();
    let out = create_plan::execute(
        rt,
        create_plan::CreatePlanArgs {
            goal: "g".into(),
            draft: "fresh draft body".into(),
            todos: vec![
                create_plan::TodoArg {
                    id: "t1".into(),
                    content: "step 1".into(),
                    status: TodoStatus::Pending,
                },
                create_plan::TodoArg {
                    id: "t2".into(),
                    content: "step 2".into(),
                    status: TodoStatus::Pending,
                },
            ],
        },
    )
    .unwrap();
    out["plan_id"].as_str().unwrap().to_string()
}

pub fn mark_plan_executing(rt: &PlanRuntime, plan_id: &str, session_key: &str) {
    let path = plan_path_for_id(plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.state = PlanFileState::Executing;
    plan.frontmatter.session_key = Some(session_key.into());
    plan.frontmatter.session_id = Some(format!("sid-{session_key}"));
    write_plan(&path, &plan, 2000).unwrap();
    rt.set_max_code_review_rounds(0);
    rt.set_executing_for_test(plan_id.to_string());
}

pub struct MockReviewerDispatcher {
    summaries: parking_lot::Mutex<Vec<ReviewSummary>>,
    pub call_count: AtomicUsize,
    pub delay: Option<Duration>,
}

impl MockReviewerDispatcher {
    pub fn new(summaries: Vec<ReviewSummary>) -> Self {
        Self {
            summaries: parking_lot::Mutex::new(summaries),
            call_count: AtomicUsize::new(0),
            delay: None,
        }
    }
}

#[async_trait]
impl ReviewerDispatcher for MockReviewerDispatcher {
    async fn dispatch(
        &self,
        _plan_id: &str,
        _plan_text: &str,
        kind: ReviewKind,
        _allow_review_edit: bool,
    ) -> ReviewSummary {
        self.call_count.fetch_add(1, Ordering::Relaxed);
        if let Some(d) = self.delay {
            tokio::time::sleep(d).await;
        }
        let mut q = self.summaries.lock();
        if q.is_empty() {
            ReviewSummary::aborted_with_kind(kind, "mock 队列耗尽")
        } else {
            let mut summary = q.remove(0);
            summary.kind = kind;
            summary
        }
    }
}

pub fn ok_review() -> ReviewSummary {
    ReviewSummary {
        aborted: false,
        summary: "looks ok".into(),
        changes_summary: "none".into(),
        applied_changes: false,
        ..Default::default()
    }
}

pub fn pass_code_review() -> ReviewSummary {
    ReviewSummary {
        kind: ReviewKind::Code,
        aborted: false,
        verdict: Some("pass".into()),
        summary: "code review passed".into(),
        changes_summary: "none".into(),
        applied_changes: false,
        ..Default::default()
    }
}

pub fn fail_code_review() -> ReviewSummary {
    ReviewSummary {
        kind: ReviewKind::Code,
        aborted: false,
        verdict: Some("fail".into()),
        summary: "code review found a concrete issue".into(),
        changes_summary: "none".into(),
        applied_changes: false,
        ..Default::default()
    }
}

pub fn aborted_code_review(summary: &str) -> ReviewSummary {
    let mut review = ReviewSummary::aborted_with_kind(ReviewKind::Code, summary);
    review.verdict = Some("aborted".into());
    review
}

pub struct MockVerifierDispatcher {
    summaries: parking_lot::Mutex<Vec<VerifySummary>>,
    pub call_count: AtomicUsize,
}

impl MockVerifierDispatcher {
    pub fn new(summaries: Vec<VerifySummary>) -> Self {
        Self {
            summaries: parking_lot::Mutex::new(summaries),
            call_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl VerifierDispatcher for MockVerifierDispatcher {
    async fn dispatch(
        &self,
        _plan_id: &str,
        _plan_text: &str,
    ) -> VerifySummary {
        self.call_count.fetch_add(1, Ordering::Relaxed);
        let mut q = self.summaries.lock();
        if q.is_empty() {
            VerifySummary::aborted_with("mock verifier 队列耗尽")
        } else {
            q.remove(0)
        }
    }
}

pub fn ok_verify_pass() -> VerifySummary {
    VerifySummary {
        checks: vec![VerifyCheck {
            name: "unit".into(),
            command: "cargo test -p tomcat plan_runtime".into(),
            result: "pass".into(),
            output_excerpt: "1 passed".into(),
        }],
        verdict: "pass".into(),
        summary: "verification passed".into(),
        verifier_turns_limit: 64,
        verifier_stop_reason: "completed".into(),
        ..Default::default()
    }
}

pub fn fail_verify() -> VerifySummary {
    VerifySummary {
        checks: vec![VerifyCheck {
            name: "unit".into(),
            command: "cargo test -p tomcat plan_runtime".into(),
            result: "fail".into(),
            output_excerpt: "1 failed".into(),
        }],
        verdict: "fail".into(),
        summary: "unit verification failed".into(),
        verifier_turns_limit: 64,
        verifier_stop_reason: "completed".into(),
        ..Default::default()
    }
}

pub fn partial_verify() -> VerifySummary {
    VerifySummary {
        checks: vec![VerifyCheck {
            name: "unit test".into(),
            command: String::new(),
            result: "skip".into(),
            output_excerpt: "sandbox blocked".into(),
        }],
        verdict: "partial".into(),
        summary: "verification inconclusive".into(),
        verifier_turns_limit: 64,
        verifier_stop_reason: "completed".into(),
        ..Default::default()
    }
}

pub fn aborted_verify(stop_reason: &str, summary: &str) -> VerifySummary {
    VerifySummary {
        verdict: "aborted".into(),
        summary: summary.into(),
        verifier_turns_limit: 64,
        verifier_stop_reason: stop_reason.into(),
        ..Default::default()
    }
}

pub fn good_args_with_todo() -> create_plan::CreatePlanArgs {
    create_plan::CreatePlanArgs {
        goal: "g".into(),
        draft: "draft body content".into(),
        todos: vec![create_plan::TodoArg {
            id: "t1".into(),
            content: "step".into(),
            status: TodoStatus::Pending,
        }],
    }
}

pub fn write_plan_file_at(
    path: &std::path::Path,
    plan_id: &str,
    disk_state: PlanFileState,
) -> std::path::PathBuf {
    let fm = PlanFileFrontmatter {
        plan_id: plan_id.into(),
        goal: "P6 build target".into(),
        state: disk_state,
        session_key: Some("orig-session-key".into()),
        session_id: Some("orig-uuid".into()),
        created_at: "2026-05-19T00:00:00Z".into(),
        schema_version: 1,
        todos: vec![TodoItem {
            id: "step1".into(),
            content: "do the thing".into(),
            status: TodoStatus::Pending,
        }],
        unknown: Default::default(),
    };
    let plan = PlanFile {
        frontmatter: fm,
        body: "## Goal\nbuild target\n".into(),
    };
    write_plan(path, &plan, 1000).unwrap();
    path.to_path_buf()
}

pub fn write_disk_plan(plan_id: &str, disk_state: PlanFileState) -> std::path::PathBuf {
    let path = plan_path_for_id(plan_id).unwrap();
    write_plan_file_at(&path, plan_id, disk_state)
}
