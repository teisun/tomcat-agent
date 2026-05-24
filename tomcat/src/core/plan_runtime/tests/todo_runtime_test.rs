use super::super::file_store::{TodoItem, TodoStatus};
use super::super::todo_runtime::{
    list_session_todos_files, persist, purge_inactive, TodoFile,
};

fn tmp_base() -> tempfile::TempDir {
    tempfile::TempDir::new().unwrap()
}

#[test]
fn todo_file_roundtrips_markdown_with_status_checkboxes() {
    let mut f = TodoFile::new("td_1", "ses-a", Some("scratch".into()));
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
    let md = f.to_markdown();
    assert!(md.contains("todos_id: td_1"));
    assert!(md.contains("title: scratch"));
    assert!(md.contains("- [~] t1: first"));
    assert!(md.contains("- [x] t2: second"));
}

#[test]
fn persist_writes_atomically_to_expected_path() {
    let dir = tmp_base();
    let f = TodoFile::new("td_2", "ses-b", None);
    let p = persist(dir.path(), &f).unwrap();
    let expected = dir
        .path()
        .join("sessions")
        .join("ses-b")
        .join("todos")
        .join("td_2.todo.md");
    assert_eq!(p, expected);
    assert!(p.exists());
    let body = std::fs::read_to_string(&p).unwrap();
    assert!(body.contains("todos_id: td_2"));
}

#[test]
fn purge_inactive_removes_only_other_ids() {
    let dir = tmp_base();
    persist(dir.path(), &TodoFile::new("a", "s1", None)).unwrap();
    persist(dir.path(), &TodoFile::new("b", "s1", None)).unwrap();
    persist(dir.path(), &TodoFile::new("c", "s1", None)).unwrap();
    let removed = purge_inactive(dir.path(), "s1", "b").unwrap();
    assert_eq!(removed, 2);
    let left = list_session_todos_files(dir.path(), "s1").unwrap();
    assert_eq!(left.len(), 1);
    assert!(left[0].to_string_lossy().ends_with("b.todo.md"));
}

#[test]
fn list_session_todos_files_handles_missing_dir() {
    let dir = tmp_base();
    let out = list_session_todos_files(dir.path(), "absent").unwrap();
    assert!(out.is_empty());
}
