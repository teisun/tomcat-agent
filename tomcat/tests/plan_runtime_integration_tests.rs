//! plan_runtime 集成测试（plan §9.4）。
//!
//! 范围：端到端串起 PlanRuntime 五态 + 工具（create_plan / update_plan / todos / ask_question）
//! + 关键 D 防御路径（D1 lock release / D2 cancel hook / D8 ask_question cancel）。
//!
//! 测试约束（plan §6 / §9.4）：
//! - 所有 `await` 必须 `tokio::time::timeout(30s, ...)` 包裹（防 D12 hang）；
//! - HOME env 通过 [`isolated_home`] 在 tmp 中隔离；每个测试 owns 自己的 dir；
//! - **不**真正连 LLM provider；reviewer 用 mock dispatcher、ask_question 用 mock panel。

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use tomcat::api::chat::plan_runtime::{
    ask_question_panel::{
        Answer, AskQuestionPanel, AskQuestionResult, MockAskQuestionPanel, Question,
        QuestionOption, CUSTOM_OPTION_ID,
    },
    file_store::{PlanFileMode, TodoStatus},
    mode::PlanMode,
    review::ReviewSummary,
    tools::{create_plan, todos, update_plan},
    PlanRuntime, ReviewerDispatcher,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// 设置一个 process-唯一 HOME 隔离目录，测试结束后还原。
fn isolated_home() -> std::path::PathBuf {
    use std::sync::OnceLock;
    static ORIG: OnceLock<Option<String>> = OnceLock::new();
    ORIG.get_or_init(|| std::env::var("HOME").ok());
    let p = std::env::temp_dir().join(format!(
        "tomcat_plan_int_test_{}_{}",
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
    use std::sync::OnceLock;
    static ORIG: OnceLock<Option<String>> = OnceLock::new();
    let orig = ORIG.get_or_init(|| std::env::var("HOME").ok());
    let _ = std::fs::remove_dir_all(p);
    match orig {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
}

/// HOME 隔离测试串行化（plan tools 测试改 HOME，并发会互踩）。
fn home_lock() -> &'static std::sync::Mutex<()> {
    static M: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(()))
}

struct AcceptReviewer;
#[async_trait::async_trait]
impl ReviewerDispatcher for AcceptReviewer {
    async fn dispatch(
        &self,
        _plan_id: &str,
        _plan_text: &str,
        _allow_review_edit: bool,
        _abort: Arc<AtomicBool>,
    ) -> ReviewSummary {
        ReviewSummary {
            aborted: false,
            summary: "looks good".into(),
            changes_summary: "none".into(),
            applied_changes: false,
        }
    }
}

// ─── E2E-PLAN-001：完整生命周期 create → build → exec→completed → chat ────

#[tokio::test]
async fn full_plan_lifecycle_create_build_complete() {
    let _g = home_lock().lock().unwrap();
    let home = isolated_home();
    let rt = PlanRuntime::new("lifecycle-session");

    let body = tokio::time::timeout(DEFAULT_TIMEOUT, async {
        // 1) /plan → Planning
        rt.enter_planning("end-to-end").unwrap();
        assert!(matches!(rt.mode(), PlanMode::Planning));

        // 2) create_plan → PlanFile 落盘 + active_planning_plan_id
        let out = create_plan::execute(
            &rt,
            create_plan::CreatePlanArgs {
                plan_id: "lifecycle".into(),
                goal: "ship full path".into(),
                body: Some("## Goal\n收口".into()),
                milestones: vec![],
                todos: vec![
                    create_plan::TodoArg {
                        id: "a".into(),
                        content: "step a".into(),
                        status: TodoStatus::Pending,
                        milestone_id: None,
                    },
                    create_plan::TodoArg {
                        id: "b".into(),
                        content: "step b".into(),
                        status: TodoStatus::Pending,
                        milestone_id: None,
                    },
                ],
            },
        )
        .unwrap();
        assert_eq!(out["plan_id"], "lifecycle");
        assert_eq!(rt.active_planning_plan_id().as_deref(), Some("lifecycle"));

        // 3) /plan build → EXEC + 首轮 user_meta 缓存
        let outcome = rt.build_plan("lifecycle", Some("uuid-1".into())).unwrap();
        assert!(matches!(rt.mode(), PlanMode::Executing { .. }));
        assert!(matches!(outcome.prev_disk_mode, PlanFileMode::Planning));
        let body = rt.consume_first_exec_turn_user_meta().expect("首轮 body");
        assert!(body.contains("## Goal"));
        assert!(body.contains("mode: executing"));
        // 第二次返回 None
        assert!(rt.consume_first_exec_turn_user_meta().is_none());

        // 4) update_plan：a in_progress
        update_plan::execute(
            &rt,
            update_plan::UpdatePlanArgs {
                plan_id: Some("lifecycle".into()),
                ops: vec![update_plan::UpdateOp::SetStatus {
                    id: "a".into(),
                    status: TodoStatus::InProgress,
                }],
                milestones_ops: vec![],
            },
        )
        .unwrap();
        // 5) update_plan：a completed
        update_plan::execute(
            &rt,
            update_plan::UpdatePlanArgs {
                plan_id: Some("lifecycle".into()),
                ops: vec![update_plan::UpdateOp::SetStatus {
                    id: "a".into(),
                    status: TodoStatus::Completed,
                }],
                milestones_ops: vec![],
            },
        )
        .unwrap();
        // 6) update_plan：b completed → 全 completed → 内存切 Completed
        let out_final = update_plan::execute(
            &rt,
            update_plan::UpdatePlanArgs {
                plan_id: Some("lifecycle".into()),
                ops: vec![update_plan::UpdateOp::SetStatus {
                    id: "b".into(),
                    status: TodoStatus::Completed,
                }],
                milestones_ops: vec![],
            },
        )
        .unwrap();
        assert_eq!(out_final["plan_mode_after"], "completed");
        assert!(matches!(rt.mode(), PlanMode::Completed { .. }));

        // 7) finalize → Chat
        let pid = rt.finalize_completed_to_chat().expect("Some(plan_id)");
        assert_eq!(pid, "lifecycle");
        assert!(matches!(rt.mode(), PlanMode::Chat));
        "ok"
    })
    .await
    .expect("生命周期测试超时");
    assert_eq!(body, "ok");
    cleanup_home(&home);
}

// ─── E2E-PLAN-002：cancel → pending → 续跑 ──────────────────────────────

#[tokio::test]
async fn build_then_cancel_demotes_pending_and_resume_works() {
    let _g = home_lock().lock().unwrap();
    let home = isolated_home();
    let rt = PlanRuntime::new("ses-cancel");

    tokio::time::timeout(DEFAULT_TIMEOUT, async {
        rt.enter_planning("obj").unwrap();
        create_plan::execute(
            &rt,
            create_plan::CreatePlanArgs {
                plan_id: "cancel_able".into(),
                goal: "long task".into(),
                body: None,
                milestones: vec![],
                todos: vec![create_plan::TodoArg {
                    id: "t1".into(),
                    content: "long".into(),
                    status: TodoStatus::Pending,
                    milestone_id: None,
                }],
            },
        )
        .unwrap();
        rt.build_plan("cancel_able", None).unwrap();
        // 模拟 Ctrl+C
        rt.demote_to_pending_on_cancel().unwrap();
        assert!(matches!(rt.mode(), PlanMode::Pending { .. }));

        // 续跑：必须能成功（lock 已释放，frontmatter.mode=pending 合规）
        // 切回 Chat 才能 build（Pending 不能 build；用户 /plan exit）
        rt.exit_to_chat().unwrap();
        let out = rt.build_plan("cancel_able", None).unwrap();
        assert!(matches!(out.prev_disk_mode, PlanFileMode::Pending));
        assert!(matches!(rt.mode(), PlanMode::Executing { .. }));
    })
    .await
    .expect("cancel→resume 超时");
    cleanup_home(&home);
}

// ─── E2E-PLAN-AQ：ask_question 集成（含 D8 cancel 路径） ─────────────────

#[tokio::test]
async fn ask_question_returns_recommended_then_custom_text() {
    let _g = home_lock().lock().unwrap();
    let home = isolated_home();
    let rt = PlanRuntime::new("ses-aq");
    rt.enter_planning("aq").unwrap();
    let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
        answers: vec![
            Answer {
                question_id: "q1".into(),
                option_ids: vec!["yes".into()],
                custom_text: None,
                picked_recommended: true,
            },
            Answer {
                question_id: "q2".into(),
                option_ids: vec![CUSTOM_OPTION_ID.into()],
                custom_text: Some("free-form choice".into()),
                picked_recommended: false,
            },
        ],
        cancelled: false,
    }]);

    let args = serde_json::json!({
        "questions": [
            {
                "id":"q1","prompt":"是否继续?","allow_multiple":false,
                "options":[
                    {"id":"yes","label":"继续","recommended":true},
                    {"id":"no","label":"取消"}
                ]
            },
            {
                "id":"q2","prompt":"补充信息?","allow_multiple":false,
                "options":[
                    {"id":"a","label":"A","recommended":true},
                    {"id":"b","label":"B"}
                ]
            }
        ]
    });
    let out = tokio::time::timeout(
        DEFAULT_TIMEOUT,
        tomcat::api::chat::plan_runtime::tools::ask_question::execute(
            &rt,
            &panel,
            &args,
            Arc::new(AtomicBool::new(false)),
        ),
    )
    .await
    .expect("ask_question 超时")
    .unwrap();
    assert_eq!(out["cancelled"], false);
    let answers = out["answers"].as_array().unwrap();
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0]["option_ids"][0], "yes");
    assert_eq!(answers[0]["picked_recommended"], true);
    assert_eq!(answers[1]["option_ids"][0], CUSTOM_OPTION_ID);
    assert_eq!(answers[1]["custom_text"], "free-form choice");
    cleanup_home(&home);
}

#[tokio::test]
async fn ask_question_user_ctrl_c_during_wait_returns_cancelled_not_err() {
    // D8 防御：用户 Ctrl+C 中断 ask_question 必须立即返回 cancelled，不可 hang
    let _g = home_lock().lock().unwrap();
    let home = isolated_home();
    let rt = PlanRuntime::new("ses-cancel");
    rt.enter_planning("aq cancel").unwrap();

    let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
        answers: vec![],
        cancelled: false,
    }])
    .with_delay(Duration::from_secs(10));
    let args = serde_json::json!({
        "questions": [{
            "id":"q1","prompt":"long?","allow_multiple":false,
            "options":[
                {"id":"a","label":"A","recommended":true},
                {"id":"b","label":"B"}
            ]
        }]
    });
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_clone = Arc::clone(&cancel);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let out = tokio::time::timeout(
        DEFAULT_TIMEOUT,
        tomcat::api::chat::plan_runtime::tools::ask_question::execute(
            &rt, &panel, &args, cancel,
        ),
    )
    .await
    .expect("ask_question cancel 超时（D8 失效）")
    .unwrap();
    assert_eq!(out["cancelled"], true);
    cleanup_home(&home);
}

// ─── E2E-PLAN-RV：reviewer 派发整链路（D1 lock release） ──────────────────

#[tokio::test]
async fn create_plan_dispatches_reviewer_summary_into_tool_result() {
    let _g = home_lock().lock().unwrap();
    let home = isolated_home();
    let rt = PlanRuntime::new("ses-rv");
    rt.attach_reviewer(Arc::new(AcceptReviewer));
    rt.enter_planning("integration").unwrap();
    let out = tokio::time::timeout(
        DEFAULT_TIMEOUT,
        create_plan::execute_with_reviewer(
            &rt,
            create_plan::CreatePlanArgs {
                plan_id: "rv_int".into(),
                goal: "integ".into(),
                body: None,
                milestones: vec![],
                todos: vec![],
            },
            false,
        ),
    )
    .await
    .expect("reviewer 集成超时")
    .unwrap();
    assert_eq!(out["plan_id"], "rv_int");
    assert_eq!(out["review"]["aborted"], false);
    assert_eq!(out["review"]["summary"], "looks good");
    cleanup_home(&home);
}

// ─── E2E-PLAN-RAW：raw edit 拦截 ──────────────────────────────────────────

#[tokio::test]
async fn raw_edit_to_plan_file_blocked_in_planning_and_executing() {
    let _g = home_lock().lock().unwrap();
    let home = isolated_home();
    let rt = PlanRuntime::new("ses-raw");
    rt.enter_planning("p").unwrap();
    create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            plan_id: "raw_guard".into(),
            goal: "g".into(),
            body: None,
            milestones: vec![],
            todos: vec![],
        },
    )
    .unwrap();

    let target = home.join(".tomcat").join("plans").join("raw_guard.plan.md");
    assert!(target.exists());

    // Planning 模式 → 拒
    assert!(!rt.allow_raw_edit_to_path(&target));

    // EXEC 模式 → 拒
    rt.build_plan("raw_guard", None).unwrap();
    assert!(!rt.allow_raw_edit_to_path(&target));

    // CHAT 模式 → 允许（用户手工 vim 编辑 plan 文件不被运行时拦）
    rt.demote_to_pending_on_cancel().unwrap(); // EXEC → Pending
    rt.exit_to_chat().unwrap(); // Pending → Chat
    assert!(rt.allow_raw_edit_to_path(&target));

    cleanup_home(&home);
}

// ─── E2E-PLAN-008/013：todos 路由 + 并发不破坏 ─────────────────────────────

#[tokio::test]
async fn todos_routes_to_plan_file_in_exec_and_session_in_chat() {
    let _g = home_lock().lock().unwrap();
    let home = isolated_home();
    let rt = PlanRuntime::new("ses-todos");

    // CHAT 模式：todos 走 session scratchpad
    todos::execute(
        &rt,
        todos::TodosArgs {
            ops: vec![todos::TodoOpArg::AddTodo {
                id: "session_x".into(),
                content: "scratch".into(),
                status: TodoStatus::Pending,
                milestone_id: None,
            }],
        },
    )
    .unwrap();
    assert_eq!(rt.snapshot_session_todos().len(), 1);

    // 闸门：把 session todo 收口为 completed，build 闸门才允许放行
    todos::execute(
        &rt,
        todos::TodosArgs {
            ops: vec![todos::TodoOpArg::SetStatus {
                id: "session_x".into(),
                status: TodoStatus::Completed,
            }],
        },
    )
    .unwrap();

    // 进 EXEC：todos 应写 PlanFile（而非 session）
    rt.enter_planning("p").unwrap();
    create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            plan_id: "td_route".into(),
            goal: "g".into(),
            body: None,
            milestones: vec![],
            todos: vec![],
        },
    )
    .unwrap();
    rt.build_plan("td_route", None).unwrap();

    let n_before = rt.snapshot_session_todos().len();
    todos::execute(
        &rt,
        todos::TodosArgs {
            ops: vec![todos::TodoOpArg::AddTodo {
                id: "plan_y".into(),
                content: "in plan".into(),
                status: TodoStatus::Pending,
                milestone_id: None,
            }],
        },
    )
    .unwrap();
    // session 未变化
    assert_eq!(rt.snapshot_session_todos().len(), n_before);
    // PlanFile 加了一项
    use tomcat::api::chat::plan_runtime::file_store::*;
    let plan = read_plan(&plan_path_for_id("td_route").unwrap()).unwrap();
    assert!(plan.frontmatter.todos.iter().any(|t| t.id == "plan_y"));
    cleanup_home(&home);
}

// ─── E2E-PLAN-016：错误用户输入友好提示 ──────────────────────────────────

#[tokio::test]
async fn build_unknown_plan_id_returns_friendly_hint() {
    let _g = home_lock().lock().unwrap();
    let home = isolated_home();
    let rt = PlanRuntime::new("ses-err");
    let err = rt.build_plan("does_not_exist", None).unwrap_err();
    let s = err.to_string();
    assert!(s.contains("does_not_exist"), "{s}");
    cleanup_home(&home);
}

// Suppress unused import for clarity (Question/QuestionOption only used via JSON):
#[allow(dead_code)]
fn _types_alive(_q: Question, _o: QuestionOption, _p: Box<dyn AskQuestionPanel>) {}
