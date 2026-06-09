use super::super::file_store::{TodoItem, TodoStatus};
use super::super::todo_runtime::{todo_path_for, TodoFile, TodosRuntime};

fn tmp_base() -> tempfile::TempDir {
    tempfile::TempDir::new().unwrap()
}

#[test]
fn todo_file_roundtrips_markdown_with_status_checkboxes() {
    let dir = tmp_base();
    let mut f = TodoFile::new("td_1", Some("scratch".into()));
    f.items.push(TodoItem {
        id: "t1".into(),
        content: "first".into(),
        status: TodoStatus::InProgress,
    });
    f.items.push(TodoItem {
        id: "t2".into(),
        content: "second".into(),
        status: TodoStatus::Completed,
    });
    let runtime = TodosRuntime::new(dir.path().to_path_buf(), "ses-a");
    let p = runtime.persist(&f).unwrap();
    let md = std::fs::read_to_string(&p).unwrap();
    assert!(md.contains("todos_id: td_1"));
    assert!(md.contains("session_id: ses-a"));
    assert!(md.contains("title: scratch"));
    assert!(md.contains("- [~] t1: first"));
    assert!(md.contains("- [x] t2: second"));
}

#[test]
fn persist_writes_atomically_to_expected_path() {
    let dir = tmp_base();
    let runtime = TodosRuntime::new(dir.path().to_path_buf(), "ses-b");
    let f = TodoFile::new("td_2", None);
    let p = runtime.persist(&f).unwrap();
    let expected = dir.path().join("todos").join("ses-b.todo.md");
    assert_eq!(p, expected);
    assert!(p.exists());
    let body = std::fs::read_to_string(&p).unwrap();
    assert!(body.contains("todos_id: td_2"));
    assert!(body.contains("session_id: ses-b"));
}

#[test]
fn todo_path_for_uses_agent_level_single_file_layout() {
    let dir = tmp_base();
    let path = todo_path_for(dir.path(), "session-123");
    assert_eq!(path, dir.path().join("todos").join("session-123.todo.md"));
}

#[test]
fn todos_runtime_isolates_multiple_sessions_without_purge() {
    let dir = tmp_base();
    let runtime_a = TodosRuntime::new(dir.path().to_path_buf(), "session-a");
    let runtime_b = TodosRuntime::new(dir.path().to_path_buf(), "session-b");

    let mut file_a = TodoFile::new("td_a", None);
    file_a.items.push(TodoItem {
        id: "a1".into(),
        content: "from a".into(),
        status: TodoStatus::Pending,
    });
    let mut file_b = TodoFile::new("td_b", None);
    file_b.items.push(TodoItem {
        id: "b1".into(),
        content: "from b".into(),
        status: TodoStatus::Completed,
    });

    let path_a = runtime_a.persist(&file_a).unwrap();
    let path_b = runtime_b.persist(&file_b).unwrap();

    assert_eq!(path_a, dir.path().join("todos").join("session-a.todo.md"));
    assert_eq!(path_b, dir.path().join("todos").join("session-b.todo.md"));
    assert!(path_a.exists());
    assert!(path_b.exists());
    let body_a = std::fs::read_to_string(&path_a).unwrap();
    let body_b = std::fs::read_to_string(&path_b).unwrap();
    assert!(body_a.contains("a1: from a"));
    assert!(!body_a.contains("b1: from b"));
    assert!(body_b.contains("b1: from b"));
    assert!(!body_b.contains("a1: from a"));
}
