//! plan tools 单元测试（§9.3B）。
//!
//! 测试策略：
//! - **路径隔离**：`tools/*` 用 `plan_path_for_id` 走 `~/.tomcat/plans/`；测试用 `HOME` env override 隔离。
//! - **不变量验证**：mode 守卫 / 跨 session 守卫 / 单 in_progress / id 唯一。
//! - **完整 snapshot 返回**：每个工具结果都包含 `items` / `applied` / `active_in_progress`。

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
fn home_lock() -> &'static std::sync::Mutex<()> {
    crate::test_support::home_env_lock()
}

/// **模块级**记录的首个 HOME 快照（在第一次 setup_isolated_home 前抓取，永不更新）。
/// 用于 cleanup_home 还原 — 避免 HOME 污染其他 suite（permission gate / cli config_keys）。
fn orig_home() -> &'static Option<String> {
    static ORIG_HOME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    ORIG_HOME.get_or_init(|| std::env::var("HOME").ok())
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
    // 在覆盖 HOME 之前先 lazy-init ORIG_HOME — 保证它一定捕获到原始 HOME。
    let _ = orig_home();
    std::env::set_var("HOME", &p);
    p
}

/// 测试结束后清理 tmp + 还原 HOME（防 D-test：HOME 污染破坏 permission gate / cli config_keys 等套件）。
fn cleanup_home(p: &std::path::Path) {
    let _ = std::fs::remove_dir_all(p);
    match orig_home() {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
}

// ─── create_plan ────────────────────────────────────────────────────────────

#[test]
fn create_plan_invisible_outside_planning_returns_error() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    // Chat 模式
    let args = create_plan::CreatePlanArgs {
        goal: "g".into(),
        draft: "d".into(),
        todos: vec![create_plan::TodoArg {
            id: "t1".into(),
            content: "x".into(),
            status: TodoStatus::Pending,
        }],
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
    rt.enter_planning().unwrap();
    let args = create_plan::CreatePlanArgs {
        goal: "为 chat 补齐 plan 闭环".into(),
        draft: "step 1; step 2; step 3".into(),
        todos: vec![create_plan::TodoArg {
            id: "t1".into(),
            content: "first".into(),
            status: TodoStatus::Pending,
        }],
    };
    let out = create_plan::execute(&rt, args).expect("create_plan OK");
    let plan_id = out["plan_id"]
        .as_str()
        .expect("plan_id present")
        .to_string();
    assert!(
        plan_id.starts_with("plan_"),
        "派生 plan_id 应以 plan_ 开头: {plan_id}"
    );
    assert_eq!(out["mode"], "planning");
    assert_eq!(out["review"]["aborted"], serde_json::Value::Bool(true));

    assert_eq!(
        rt.active_planning_plan_id().as_deref(),
        Some(plan_id.as_str())
    );
    let path = home
        .join(".tomcat")
        .join("plans")
        .join(format!("{plan_id}.plan.md"));
    assert!(path.exists(), "{path:?} 应该已写盘");
    let plan_text = std::fs::read_to_string(&path).expect("plan file readable");
    assert!(plan_text.contains("## Goal"), "新 plan 应包含 ## Goal 段");
    assert!(plan_text.contains("## Plan"), "新 plan 应包含 ## Plan 段");
    assert!(
        !plan_text.contains("## Draft"),
        "新 plan 不应再带 ## Draft 段"
    );
    assert!(
        !plan_text.contains("## Notes"),
        "新 plan 不应再带 ## Notes 段"
    );
    assert!(
        !plan_text.contains("## Review"),
        "新 plan 不应再带 ## Review 段"
    );
    cleanup_home(&home);
}

#[test]
fn create_plan_normalizes_legacy_heading_wrapped_draft() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.enter_planning().unwrap();
    let args = create_plan::CreatePlanArgs {
        goal: "Draft a minimal internal plan with two clear next steps.".into(),
        draft: "## Goal\n\nDraft a minimal internal plan.\n\n## Notes\n\nKeep scope small and actionable.\n".into(),
        todos: vec![create_plan::TodoArg {
            id: "t1".into(),
            content: "first".into(),
            status: TodoStatus::Pending,
        }],
    };
    let out = create_plan::execute(&rt, args).expect("create_plan OK");
    let plan_id = out["plan_id"]
        .as_str()
        .expect("plan_id present")
        .to_string();
    let path = home
        .join(".tomcat")
        .join("plans")
        .join(format!("{plan_id}.plan.md"));
    let plan_text = std::fs::read_to_string(&path).expect("plan file readable");
    assert_eq!(
        plan_text.matches("## Goal").count(),
        1,
        "plan 正文里不应重复写出第二个 ## Goal"
    );
    assert_eq!(
        plan_text.matches("## Plan").count(),
        1,
        "plan 正文里应只保留一个 ## Plan"
    );
    assert!(
        !plan_text.contains("## Draft") && !plan_text.contains("## Notes"),
        "旧式 Draft/Notes heading 应在写盘前被规范化"
    );
    assert!(
        plan_text.contains("Keep scope small and actionable."),
        "旧式 Notes 内容应并入 ## Plan，而不是整段丢失"
    );
    cleanup_home(&home);
}

#[test]
fn create_plan_rejects_empty_goal() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.enter_planning().unwrap();
    let args = create_plan::CreatePlanArgs {
        goal: "".into(),
        draft: "d".into(),
        todos: vec![create_plan::TodoArg {
            id: "t1".into(),
            content: "x".into(),
            status: TodoStatus::Pending,
        }],
    };
    let err = create_plan::execute(&rt, args).expect_err("空 goal 应被拒");
    matches!(err, ToolError::BadArgs(_));
    cleanup_home(&home);
}

#[test]
fn create_plan_rejects_empty_draft_or_todos() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.enter_planning().unwrap();
    // empty draft
    let err = create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            goal: "g".into(),
            draft: "   ".into(),
            todos: vec![create_plan::TodoArg {
                id: "t1".into(),
                content: "x".into(),
                status: TodoStatus::Pending,
            }],
        },
    )
    .expect_err("空 draft 应被拒");
    matches!(err, ToolError::BadArgs(_));
    // empty todos
    let err = create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            goal: "g".into(),
            draft: "d".into(),
            todos: vec![],
        },
    )
    .expect_err("空 todos 应被拒");
    matches!(err, ToolError::BadArgs(_));
    cleanup_home(&home);
}

#[test]
fn create_plan_from_json_rejects_legacy_plan_id_and_body() {
    // D3：LLM 误传旧字段 → BadArgs。
    let err = create_plan::CreatePlanArgs::from_json(&serde_json::json!({
        "plan_id": "x",
        "goal": "g",
        "draft": "d",
        "todos": [],
    }))
    .expect_err("旧字段 plan_id 应被拒");
    matches!(err, ToolError::BadArgs(_));
    let err = create_plan::CreatePlanArgs::from_json(&serde_json::json!({
        "goal": "g",
        "body": "old",
        "todos": [],
    }))
    .expect_err("旧字段 body 应被拒");
    matches!(err, ToolError::BadArgs(_));
}

#[test]
fn create_plan_derived_id_passes_safety_check() {
    // 即使 goal 全是非字母数字字符，派生 id 仍应通过 assert_plan_id_safe。
    let id = create_plan::derive_plan_id("@#$%^");
    crate::api::chat::plan_runtime::safety::assert_plan_id_safe(&id).unwrap();
    let id = create_plan::derive_plan_id("");
    crate::api::chat::plan_runtime::safety::assert_plan_id_safe(&id).unwrap();
}

// ─── update_plan ────────────────────────────────────────────────────────────

/// 创建一个 Planning 模式下的 plan，返回**派生**的 `plan_id`（G4：LLM 不传 id）。
/// 调用方应 capture 返回的 plan_id 用于后续 update_plan / build。
fn fresh_planning_plan(rt: &PlanRuntime) -> String {
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

#[test]
fn update_plan_set_status_returns_full_items_snapshot() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt);
    // G1 + G2：set_status(in_progress) 仅在 executing 允许；先把 plan 切到 executing 再走 update_plan
    use crate::api::chat::plan_runtime::file_store::{
        plan_path_for_id, read_plan, write_plan, PlanFileMode,
    };
    let path = plan_path_for_id(&plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.mode = PlanFileMode::Executing;
    plan.frontmatter.session_key = Some("session-a".into());
    plan.frontmatter.session_id = Some("sid-a".into());
    write_plan(&path, &plan, 2000).unwrap();
    rt.set_executing_for_test(plan_id.clone());

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        },
    )
    .unwrap();
    assert_eq!(out["plan_id"], plan_id);
    let items = out["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["status"], "in_progress");
    assert_eq!(items[1]["status"], "pending");
    assert!(out.get("path").is_some());
    assert!(out.get("panel_snapshot_id").is_some());
    assert_eq!(out["active_in_progress"], "t1");
    cleanup_home(&home);
}

#[test]
fn update_plan_reuses_todos_op_engine_single_in_progress_violation() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt);
    // 切 executing 才能用 in_progress
    use crate::api::chat::plan_runtime::file_store::{
        plan_path_for_id, read_plan, write_plan, PlanFileMode,
    };
    let path = plan_path_for_id(&plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.mode = PlanFileMode::Executing;
    plan.frontmatter.session_key = Some("session-a".into());
    write_plan(&path, &plan, 2000).unwrap();
    rt.set_executing_for_test(plan_id.clone());

    let err = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: false,
            ops: vec![
                update_plan::UpdateOp::SetStatus {
                    id: "t1".into(),
                    content: None,
                    status: TodoStatus::InProgress,
                },
                update_plan::UpdateOp::SetStatus {
                    id: "t2".into(),
                    content: None,
                    status: TodoStatus::InProgress,
                },
            ],
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
    let plan_id = fresh_planning_plan(&rt_a);
    // session-b 不同 session，对同一 plan（planning）做 update_plan：允许
    let rt_b = PlanRuntime::new("session-b");
    rt_b.enter_planning().unwrap();
    let out = update_plan::execute(
        &rt_b,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::Upsert {
                id: "t1".into(),
                content: Some("edited by b".into()),
                status: None,
            }],
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
    let plan_id = fresh_planning_plan(&rt_a);
    // 模拟 session-a /plan build：手动写 frontmatter 为 executing + session_key
    let path = plan_path_for_id(&plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.mode = PlanFileMode::Executing;
    plan.frontmatter.session_key = Some("session-a".into());
    plan.frontmatter.session_id = Some("sid-a".into());
    write_plan(&path, &plan, 2000).unwrap();

    let rt_b = PlanRuntime::new("session-b");
    let err = update_plan::execute(
        &rt_b,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::Upsert {
                id: "t1".into(),
                content: Some("intruder".into()),
                status: None,
            }],
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
    let plan_id = fresh_planning_plan(&rt);
    // 手动 build：frontmatter executing + session_key 写当前 session
    let path = plan_path_for_id(&plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.mode = PlanFileMode::Executing;
    plan.frontmatter.session_key = Some("session-a".into());
    plan.frontmatter.session_id = Some("sid-a".into());
    write_plan(&path, &plan, 2000).unwrap();
    rt.set_executing_for_test(plan_id.clone());

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            ops: vec![
                update_plan::UpdateOp::SetStatus {
                    id: "t1".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
                update_plan::UpdateOp::SetStatus {
                    id: "t2".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
            ],
        },
    )
    .unwrap();
    assert_eq!(out["plan_mode_before"], "executing");
    assert_eq!(out["plan_mode_after"], "completed");

    // 内存切到 Completed
    match rt.mode() {
        PlanMode::Completed { plan_id: cur } => assert_eq!(cur, plan_id),
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
            new_todos: false,
            title: None,
            replace: false,
            ops: vec![
                todos::TodoOpArg::Upsert {
                    id: "x1".into(),
                    content: Some("scratchpad 1".into()),
                    status: Some(TodoStatus::Pending),
                },
                todos::TodoOpArg::Upsert {
                    id: "x2".into(),
                    content: Some("scratchpad 2".into()),
                    status: Some(TodoStatus::Pending),
                },
                todos::TodoOpArg::SetStatus {
                    id: "x1".into(),
                    content: None,
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
fn todos_persists_to_disk_when_persist_base_configured() {
    // G3：注入 persist_base 后，session 落盘到
    // `<base>/sessions/<session_key>/todos/<active_todos_id>.todo.md`。
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let base = home.join(".tomcat").join("agents").join("main");
    rt.set_todos_persist_base(Some(base.clone()));
    let out = todos::execute(
        &rt,
        todos::TodosArgs {
            new_todos: false,
            title: None,
            replace: false,
            ops: vec![todos::TodoOpArg::Upsert {
                id: "p1".into(),
                content: Some("persist me".into()),
                status: Some(TodoStatus::Pending),
            }],
        },
    )
    .unwrap();
    let active_id = out["active_todos_id"].as_str().expect("active_todos_id");
    let expected = base
        .join("sessions")
        .join("session-a")
        .join("todos")
        .join(format!("{active_id}.todo.md"));
    assert!(expected.exists(), "落盘文件应存在: {expected:?}");
    let body = std::fs::read_to_string(&expected).unwrap();
    assert!(body.contains("p1: persist me"));
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
            new_todos: false,
            title: None,
            replace: false,
            ops: vec![todos::TodoOpArg::Upsert {
                id: "x1".into(),
                content: Some("should not touch plan".into()),
                status: Some(TodoStatus::Pending),
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
    assert!(
        entries.is_empty(),
        "CHAT 下 todos 不应写 plan，发现：{entries:?}"
    );
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
            new_todos: false,
            title: None,
            replace: false,
            ops: vec![
                todos::TodoOpArg::Upsert {
                    id: "x1".into(),
                    content: Some("1".into()),
                    status: Some(TodoStatus::InProgress),
                },
                todos::TodoOpArg::Upsert {
                    id: "x2".into(),
                    content: Some("2".into()),
                    status: Some(TodoStatus::Pending),
                },
            ],
        },
    )
    .unwrap();
    let err = todos::execute(
        &rt,
        todos::TodosArgs {
            new_todos: false,
            title: None,
            replace: false,
            ops: vec![todos::TodoOpArg::SetStatus {
                id: "x2".into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        },
    )
    .expect_err("第二个 in_progress 应被 ops 引擎拒");
    matches!(err, ToolError::Op(_));
    cleanup_home(&home);
}

#[test]
fn todos_in_exec_writes_session_not_plan_file() {
    // D 方案：todos 在任何模式（含 EXEC）都只写 session 本地 scratchpad，绝不动 PlanFile。
    use crate::api::chat::plan_runtime::file_store::{plan_path_for_id, read_plan, write_plan};
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt);
    let path = plan_path_for_id(&plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.mode = PlanFileMode::Executing;
    plan.frontmatter.session_key = Some("session-a".into());
    plan.frontmatter.session_id = Some("sid-a".into());
    write_plan(&path, &plan, 2000).unwrap();
    rt.set_executing_for_test(plan_id);

    // 先在 session 里 add 一条，再 set_status，模拟 LLM 用 todos 当 scratchpad
    todos::execute(
        &rt,
        todos::TodosArgs {
            new_todos: false,
            title: None,
            replace: false,
            ops: vec![todos::TodoOpArg::Upsert {
                id: "sub-1".into(),
                content: Some("debug step".into()),
                status: Some(TodoStatus::Pending),
            }],
        },
    )
    .unwrap();
    let out = todos::execute(
        &rt,
        todos::TodosArgs {
            new_todos: false,
            title: None,
            replace: false,
            ops: vec![todos::TodoOpArg::SetStatus {
                id: "sub-1".into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        },
    )
    .unwrap();
    assert_eq!(out["scope"], "session");
    // PlanFile.todos 不应被 todos 工具改动
    let parsed = read_plan(&path).unwrap();
    assert_eq!(parsed.frontmatter.todos[0].status, TodoStatus::Pending);
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
        ..Default::default()
    }
}

fn good_args_with_todo() -> create_plan::CreatePlanArgs {
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

#[tokio::test]
async fn create_plan_internally_dispatches_reviewer_with_real_summary() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.attach_reviewer(std::sync::Arc::new(MockReviewerDispatcher::new(vec![
        ok_review(),
    ])));
    rt.enter_planning().unwrap();
    let out = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), false)
        .await
        .unwrap();
    assert!(out["plan_id"].as_str().unwrap().starts_with("plan_"));
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
    rt.enter_planning().unwrap();
    let out = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), false)
        .await
        .unwrap();
    let plan_id = out["plan_id"].as_str().unwrap().to_string();
    assert_eq!(out["review"]["aborted"], serde_json::Value::Bool(true));
    assert!(out["review"]["summary"]
        .as_str()
        .unwrap()
        .contains("parse error"));
    let plan_path = home
        .join(".tomcat")
        .join("plans")
        .join(format!("{plan_id}.plan.md"));
    assert!(plan_path.exists());
    cleanup_home(&home);
}

#[tokio::test]
async fn create_plan_without_reviewer_returns_placeholder() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.enter_planning().unwrap();
    let out = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), false)
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
                    ..Default::default()
                },
                Err(e) => ReviewSummary::aborted_with(format!("LockBusy: {e}")),
            }
        }
    }
    rt.attach_reviewer(std::sync::Arc::new(LockAcquiringMock));
    rt.enter_planning().unwrap();
    let out = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), false)
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

/// reviewer.md §11 RV-T7：plan.review transcript 自定义事件必须落 transcript_appender，
/// 含 `event=plan.review`、`plan_id`、`reviewer_turns_*`、`reviewer_stop_reason`。
#[test]
fn create_plan_writes_transcript_plan_create_event() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");

    let captured: std::sync::Arc<parking_lot::Mutex<Vec<serde_json::Value>>> =
        std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    {
        let sink = std::sync::Arc::clone(&captured);
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            sink.lock().push(extra);
            Ok(())
        }));
    }

    rt.enter_planning().unwrap();
    let out = create_plan::execute(&rt, good_args_with_todo()).expect("create_plan OK");
    let plan_id = out["plan_id"].as_str().unwrap().to_string();

    let events = captured.lock();
    let plan_create = events
        .iter()
        .find(|v| v["event"] == "plan.create")
        .expect("缺少 plan.create 事件");
    assert_eq!(plan_create["plan_id"], plan_id);
    assert_eq!(plan_create["mode"], "planning");
    assert!(
        plan_create["path"].as_str().unwrap().ends_with(".plan.md"),
        "plan.create 路径应指向计划文件：{:?}",
        plan_create
    );
    cleanup_home(&home);
}

/// reviewer.md §11 RV-T7：plan.review transcript 自定义事件必须落 transcript_appender，
/// 含 `event=plan.review`、`plan_id`、`reviewer_turns_*`、`reviewer_stop_reason`。
#[tokio::test]
async fn reviewer_summary_lands_in_transcript_plan_review() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");

    let captured: std::sync::Arc<parking_lot::Mutex<Vec<serde_json::Value>>> =
        std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    {
        let sink = std::sync::Arc::clone(&captured);
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            sink.lock().push(extra);
            Ok(())
        }));
    }

    let summary = ReviewSummary {
        aborted: false,
        summary: "ok".into(),
        changes_summary: "none".into(),
        applied_changes: false,
        reviewer_turns_used: 2,
        reviewer_turns_limit: 64,
        reviewer_stop_reason: "completed".into(),
        child_session_id: "child-1".into(),
        ..Default::default()
    };
    rt.attach_reviewer(std::sync::Arc::new(MockReviewerDispatcher::new(vec![
        summary,
    ])));
    rt.enter_planning().unwrap();
    let _ = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), true)
        .await
        .unwrap();

    let events = captured.lock();
    let plan_review = events
        .iter()
        .find(|v| v["event"] == "plan.review")
        .expect("缺少 plan.review 事件");
    assert_eq!(plan_review["reviewer_turns_used"], 2);
    assert_eq!(plan_review["reviewer_turns_limit"], 64);
    assert_eq!(plan_review["reviewer_stop_reason"], "completed");
    assert!(plan_review["plan_id"]
        .as_str()
        .unwrap()
        .starts_with("plan_"));
    cleanup_home(&home);
}

/// reviewer.md §11 RV-T8：第二轮 dispatch_reviewer 写一条 `plan.review.warning`，
/// 含 rounds=2，便于审计是否过度复盘。
#[tokio::test]
async fn reviewer_writes_warning_event_on_second_round() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");

    let captured: std::sync::Arc<parking_lot::Mutex<Vec<serde_json::Value>>> =
        std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    {
        let sink = std::sync::Arc::clone(&captured);
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            sink.lock().push(extra);
            Ok(())
        }));
    }
    rt.attach_reviewer(std::sync::Arc::new(MockReviewerDispatcher::new(vec![
        ok_review(),
        ok_review(),
    ])));
    rt.enter_planning().unwrap();
    let out1 = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), true)
        .await
        .unwrap();
    let plan_id = out1["plan_id"].as_str().unwrap().to_string();
    // 二次评审
    let _ = rt.dispatch_reviewer(&plan_id, true).await;

    let events = captured.lock();
    let warning = events
        .iter()
        .find(|v| v["event"] == "plan.review.warning")
        .expect("第二轮应有 plan.review.warning");
    assert_eq!(warning["rounds"], 2);
    cleanup_home(&home);
}

/// reviewer.md §11 RV-T9：父 cascade abort 信号传给 dispatcher 时返回 aborted=true 摘要。
/// MockReviewerDispatcher 不直接读 abort，但 ProdReviewerDispatcher 行为通过 outcome 转换实现；
/// 这里用 mock 验证 PlanRuntime 完整 ferry 了 abort_signal 到 dispatcher.dispatch（不丢字段）。
#[tokio::test]
async fn reviewer_dispatch_passes_through_abort_signal() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");

    struct AbortPeekMock {
        saw_signal: std::sync::Arc<AtomicBool>,
    }
    #[async_trait]
    impl ReviewerDispatcher for AbortPeekMock {
        async fn dispatch(
            &self,
            _plan_id: &str,
            _plan_text: &str,
            _allow_review_edit: bool,
            abort: std::sync::Arc<AtomicBool>,
        ) -> ReviewSummary {
            // 仅记录 dispatcher 收到了非 null 的 abort signal（不读其值）。
            self.saw_signal
                .store(true, std::sync::atomic::Ordering::Release);
            // 模拟"对方观察到 abort=true"返回 aborted=true。
            if abort.load(std::sync::atomic::Ordering::Acquire) {
                ReviewSummary::aborted_with("aborted by parent cascade")
            } else {
                ok_review()
            }
        }
    }
    let saw = std::sync::Arc::new(AtomicBool::new(false));
    rt.attach_reviewer(std::sync::Arc::new(AbortPeekMock {
        saw_signal: std::sync::Arc::clone(&saw),
    }));
    rt.enter_planning().unwrap();
    let out = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), true)
        .await
        .unwrap();
    assert!(
        saw.load(std::sync::atomic::Ordering::Acquire),
        "dispatcher 未收到 abort signal"
    );
    assert_eq!(out["review"]["aborted"], serde_json::Value::Bool(false));
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
    rt.enter_planning().unwrap();
    // 第一轮：使用稳定 goal，让两次 create_plan 派生出相同的 slug 前缀。
    let out1 = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), false)
        .await
        .unwrap();
    let plan_id_1 = out1["plan_id"].as_str().unwrap().to_string();
    assert!(!out1["review"]["summary"]
        .as_str()
        .unwrap()
        .starts_with("[round"));
    assert_eq!(rt.reviewer_rounds(&plan_id_1), 1);

    // 第二轮：手动用相同 plan_id 再次走 reviewer（模拟用户/agent 对同 plan 二次评审）。
    // 直接走 PlanRuntime 内部 dispatcher：summary 应带 [round 2] 前缀。
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _ = cancel;
    let summary = rt.dispatch_reviewer(&plan_id_1, false).await;
    assert!(summary.summary.starts_with("[round 2]"), "{summary:?}");
    assert_eq!(rt.reviewer_rounds(&plan_id_1), 2);
    cleanup_home(&home);
}

#[test]
fn from_json_helpers_reject_bad_args() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    // D3：plan_id 已不再是 create_plan 入参字段，from_json 应拒。
    let err = create_plan::CreatePlanArgs::from_json(&serde_json::json!({
        "plan_id": "x",
        "goal": "g",
        "draft": "d",
        "todos": [],
    }))
    .expect_err("plan_id 旧字段应被拒");
    matches!(err, ToolError::BadArgs(_));
    let err = update_plan::UpdatePlanArgs::from_json(&serde_json::json!({"ops": "not_array"}))
        .expect_err("ops 必须是数组");
    matches!(err, ToolError::BadArgs(_));
    let err = todos::TodosArgs::from_json(&serde_json::json!({})).expect_err("缺 ops 字段应被拒");
    matches!(err, ToolError::BadArgs(_));
    cleanup_home(&home);
}

#[test]
fn update_plan_from_json_accepts_set_status_with_extra_content_field() {
    let args = update_plan::UpdatePlanArgs::from_json(&serde_json::json!({
        "ops": [
            {
                "kind": "set_status",
                "id": "t1",
                "status": "in_progress",
                "content": "model carried over old field"
            }
        ]
    }))
    .expect("set_status 应容忍冗余 content 字段");

    assert_eq!(args.ops.len(), 1);
    match &args.ops[0] {
        update_plan::UpdateOp::SetStatus { id, status, .. } => {
            assert_eq!(id, "t1");
            assert_eq!(*status, TodoStatus::InProgress);
        }
        other => panic!("unexpected op parsed: {other:?}"),
    }
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
            assert!(
                hint.contains("create_plan"),
                "hint 应引导 create_plan：{hint}"
            );
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
    assert_eq!(
        plan.frontmatter.session_key.as_deref(),
        Some("new-session-key")
    );
    assert_eq!(
        plan.frontmatter.session_id.as_deref(),
        Some("new-session-uuid")
    );

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
    assert!(
        outcome.warnings.is_empty(),
        "同 key 续跑无 warning：{:?}",
        outcome.warnings
    );
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
fn exec_first_turn_injects_plan_meta_only_once() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("oneshot", PlanFileMode::Planning);
    rt.build_plan("oneshot", None).unwrap();
    // 第一轮：返回 Some(plan 全文)
    let body1 = rt
        .consume_first_exec_turn_user_meta()
        .expect("首轮应返回 plan body");
    assert!(
        body1.contains("plan_id: oneshot"),
        "应含 frontmatter plan_id"
    );
    assert!(body1.contains("## Goal"), "应含正文");
    assert!(
        body1.contains("mode: executing"),
        "frontmatter 已更新为 executing"
    );

    // 第二、第三轮：返回 None（防止重复注入）
    assert!(rt.consume_first_exec_turn_user_meta().is_none());
    assert!(rt.consume_first_exec_turn_user_meta().is_none());
    cleanup_home(&home);
}

// ─── P7 PR-PLE/PLF：cancel→pending、finalize completed、raw edit 拦截 ─────

#[test]
fn cancel_token_demotes_executing_to_pending() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("cancellable", PlanFileMode::Planning);
    rt.build_plan("cancellable", None).unwrap();
    assert!(matches!(rt.mode(), PlanMode::Executing { .. }));

    let demoted = rt.demote_to_pending_on_cancel().unwrap();
    assert_eq!(demoted.as_deref(), Some("cancellable"));
    match rt.mode() {
        PlanMode::Pending { plan_id } => assert_eq!(plan_id, "cancellable"),
        other => panic!("expected Pending, got {other:?}"),
    }
    // 首轮注入旗标也清掉（防 D5）
    assert!(!rt.first_exec_turn_pending_for_test());

    // 磁盘 frontmatter.mode = pending
    use crate::api::chat::plan_runtime::file_store::*;
    let plan = read_plan(&plan_path_for_id("cancellable").unwrap()).unwrap();
    assert!(matches!(plan.frontmatter.mode, PlanFileMode::Pending));
    cleanup_home(&home);
}

#[test]
fn cancel_outside_exec_is_noop() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    assert!(rt.demote_to_pending_on_cancel().unwrap().is_none());

    rt.enter_planning().unwrap();
    assert!(rt.demote_to_pending_on_cancel().unwrap().is_none());
    assert!(matches!(rt.mode(), PlanMode::Planning));
    cleanup_home(&home);
}

#[test]
fn attach_cancel_hook_rebinds_replaces_old_token() {
    // D2 防御：每轮 readline 后必须重挂 cancel_token；attach_cancel_hook 应替换上一轮 token
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let first = tokio_util::sync::CancellationToken::new();
    rt.attach_cancel_hook(first.clone());
    let cur = rt.current_cancel_token().expect("有 token");
    assert!(!cur.is_cancelled());

    let second = tokio_util::sync::CancellationToken::new();
    rt.attach_cancel_hook(second.clone());
    let cur2 = rt.current_cancel_token().expect("有 token");
    // 触发上一轮 token：current_cancel_token 应仍未 cancel
    first.cancel();
    assert!(!cur2.is_cancelled(), "上一轮 cancel 不应影响新 token");
    // 触发本轮 token：current 立即 cancelled
    second.cancel();
    let cur3 = rt.current_cancel_token().expect("有 token");
    assert!(cur3.is_cancelled());
    cleanup_home(&home);
}

#[test]
fn concurrent_write_plan_serialized_by_lock() {
    // D9 防御：双 thread 并发 write_plan 同一 plan_id，advisory lock 串行 → 不破坏数据。
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    use crate::api::chat::plan_runtime::file_store::*;
    let path = plan_path_for_id("hot_plan").unwrap();
    let base = PlanFile {
        frontmatter: PlanFileFrontmatter {
            plan_id: "hot_plan".into(),
            goal: "concurrent".into(),
            mode: PlanFileMode::Planning,
            session_key: None,
            session_id: None,
            created_at: "2026-05-19T00:00:00Z".into(),
            schema_version: 1,
            todos: vec![],
            unknown: Default::default(),
        },
        body: "## seed\n".into(),
    };
    write_plan(&path, &base, 2000).unwrap();
    // 两个线程同时把 todos 列表覆盖为不同内容，每个跑 5 轮。
    let p1 = path.clone();
    let p2 = path.clone();
    let h1 = std::thread::spawn(move || {
        for i in 0..5 {
            let mut plan = read_plan(&p1).unwrap();
            plan.frontmatter.todos = vec![TodoItem {
                id: format!("t{i}-a"),
                content: format!("a-{i}"),
                status: TodoStatus::Pending,
            }];
            write_plan(&p1, &plan, 2000).unwrap();
        }
    });
    let h2 = std::thread::spawn(move || {
        for i in 0..5 {
            let mut plan = read_plan(&p2).unwrap();
            plan.frontmatter.todos = vec![TodoItem {
                id: format!("t{i}-b"),
                content: format!("b-{i}"),
                status: TodoStatus::Pending,
            }];
            write_plan(&p2, &plan, 2000).unwrap();
        }
    });
    h1.join().unwrap();
    h2.join().unwrap();
    // 任意时刻磁盘 plan 都必须可解析、frontmatter 合法
    let final_plan = read_plan(&path).expect("最终态可解析");
    validate_frontmatter_invariants(&final_plan.frontmatter).expect("最终态合法");
    cleanup_home(&home);
}

#[test]
fn cancel_token_releases_plan_lock() {
    // D1 防御：demote 写盘后 lock 必须释放，否则下次 build 抢锁会超时
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::with_lock_timeout("session-a", 200);
    write_disk_plan("lockable", PlanFileMode::Planning);
    rt.build_plan("lockable", None).unwrap();
    rt.demote_to_pending_on_cancel().unwrap();

    // 再 build 同 plan_id（pending 续跑），如果上次 demote 没释放锁，这里 LockBusy
    let rt2 = PlanRuntime::with_lock_timeout("session-b", 200);
    let outcome = rt2
        .build_plan("lockable", None)
        .expect("demote 后 lock 应已释放，再 build 应成功");
    assert!(matches!(outcome.prev_disk_mode, PlanFileMode::Pending));
    cleanup_home(&home);
}

#[test]
fn finalize_completed_to_chat_clears_first_exec_turn() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("done_path", PlanFileMode::Planning);
    rt.build_plan("done_path", None).unwrap();
    assert!(rt.first_exec_turn_pending_for_test());
    // 模拟 update_plan 派生 completed
    rt.set_mode_completed("done_path".into());
    let pid = rt.finalize_completed_to_chat().expect("Some(plan_id)");
    assert_eq!(pid, "done_path");
    assert!(matches!(rt.mode(), PlanMode::Chat));
    assert!(!rt.first_exec_turn_pending_for_test());
    // 非 Completed 状态调一次 → None
    assert!(rt.finalize_completed_to_chat().is_none());
    cleanup_home(&home);
}

#[test]
fn plan_mode_raw_edit_blocked_for_plan_files_in_planning_and_executing() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("guarded", PlanFileMode::Planning);
    use crate::api::chat::plan_runtime::file_store::*;
    let plan_path = plan_path_for_id("guarded").unwrap();

    // CHAT 模式 → 允许（无 PLAN/EXEC 守卫）
    assert!(matches!(rt.mode(), PlanMode::Chat));
    assert!(rt.allow_raw_edit_to_path(&plan_path));

    // Planning 模式 → 拒
    rt.enter_planning().unwrap();
    assert!(!rt.allow_raw_edit_to_path(&plan_path));

    // Executing 模式 → 拒
    rt.exit_to_chat().unwrap();
    rt.build_plan("guarded", None).unwrap();
    assert!(!rt.allow_raw_edit_to_path(&plan_path));

    // 非 plan 文件 → 始终允许
    let other = home.join(".tomcat").join("notes.md");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::write(&other, "ok").unwrap();
    assert!(rt.allow_raw_edit_to_path(&other));
    cleanup_home(&home);
}

#[test]
fn plan_build_atomic_rollback_on_write_failure() {
    // 制造 write_plan 失败：把 ~/.tomcat/plans 替换为只读普通文件 → write 时 sync_all 或 rename 失败。
    // 平台无关方案：用一个被锁定且超 timeout 的同名 lock 文件触发 LockBusy → write_plan 直接报错。
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let _rt = PlanRuntime::new("session-a");
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
            assert!(
                s.contains("锁") || s.contains("lock") || s.contains("LockBusy"),
                "应是锁/IO 错：{s}"
            )
        }
        other => panic!("expected Io (LockBusy), got {other:?}"),
    }

    // 释放锁，rt 仍可继续 build（证明状态可恢复）
    FileExt::unlock(&_f).unwrap();
    drop(_f);
    let rt = PlanRuntime::with_lock_timeout("session-a", 1000);
    let _ok = rt
        .build_plan("rollback", None)
        .expect("放锁后 build 应成功");
    assert!(matches!(rt.mode(), PlanMode::Executing { .. }));
    cleanup_home(&home);
}

#[test]
fn e8_recover_demotes_orphan_executing_plan_to_pending() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    // 写一个 mode=executing 但 session_key=别人 的 plan 到盘。
    use crate::api::chat::plan_runtime::file_store::*;
    let plan_id = "orphan-plan";
    write_disk_plan(plan_id, PlanFileMode::Executing);
    // 修一下 session_key 让它属于别的 session
    let path = plan_path_for_id(plan_id).unwrap();
    let mut p = read_plan(&path).unwrap();
    p.frontmatter.session_key = Some("session-other".into());
    write_plan(&path, &p, 2000).unwrap();

    let rt = PlanRuntime::new("session-a");
    rt.recover().unwrap();

    // 磁盘应已降级为 pending
    let p2 = read_plan(&path).unwrap();
    assert_eq!(p2.frontmatter.mode, PlanFileMode::Pending);
    // 内存仍 Chat（孤儿不应自动接管）
    assert!(matches!(rt.mode(), PlanMode::Chat));
    cleanup_home(&home);
}

#[test]
fn e8_recover_restores_executing_for_current_session() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    use crate::api::chat::plan_runtime::file_store::*;
    let plan_id = "owned-plan";
    write_disk_plan(plan_id, PlanFileMode::Executing);
    let path = plan_path_for_id(plan_id).unwrap();
    let mut p = read_plan(&path).unwrap();
    p.frontmatter.session_key = Some("session-a".into());
    write_plan(&path, &p, 2000).unwrap();

    let rt = PlanRuntime::new("session-a");
    rt.recover().unwrap();

    // 内存切回 Executing
    match rt.mode() {
        PlanMode::Executing { plan_id: ref pid } => assert_eq!(pid, plan_id),
        other => panic!("expected Executing, got {other:?}"),
    }
    // 首轮 user_meta 应已 armed
    assert!(rt.first_exec_turn_pending_for_test());
    cleanup_home(&home);
}

#[test]
fn e7_reload_active_plan_from_disk_picks_up_session_owned_executing() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    use crate::api::chat::plan_runtime::file_store::*;
    let plan_id = "reload-plan";
    write_disk_plan(plan_id, PlanFileMode::Executing);
    let path = plan_path_for_id(plan_id).unwrap();
    let mut p = read_plan(&path).unwrap();
    p.frontmatter.session_key = Some("session-a".into());
    write_plan(&path, &p, 2000).unwrap();

    // rt 起初是 Chat（模拟 /restore 调用前的状态机）
    let rt = PlanRuntime::new("session-a");
    assert!(matches!(rt.mode(), PlanMode::Chat));

    let restored = rt.reload_active_plan_from_disk().unwrap();
    assert_eq!(restored.as_deref(), Some(plan_id));
    assert!(matches!(rt.mode(), PlanMode::Executing { .. }));
    cleanup_home(&home);
}
