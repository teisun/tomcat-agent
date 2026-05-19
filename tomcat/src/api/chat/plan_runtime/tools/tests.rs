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
