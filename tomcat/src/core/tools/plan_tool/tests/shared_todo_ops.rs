use super::common::*;

#[test]
fn from_json_helpers_reject_bad_args() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
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

#[test]
fn shared_todo_ops_replace_requires_upsert_only() {
    let mut todos = vec![TodoItem {
        id: "t1".into(),
        content: "old".into(),
        status: TodoStatus::Pending,
    }];
    let err = shared_todo_ops::apply_shared_todo_ops(
        &mut todos,
        &[shared_todo_ops::SharedTodoOpArg::SetStatus {
            id: "t1".into(),
            content: None,
            status: TodoStatus::Completed,
        }],
        true,
    )
    .expect_err("replace=true 仅允许 upsert");
    matches!(err, ToolError::BadArgs(_));
}

#[test]
fn shared_todo_ops_upsert_can_insert_and_update() {
    let mut todos = Vec::new();
    shared_todo_ops::apply_shared_todo_ops(
        &mut todos,
        &[shared_todo_ops::SharedTodoOpArg::Upsert {
            id: "t1".into(),
            content: Some("first".into()),
            status: Some(TodoStatus::Pending),
        }],
        false,
    )
    .unwrap();
    shared_todo_ops::apply_shared_todo_ops(
        &mut todos,
        &[shared_todo_ops::SharedTodoOpArg::Upsert {
            id: "t1".into(),
            content: Some("updated".into()),
            status: Some(TodoStatus::Completed),
        }],
        false,
    )
    .unwrap();
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0].content, "updated");
    assert_eq!(todos[0].status, TodoStatus::Completed);
}
