use super::common::*;

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
    assert_eq!(rt.snapshot_session_todos().len(), 2);
    cleanup_home(&home);
}

#[test]
fn todos_persists_to_disk_when_persist_base_configured() {
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
    let parsed = read_plan(&path).unwrap();
    assert_eq!(parsed.frontmatter.todos[0].status, TodoStatus::Pending);
    cleanup_home(&home);
}
