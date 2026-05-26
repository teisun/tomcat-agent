use super::super::file_store::{TodoItem, TodoStatus};
use super::super::ops::{all_completed, apply_todos_ops, OpError, TodoOp};

fn td(id: &str, status: TodoStatus) -> TodoItem {
    TodoItem {
        id: id.into(),
        content: id.into(),
        status,
    }
}

#[test]
fn add_and_set_status_ok() {
    let mut v = vec![td("a", TodoStatus::Pending)];
    apply_todos_ops(
        &mut v,
        &[
            TodoOp::AddTodo(td("b", TodoStatus::Pending)),
            TodoOp::SetStatus {
                id: "a".into(),
                status: TodoStatus::InProgress,
            },
        ],
    )
    .unwrap();
    assert_eq!(v.len(), 2);
    assert_eq!(v[0].status, TodoStatus::InProgress);
    assert_eq!(v[1].status, TodoStatus::Pending);
}

#[test]
fn duplicate_id_returns_err() {
    let mut v = vec![td("a", TodoStatus::Pending)];
    let err =
        apply_todos_ops(&mut v, &[TodoOp::AddTodo(td("a", TodoStatus::Pending))]).expect_err("dup");
    assert_eq!(err, OpError::DuplicateId("a".into()));
}

#[test]
fn remove_nonexistent_returns_err() {
    let mut v: Vec<TodoItem> = vec![];
    let err =
        apply_todos_ops(&mut v, &[TodoOp::RemoveTodo { id: "x".into() }]).expect_err("not found");
    assert_eq!(err, OpError::TodoNotFound("x".into()));
}

#[test]
fn set_status_nonexistent_returns_err() {
    let mut v = vec![td("a", TodoStatus::Pending)];
    let err = apply_todos_ops(
        &mut v,
        &[TodoOp::SetStatus {
            id: "ghost".into(),
            status: TodoStatus::Completed,
        }],
    )
    .expect_err("not found");
    assert_eq!(err, OpError::TodoNotFound("ghost".into()));
}

#[test]
fn enforces_single_in_progress_after_batch() {
    let mut v = vec![td("a", TodoStatus::Pending), td("b", TodoStatus::Pending)];
    let err = apply_todos_ops(
        &mut v,
        &[
            TodoOp::SetStatus {
                id: "a".into(),
                status: TodoStatus::InProgress,
            },
            TodoOp::SetStatus {
                id: "b".into(),
                status: TodoStatus::InProgress,
            },
        ],
    )
    .expect_err("two in_progress");
    assert_eq!(err, OpError::MultipleInProgress { count: 2 });
}

#[test]
fn set_content_ok() {
    let mut v = vec![td("a", TodoStatus::Pending)];
    apply_todos_ops(
        &mut v,
        &[TodoOp::SetContent {
            id: "a".into(),
            content: "new".into(),
        }],
    )
    .unwrap();
    assert_eq!(v[0].content, "new");
}

#[test]
fn all_completed_empty_returns_false() {
    assert!(!all_completed(&[]));
}

#[test]
fn all_completed_mixed_returns_false() {
    let v = vec![td("a", TodoStatus::Completed), td("b", TodoStatus::Pending)];
    assert!(!all_completed(&v));
}

#[test]
fn all_completed_all_completed_returns_true() {
    let v = vec![
        td("a", TodoStatus::Completed),
        td("b", TodoStatus::Completed),
    ];
    assert!(all_completed(&v));
}

#[test]
fn cancelled_does_not_block_in_progress() {
    let mut v = vec![td("a", TodoStatus::Cancelled), td("b", TodoStatus::Pending)];
    apply_todos_ops(
        &mut v,
        &[TodoOp::SetStatus {
            id: "b".into(),
            status: TodoStatus::InProgress,
        }],
    )
    .unwrap();
}
