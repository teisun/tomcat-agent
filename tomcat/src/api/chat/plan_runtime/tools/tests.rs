//! plan tools 单元测试（§9.3B）。
//!
//! 测试策略：
//! - **路径隔离**：`tools/*` 用 `plan_path_for_id` 走 `~/.tomcat/plans/`；测试用 `HOME` env override 隔离。
//! - **不变量验证**：mode 守卫 / 跨 session 守卫 / 单 in_progress / id 唯一。
//! - **完整 snapshot 返回**：每个工具结果都包含 `items` / `applied` / `active_in_progress`。

use std::sync::{Mutex, OnceLock};

use super::*;
use crate::api::chat::plan_runtime::{
    file_store::{PlanFileMode, TodoStatus},
    mode::PlanMode,
    PlanRuntime,
};

/// 测试用 HOME 隔离：每个测试给一个独立 tmp dir，所有 `plan_path_for_id` 落到这里。
///
/// **注意**：cargo test 默认多线程，且 plan_path_for_id 读 HOME；为防测试并发互相污染，
/// 这里用全局 `HomeMutex` 串行化所有走盘的 plan tools 测试。
fn home_lock() -> &'static Mutex<()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
}

fn setup_isolated_home() -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "tomcat_tools_test_home_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(p.join(".tomcat").join("plans")).unwrap();
    std::env::set_var("HOME", &p);
    p
}

fn cleanup_home(p: &std::path::Path) {
    let _ = std::fs::remove_dir_all(p);
}

// ─── create_plan ────────────────────────────────────────────────────────────

#[test]
fn create_plan_invisible_outside_planning_returns_error() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    // Chat 模式
    let args = create_plan::CreatePlanArgs {
        plan_id: "p1".into(),
        goal: "g".into(),
        body: None,
        milestones: vec![],
        todos: vec![],
    };
    let err = create_plan::execute(&rt, args).expect_err("CHAT 模式应被拒");
    match err {
        ToolError::InvisibleInMode { tool, mode } => {
            assert_eq!(tool, "create_plan");
            assert_eq!(mode, "chat");
        }
        other => panic!("expected InvisibleInMode, got {other:?}"),
    }
    cleanup_home(&home);
}

#[test]
fn create_plan_in_planning_writes_disk_and_records_active_id() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.enter_planning("test obj").unwrap();
    let args = create_plan::CreatePlanArgs {
        plan_id: "demo_plan".into(),
        goal: "为 chat 补齐 plan 闭环".into(),
        body: None,
        milestones: vec![],
        todos: vec![create_plan::TodoArg {
            id: "t1".into(),
            content: "first".into(),
            status: TodoStatus::Pending,
            milestone_id: None,
        }],
    };
    let out = create_plan::execute(&rt, args).expect("create_plan OK");
    assert_eq!(out["plan_id"], "demo_plan");
    assert_eq!(out["mode"], "planning");
    assert_eq!(out["review"]["aborted"], serde_json::Value::Bool(true));

    assert_eq!(rt.active_planning_plan_id().as_deref(), Some("demo_plan"));
    let path = home.join(".tomcat").join("plans").join("demo_plan.plan.md");
    assert!(path.exists(), "{path:?} 应该已写盘");
    cleanup_home(&home);
}

#[test]
fn create_plan_rejects_empty_goal() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.enter_planning("obj").unwrap();
    let args = create_plan::CreatePlanArgs {
        plan_id: "p1".into(),
        goal: "".into(),
        body: None,
        milestones: vec![],
        todos: vec![],
    };
    let err = create_plan::execute(&rt, args).expect_err("空 goal 应被拒");
    matches!(err, ToolError::BadArgs(_));
    cleanup_home(&home);
}

#[test]
fn create_plan_rejects_unsafe_plan_id() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.enter_planning("obj").unwrap();
    let args = create_plan::CreatePlanArgs {
        plan_id: "../etc/passwd".into(),
        goal: "g".into(),
        body: None,
        milestones: vec![],
        todos: vec![],
    };
    let err = create_plan::execute(&rt, args).expect_err("非法 plan_id 应被拒");
    matches!(err, ToolError::BadArgs(_));
    cleanup_home(&home);
}

// ─── update_plan ────────────────────────────────────────────────────────────

fn fresh_planning_plan(rt: &PlanRuntime, plan_id: &str) {
    rt.enter_planning("obj").unwrap();
    create_plan::execute(
        rt,
        create_plan::CreatePlanArgs {
            plan_id: plan_id.into(),
            goal: "g".into(),
            body: None,
            milestones: vec![],
            todos: vec![
                create_plan::TodoArg {
                    id: "t1".into(),
                    content: "step 1".into(),
                    status: TodoStatus::Pending,
                    milestone_id: None,
                },
                create_plan::TodoArg {
                    id: "t2".into(),
                    content: "step 2".into(),
                    status: TodoStatus::Pending,
                    milestone_id: None,
                },
            ],
        },
    )
    .unwrap();
}

#[test]
fn update_plan_set_status_returns_full_items_snapshot() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    fresh_planning_plan(&rt, "demo_plan");
    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some("demo_plan".into()),
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                status: TodoStatus::InProgress,
            }],
            milestones_ops: vec![],
        },
    )
    .unwrap();
    assert_eq!(out["plan_id"], "demo_plan");
    let items = out["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["status"], "in_progress");
    assert_eq!(items[1]["status"], "pending");
    cleanup_home(&home);
}

#[test]
fn update_plan_reuses_todos_op_engine_single_in_progress_violation() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    fresh_planning_plan(&rt, "demo_plan");
    let err = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some("demo_plan".into()),
            ops: vec![
                update_plan::UpdateOp::SetStatus {
                    id: "t1".into(),
                    status: TodoStatus::InProgress,
                },
                update_plan::UpdateOp::SetStatus {
                    id: "t2".into(),
                    status: TodoStatus::InProgress,
                },
            ],
            milestones_ops: vec![],
        },
    )
    .expect_err("两个 in_progress 应被 ops 引擎拒");
    matches!(err, ToolError::Op(_));
    cleanup_home(&home);
}

#[test]
fn update_plan_cross_session_allowed_for_planning_pending() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    // session-a 创建 plan（planning）
    let rt_a = PlanRuntime::new("session-a");
    fresh_planning_plan(&rt_a, "shared_plan");
    // session-b 不同 session，对同一 plan（planning）做 update_plan：允许
    let rt_b = PlanRuntime::new("session-b");
    rt_b.enter_planning("b obj").unwrap();
    let out = update_plan::execute(
        &rt_b,
        update_plan::UpdatePlanArgs {
            plan_id: Some("shared_plan".into()),
            ops: vec![update_plan::UpdateOp::SetContent {
                id: "t1".into(),
                content: "edited by b".into(),
            }],
            milestones_ops: vec![],
        },
    )
    .unwrap();
    let items = out["items"].as_array().unwrap();
    assert_eq!(items[0]["content"], "edited by b");
    cleanup_home(&home);
}

#[test]
fn update_plan_cross_session_rejected_for_executing() {
    use crate::api::chat::plan_runtime::file_store::{
        plan_path_for_id, read_plan, write_plan, PlanFileMode,
    };
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt_a = PlanRuntime::new("session-a");
    fresh_planning_plan(&rt_a, "shared_plan");
    // 模拟 session-a /plan build：手动写 frontmatter 为 executing + session_key
    let path = plan_path_for_id("shared_plan").unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.mode = PlanFileMode::Executing;
    plan.frontmatter.session_key = Some("session-a".into());
    plan.frontmatter.session_id = Some("sid-a".into());
    write_plan(&path, &plan, 2000).unwrap();

    let rt_b = PlanRuntime::new("session-b");
    let err = update_plan::execute(
        &rt_b,
        update_plan::UpdatePlanArgs {
            plan_id: Some("shared_plan".into()),
            ops: vec![update_plan::UpdateOp::SetContent {
                id: "t1".into(),
                content: "intruder".into(),
            }],
            milestones_ops: vec![],
        },
    )
    .expect_err("session-b 不应能写入 session-a 的 executing plan");
    matches!(err, ToolError::CrossSessionDenied(_));
    cleanup_home(&home);
}

#[test]
fn update_plan_in_exec_promotes_completed() {
    use crate::api::chat::plan_runtime::file_store::{
        plan_path_for_id, read_plan, write_plan, PlanFileMode,
    };
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    fresh_planning_plan(&rt, "shared_plan");
    // 手动 build：frontmatter executing + session_key 写当前 session
    let path = plan_path_for_id("shared_plan").unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.mode = PlanFileMode::Executing;
    plan.frontmatter.session_key = Some("session-a".into());
    plan.frontmatter.session_id = Some("sid-a".into());
    write_plan(&path, &plan, 2000).unwrap();
    rt.set_executing_for_test("shared_plan".into());

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some("shared_plan".into()),
            ops: vec![
                update_plan::UpdateOp::SetStatus {
                    id: "t1".into(),
                    status: TodoStatus::Completed,
                },
                update_plan::UpdateOp::SetStatus {
                    id: "t2".into(),
                    status: TodoStatus::Completed,
                },
            ],
            milestones_ops: vec![],
        },
    )
    .unwrap();
    assert_eq!(out["plan_mode_before"], "executing");
    assert_eq!(out["plan_mode_after"], "completed");

    // 内存切到 Completed
    match rt.mode() {
        PlanMode::Completed { plan_id } => assert_eq!(plan_id, "shared_plan"),
        other => panic!("expected Completed, got {other:?}"),
    }
    cleanup_home(&home);
}

// ─── todos ──────────────────────────────────────────────────────────────────

#[test]
fn todos_in_chat_writes_session_scratchpad_returns_full_snapshot() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let out = todos::execute(
        &rt,
        todos::TodosArgs {
            ops: vec![
                todos::TodoOpArg::AddTodo {
                    id: "x1".into(),
                    content: "scratchpad 1".into(),
                    status: TodoStatus::Pending,
                    milestone_id: None,
                },
                todos::TodoOpArg::AddTodo {
                    id: "x2".into(),
                    content: "scratchpad 2".into(),
                    status: TodoStatus::Pending,
                    milestone_id: None,
                },
                todos::TodoOpArg::SetStatus {
                    id: "x1".into(),
                    status: TodoStatus::InProgress,
                },
            ],
        },
    )
    .unwrap();
    assert_eq!(out["scope"], "session");
    assert_eq!(out["mode"], "chat");
    assert_eq!(out["active_in_progress"], "x1");
    let items = out["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    // session_todos 内存留存
    assert_eq!(rt.snapshot_session_todos().len(), 2);
    cleanup_home(&home);
}

#[test]
fn todos_never_writes_plan_file_in_chat() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    todos::execute(
        &rt,
        todos::TodosArgs {
            ops: vec![todos::TodoOpArg::AddTodo {
                id: "x1".into(),
                content: "should not touch plan".into(),
                status: TodoStatus::Pending,
                milestone_id: None,
            }],
        },
    )
    .unwrap();
    // CHAT 下 plan dir 内不应出现任何 plan 文件
    let plans_dir = home.join(".tomcat").join("plans");
    let entries: Vec<_> = std::fs::read_dir(&plans_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|s| s.ends_with(".plan.md"))
        .collect();
    assert!(entries.is_empty(), "CHAT 下 todos 不应写 plan，发现：{entries:?}");
    cleanup_home(&home);
}

#[test]
fn todos_state_enforces_single_in_progress() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    todos::execute(
        &rt,
        todos::TodosArgs {
            ops: vec![
                todos::TodoOpArg::AddTodo {
                    id: "x1".into(),
                    content: "1".into(),
                    status: TodoStatus::InProgress,
                    milestone_id: None,
                },
                todos::TodoOpArg::AddTodo {
                    id: "x2".into(),
                    content: "2".into(),
                    status: TodoStatus::Pending,
                    milestone_id: None,
                },
            ],
        },
    )
    .unwrap();
    let err = todos::execute(
        &rt,
        todos::TodosArgs {
            ops: vec![todos::TodoOpArg::SetStatus {
                id: "x2".into(),
                status: TodoStatus::InProgress,
            }],
        },
    )
    .expect_err("第二个 in_progress 应被 ops 引擎拒");
    matches!(err, ToolError::Op(_));
    cleanup_home(&home);
}

#[test]
fn todos_in_exec_writes_plan_file() {
    use crate::api::chat::plan_runtime::file_store::{
        plan_path_for_id, read_plan, write_plan,
    };
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    fresh_planning_plan(&rt, "shared_plan");
    // 手动 build：mode=executing + session_key
    let path = plan_path_for_id("shared_plan").unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.mode = PlanFileMode::Executing;
    plan.frontmatter.session_key = Some("session-a".into());
    plan.frontmatter.session_id = Some("sid-a".into());
    write_plan(&path, &plan, 2000).unwrap();
    rt.set_executing_for_test("shared_plan".into());

    let out = todos::execute(
        &rt,
        todos::TodosArgs {
            ops: vec![todos::TodoOpArg::SetStatus {
                id: "t1".into(),
                status: TodoStatus::InProgress,
            }],
        },
    )
    .unwrap();
    assert_eq!(out["scope"], "plan");
    assert_eq!(out["plan_id"], "shared_plan");
    // 回读盘验证持久化
    let parsed = read_plan(&path).unwrap();
    assert_eq!(parsed.frontmatter.todos[0].status, TodoStatus::InProgress);
    cleanup_home(&home);
}

// ─── reviewer 集成（§9.3D P4 余量） ─────────────────────────────────────────

use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Duration;

use async_trait::async_trait;

use crate::api::chat::plan_runtime::review::ReviewSummary;
use crate::api::chat::plan_runtime::ReviewerDispatcher;

/// MockReviewer：按预编排队列返回 ReviewSummary，并记录 dispatcher 被调用时刻。
struct MockReviewerDispatcher {
    summaries: parking_lot::Mutex<Vec<ReviewSummary>>,
    call_count: AtomicUsize,
    /// 测试用：dispatcher 内部 sleep；用于验证 write_plan 已释放 lock（D1）。
    delay: Option<Duration>,
}

impl MockReviewerDispatcher {
    fn new(summaries: Vec<ReviewSummary>) -> Self {
        Self {
            summaries: parking_lot::Mutex::new(summaries),
            call_count: AtomicUsize::new(0),
            delay: None,
        }
    }

    fn with_delay(mut self, d: Duration) -> Self {
        self.delay = Some(d);
        self
    }
}

#[async_trait]
impl ReviewerDispatcher for MockReviewerDispatcher {
    async fn dispatch(
        &self,
        _plan_id: &str,
        _plan_text: &str,
        _allow_review_edit: bool,
        _abort_signal: std::sync::Arc<AtomicBool>,
    ) -> ReviewSummary {
        self.call_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Some(d) = self.delay {
            tokio::time::sleep(d).await;
        }
        let mut q = self.summaries.lock();
        if q.is_empty() {
            ReviewSummary::aborted_with("mock 队列耗尽")
        } else {
            q.remove(0)
        }
    }
}

fn ok_review() -> ReviewSummary {
    ReviewSummary {
        aborted: false,
        summary: "looks ok".into(),
        changes_summary: "none".into(),
        applied_changes: false,
    }
}

#[tokio::test]
async fn create_plan_internally_dispatches_reviewer_with_real_summary() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.attach_reviewer(std::sync::Arc::new(MockReviewerDispatcher::new(vec![ok_review()])));
    rt.enter_planning("obj").unwrap();
    let args = create_plan::CreatePlanArgs {
        plan_id: "rv_demo".into(),
        goal: "g".into(),
        body: None,
        milestones: vec![],
        todos: vec![],
    };
    let out = create_plan::execute_with_reviewer(&rt, args, false)
        .await
        .unwrap();
    assert_eq!(out["plan_id"], "rv_demo");
    assert_eq!(out["review"]["aborted"], serde_json::Value::Bool(false));
    assert_eq!(out["review"]["summary"], "looks ok");
    cleanup_home(&home);
}

#[tokio::test]
async fn create_plan_succeeds_even_when_reviewer_aborts() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.attach_reviewer(std::sync::Arc::new(MockReviewerDispatcher::new(vec![
        ReviewSummary::aborted_with("simulated parse error"),
    ])));
    rt.enter_planning("obj").unwrap();
    let out = create_plan::execute_with_reviewer(
        &rt,
        create_plan::CreatePlanArgs {
            plan_id: "rv_abort".into(),
            goal: "g".into(),
            body: None,
            milestones: vec![],
            todos: vec![],
        },
        false,
    )
    .await
    .unwrap();
    assert_eq!(out["plan_id"], "rv_abort");
    assert_eq!(out["review"]["aborted"], serde_json::Value::Bool(true));
    assert!(out["review"]["summary"]
        .as_str()
        .unwrap()
        .contains("parse error"));
    // plan 文件仍在 disk
    let plan_path = home.join(".tomcat").join("plans").join("rv_abort.plan.md");
    assert!(plan_path.exists());
    cleanup_home(&home);
}

#[tokio::test]
async fn create_plan_without_reviewer_returns_placeholder() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.enter_planning("obj").unwrap();
    let out = create_plan::execute_with_reviewer(
        &rt,
        create_plan::CreatePlanArgs {
            plan_id: "no_rv".into(),
            goal: "g".into(),
            body: None,
            milestones: vec![],
            todos: vec![],
        },
        false,
    )
    .await
    .unwrap();
    assert_eq!(out["review"]["aborted"], serde_json::Value::Bool(true));
    assert!(out["review"]["summary"]
        .as_str()
        .unwrap()
        .contains("P4 接入"));
    cleanup_home(&home);
}

#[tokio::test]
async fn dispatch_reviewer_releases_plan_lock_before_spawn() {
    // RV14 防 D1：write_plan 必须先释放 lock，dispatch 才能正常 await
    // （否则 dispatcher 内若再次试图持锁就会死锁/超时）。
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    // dispatcher 在被调用时**也**尝试抢同一个 plan 的 advisory lock：
    // 如果 create_plan 未释放，则 dispatcher 会 LockBusy；释放则瞬时成功。
    struct LockAcquiringMock;
    #[async_trait]
    impl ReviewerDispatcher for LockAcquiringMock {
        async fn dispatch(
            &self,
            plan_id: &str,
            _plan_text: &str,
            _allow_review_edit: bool,
            _abort: std::sync::Arc<AtomicBool>,
        ) -> ReviewSummary {
            use crate::api::chat::plan_runtime::file_store::{
                plan_path_for_id, with_advisory_lock,
            };
            let path = plan_path_for_id(plan_id).unwrap();
            let lock_path = path.with_file_name(format!(
                "{}.lock",
                path.file_name().unwrap().to_string_lossy()
            ));
            let r = with_advisory_lock(&lock_path, 150, || Ok::<_, _>(()));
            match r {
                Ok(()) => ReviewSummary {
                    aborted: false,
                    summary: "lock acquired by reviewer (write_plan 已释放)".into(),
                    changes_summary: "none".into(),
                    applied_changes: false,
                },
                Err(e) => ReviewSummary::aborted_with(format!("LockBusy: {e}")),
            }
        }
    }
    rt.attach_reviewer(std::sync::Arc::new(LockAcquiringMock));
    rt.enter_planning("obj").unwrap();
    let out = create_plan::execute_with_reviewer(
        &rt,
        create_plan::CreatePlanArgs {
            plan_id: "rv_lock".into(),
            goal: "g".into(),
            body: None,
            milestones: vec![],
            todos: vec![],
        },
        false,
    )
    .await
    .unwrap();
    let aborted = out["review"]["aborted"].as_bool().unwrap();
    assert!(
        !aborted,
        "dispatch_reviewer 应能拿到 lock（说明 write_plan 已释放），实际：{:?}",
        out["review"]
    );
    cleanup_home(&home);
}

#[tokio::test]
async fn reviewer_round_count_warns_after_threshold() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.attach_reviewer(std::sync::Arc::new(MockReviewerDispatcher::new(vec![
        ok_review(),
        ok_review(),
    ])));
    rt.enter_planning("obj").unwrap();
    // 第一轮
    let out1 = create_plan::execute_with_reviewer(
        &rt,
        create_plan::CreatePlanArgs {
            plan_id: "rv_round".into(),
            goal: "g".into(),
            body: None,
            milestones: vec![],
            todos: vec![],
        },
        false,
    )
    .await
    .unwrap();
    assert!(!out1["review"]["summary"]
        .as_str()
        .unwrap()
        .starts_with("[round"));
    assert_eq!(rt.reviewer_rounds("rv_round"), 1);

    // 第二轮：summary 应带 [round 2] 前缀（warning hint）
    let out2 = create_plan::execute_with_reviewer(
        &rt,
        create_plan::CreatePlanArgs {
            plan_id: "rv_round".into(),
            goal: "g".into(),
            body: None,
            milestones: vec![],
            todos: vec![],
        },
        false,
    )
    .await
    .unwrap();
    assert!(out2["review"]["summary"]
        .as_str()
        .unwrap()
        .starts_with("[round 2]"));
    assert_eq!(rt.reviewer_rounds("rv_round"), 2);
    cleanup_home(&home);
}

#[test]
fn from_json_helpers_reject_bad_args() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let err = create_plan::CreatePlanArgs::from_json(&serde_json::json!({"plan_id": 1}))
        .expect_err("数字 plan_id 应被拒");
    matches!(err, ToolError::BadArgs(_));
    let err = update_plan::UpdatePlanArgs::from_json(&serde_json::json!({"ops": "not_array"}))
        .expect_err("ops 必须是数组");
    matches!(err, ToolError::BadArgs(_));
    let err = todos::TodosArgs::from_json(&serde_json::json!({}))
        .expect_err("缺 ops 字段应被拒");
    matches!(err, ToolError::BadArgs(_));
    cleanup_home(&home);
}

// ─── P6 /plan build 五件事单测（§9.3 A build 行 + plan_build_*） ────────────

use crate::api::chat::plan_runtime::PlanRuntimeError;

/// 构造一个 planning 模式的 PlanFile 写盘，但**保持** PlanRuntime 当前为 Chat
/// （模拟 "其他 session 留下的 planning plan"，本 session 用 build 续跑）。
fn write_disk_plan(plan_id: &str, disk_mode: PlanFileMode) -> std::path::PathBuf {
    use crate::api::chat::plan_runtime::file_store::*;
    let path = plan_path_for_id(plan_id).unwrap();
    let fm = PlanFileFrontmatter {
        plan_id: plan_id.into(),
        goal: "P6 build target".into(),
        mode: disk_mode,
        session_key: Some("orig-session-key".into()),
        session_id: Some("orig-uuid".into()),
        created_at: "2026-05-19T00:00:00Z".into(),
        schema_version: 1,
        milestones: vec![],
        todos: vec![TodoItem {
            id: "step1".into(),
            content: "do the thing".into(),
            status: TodoStatus::Pending,
            milestone_id: None,
        }],
        unknown: Default::default(),
    };
    let plan = PlanFile {
        frontmatter: fm,
        body: "## Goal\nbuild target\n".into(),
    };
    write_plan(&path, &plan, 1000).unwrap();
    path
}

#[test]
fn plan_build_requires_no_active_plan_or_todos() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("blockee", PlanFileMode::Planning);

    // 1) Executing → 拒
    rt.set_executing_for_test("other_plan".into());
    let err = rt.build_plan("blockee", None).unwrap_err();
    matches!(err, PlanRuntimeError::BuildBlocked(_));

    // 重置 + 给 active todos
    let rt = PlanRuntime::new("session-a");
    rt.replace_session_todos(vec![crate::api::chat::plan_runtime::file_store::TodoItem {
        id: "live".into(),
        content: "x".into(),
        status: TodoStatus::Pending,
        milestone_id: None,
    }]);
    let err = rt.build_plan("blockee", None).unwrap_err();
    match err {
        PlanRuntimeError::BuildBlocked(s) => assert!(s.contains("未完成 todos"), "{s}"),
        other => panic!("expected BuildBlocked, got {other:?}"),
    }
    cleanup_home(&home);
}

#[test]
fn plan_build_rejects_completed_plan() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("done", PlanFileMode::Completed);
    let err = rt.build_plan("done", None).unwrap_err();
    matches!(err, PlanRuntimeError::BuildBlocked(_));
    cleanup_home(&home);
}

#[test]
fn plan_build_rejects_disk_executing() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("racy", PlanFileMode::Executing);
    let err = rt.build_plan("racy", None).unwrap_err();
    match err {
        PlanRuntimeError::BuildBlocked(s) => assert!(s.contains("executing"), "{s}"),
        other => panic!("expected BuildBlocked, got {other:?}"),
    }
    cleanup_home(&home);
}

#[test]
fn plan_build_rejects_nonexistent_plan_id() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let err = rt.build_plan("missing_plan", None).unwrap_err();
    match err {
        PlanRuntimeError::BuildPlanNotFound { plan_id, hint } => {
            assert_eq!(plan_id, "missing_plan");
            assert!(hint.contains("create_plan"), "hint 应引导 create_plan：{hint}");
        }
        other => panic!("expected BuildPlanNotFound, got {other:?}"),
    }
    cleanup_home(&home);
}

#[test]
fn plan_build_rejects_unsafe_plan_id() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let err = rt.build_plan("../etc/passwd", None).unwrap_err();
    matches!(err, PlanRuntimeError::UnsafePlanId(_));
    cleanup_home(&home);
}

#[test]
fn plan_build_swaps_session_reminder_prefix_meta_catalog() {
    // 五件事一次性生效（disk session_key/id + disk mode=executing + 内存 mode + first_exec_turn flag）。
    // reminder/prefix/catalog swap 是 mode 派生 → 通过 PlanMode::Executing 间接证明。
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("new-session-key");
    write_disk_plan("five_things", PlanFileMode::Planning);
    let outcome = rt
        .build_plan("five_things", Some("new-session-uuid".into()))
        .expect("build 成功");

    // 内存 mode = Executing
    match rt.mode() {
        PlanMode::Executing { plan_id } => assert_eq!(plan_id, "five_things"),
        other => panic!("expected Executing, got {other:?}"),
    }
    // active_planning_plan_id 已清空
    assert!(rt.active_planning_plan_id().is_none());
    // 首轮注入 flag = true
    assert!(rt.first_exec_turn_pending_for_test());

    // 磁盘 frontmatter: mode=executing + session 改写
    use crate::api::chat::plan_runtime::file_store::*;
    let plan = read_plan(&plan_path_for_id("five_things").unwrap()).unwrap();
    assert!(matches!(plan.frontmatter.mode, PlanFileMode::Executing));
    assert_eq!(plan.frontmatter.session_key.as_deref(), Some("new-session-key"));
    assert_eq!(plan.frontmatter.session_id.as_deref(), Some("new-session-uuid"));

    // 上一磁盘模式正确报告
    assert!(matches!(outcome.prev_disk_mode, PlanFileMode::Planning));
    // planning → executing 不产生 session 覆盖 warning
    assert!(outcome.warnings.is_empty());
    cleanup_home(&home);
}

#[test]
fn pending_plan_resumable_via_build() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("orig-session-key");
    write_disk_plan("resumable", PlanFileMode::Pending);
    let outcome = rt.build_plan("resumable", None).expect("续跑 build 成功");
    assert!(matches!(outcome.prev_disk_mode, PlanFileMode::Pending));
    // 同 session_key 续跑不应 warning
    assert!(outcome.warnings.is_empty(), "同 key 续跑无 warning：{:?}", outcome.warnings);
    match rt.mode() {
        PlanMode::Executing { plan_id } => assert_eq!(plan_id, "resumable"),
        other => panic!("expected Executing, got {other:?}"),
    }
    cleanup_home(&home);
}

#[test]
fn pending_plan_session_override_warns() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("brand-new-session"); // 与 disk 中 orig-session-key 不同
    write_disk_plan("crossover", PlanFileMode::Pending);
    let outcome = rt.build_plan("crossover", None).expect("续跑 build 成功");
    assert!(matches!(outcome.prev_disk_mode, PlanFileMode::Pending));
    assert_eq!(
        outcome.warnings.len(),
        1,
        "异 session_key 续跑应有 1 条 warning：{:?}",
        outcome.warnings
    );
    assert!(outcome.warnings[0].contains("orig-session-key"));
    assert!(outcome.warnings[0].contains("brand-new-session"));
    cleanup_home(&home);
}

#[test]
fn exec_first_turn_injects_plan_body_user_meta_only_once() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("oneshot", PlanFileMode::Planning);
    rt.build_plan("oneshot", None).unwrap();
    // 第一轮：返回 Some(plan 全文)
    let body1 = rt
        .consume_first_exec_turn_user_meta()
        .expect("首轮应返回 plan body");
    assert!(body1.contains("plan_id: oneshot"), "应含 frontmatter plan_id");
    assert!(body1.contains("## Goal"), "应含正文");
    assert!(body1.contains("mode: executing"), "frontmatter 已更新为 executing");

    // 第二、第三轮：返回 None（防止重复注入）
    assert!(rt.consume_first_exec_turn_user_meta().is_none());
    assert!(rt.consume_first_exec_turn_user_meta().is_none());
    cleanup_home(&home);
}

#[test]
fn plan_build_atomic_rollback_on_write_failure() {
    // 制造 write_plan 失败：把 ~/.tomcat/plans 替换为只读普通文件 → write 时 sync_all 或 rename 失败。
    // 平台无关方案：用一个被锁定且超 timeout 的同名 lock 文件触发 LockBusy → write_plan 直接报错。
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("rollback", PlanFileMode::Planning);

    use crate::api::chat::plan_runtime::file_store::*;
    use fs2::FileExt;
    let plan_path = plan_path_for_id("rollback").unwrap();
    let lock_path = plan_path.with_file_name(format!(
        "{}.lock",
        plan_path.file_name().unwrap().to_string_lossy()
    ));
    let _f = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    _f.try_lock_exclusive().unwrap();

    // 用极短超时构造 rt，加快测试
    let rt = PlanRuntime::with_lock_timeout("session-a", 50);
    let err = rt.build_plan("rollback", None).unwrap_err();
    // 内存 mode 必须未变（仍 Chat）— 这才是"原子回滚"的核心
    assert!(matches!(rt.mode(), PlanMode::Chat), "内存 mode 必须未提升");
    // first_exec_turn_pending 也必须未置 true
    assert!(!rt.first_exec_turn_pending_for_test());
    match err {
        PlanRuntimeError::Io(s) => {
            assert!(s.contains("锁") || s.contains("lock") || s.contains("LockBusy"), "应是锁/IO 错：{s}")
        }
        other => panic!("expected Io (LockBusy), got {other:?}"),
    }

    // 释放锁，rt 仍可继续 build（证明状态可恢复）
    FileExt::unlock(&_f).unwrap();
    drop(_f);
    let rt = PlanRuntime::with_lock_timeout("session-a", 1000);
    let _ok = rt.build_plan("rollback", None).expect("放锁后 build 应成功");
    assert!(matches!(rt.mode(), PlanMode::Executing { .. }));
    cleanup_home(&home);
}
