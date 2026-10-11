//! Completion notification must not overwrite a later accepted plan update.
#![allow(clippy::await_holding_lock)]

use super::*;
use crate::core::tools::plan_tool::tests::common::{
    cleanup_home, fresh_planning_plan, home_lock, mark_plan_executing, plan_path_for_id, read_plan,
    setup_isolated_home,
};

#[tokio::test]
async fn completion_publication_preserves_an_intervening_accepted_update() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let runtime = PlanRuntime::new("session-a");
    let id = fresh_planning_plan(&runtime);
    mark_plan_executing(&runtime, &id, "session-a");
    let path = plan_path_for_id(&id).unwrap();

    // A commits its terminal todos and derived state in the same transaction.
    update_plan_locked(&path, 2000, |plan| -> Result<_, ToolError> {
        for todo in &mut plan.frontmatter.todos {
            todo.status = TodoStatus::Completed;
        }
        plan.frontmatter.state = PlanFileState::Completed;
        rewrite_todos_board(&mut plan.body, &plan.frontmatter.todos);
        Ok(())
    })
    .unwrap();

    // B enters the real tool after A released the file lock. Its success must
    // remain durable even when A's completion notification is late.
    let accepted = execute(
        &runtime,
        UpdatePlanArgs {
            plan_id: Some(id.clone()),
            path: None,
            replace: false,
            ops: vec![UpdateOp::Upsert {
                id: "new-work".into(),
                content: Some("accepted after the earlier snapshot".into()),
                status: Some(TodoStatus::Pending),
                todo_kind: None,
            }],
        },
    )
    .await
    .unwrap();
    assert_eq!(accepted["items"][2]["id"], "new-work");
    assert_eq!(accepted["plan_state_after"], "pending");
    let committed = std::fs::read(&path).unwrap();

    emit_plan_completed(&runtime, &id, &path);
    let after_notification = std::fs::read(&path).unwrap();
    let final_plan = read_plan(&path).unwrap();
    cleanup_home(&home);

    assert_eq!(
        after_notification, committed,
        "completion publication must not rewrite an obsolete plan snapshot"
    );
    assert!(final_plan
        .frontmatter
        .todos
        .iter()
        .any(|todo| todo.id == "new-work"));
}
