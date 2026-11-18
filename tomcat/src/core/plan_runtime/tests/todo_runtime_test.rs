use super::super::file_store::{TodoItem, TodoStatus};
use super::super::todo_runtime::{todo_path_for, TodosRuntime};
use crate::infra::error::AppError;

#[test]
fn todo_document_v2_roundtrips_body_and_all_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = TodosRuntime::open(dir.path().into(), "ses-a", None);
    assert!(runtime.snapshot().is_none());
    assert!(!runtime.path().exists());
    let body = "\n# 正文\r\n```yaml\n---\nitems: not metadata\n```\n\n尾部  ";
    let before = runtime
        .commit(false, |file| {
            file.title = Some("Title: #quoted\nnext line".into());
            file.draft = body.into();
            file.items.push(TodoItem {
                id: "t1".into(),
                content: "first: #line\nsecond".into(),
                status: TodoStatus::Completed,
                evidence: vec!["task:123".into()],
                kind: Default::default(),
            });
            Ok::<_, AppError>(())
        })
        .unwrap();
    let reopened = TodosRuntime::open(dir.path().into(), "ses-a", None);
    assert_eq!(reopened.snapshot().unwrap(), before);
    assert_eq!(
        reopened.snapshot().unwrap().draft.as_bytes(),
        body.as_bytes()
    );
    assert_eq!(
        reopened.progress().unwrap().0,
        todo_path_for(dir.path(), "ses-a")
    );
    assert_eq!(reopened.snapshot_items(), before.items);
    assert!(!reopened.has_unfinished());
}

#[test]
fn todo_document_protects_invalid_files_and_retries_after_repair() {
    let dir = tempfile::tempdir().unwrap();
    let valid = TodosRuntime::open(dir.path().into(), "ses-a", None);
    valid
        .commit(false, |file| {
            file.draft = "kept".into();
            Ok::<_, AppError>(())
        })
        .unwrap();
    let valid_text = std::fs::read_to_string(valid.path()).unwrap();
    for invalid in [
        "broken".to_string(),
        valid_text.replace("schema_version: 2", "schema_version: 42"),
        valid_text.replace("session_id: ses-a", "session_id: other"),
        valid_text.replace("items: []", "items: invalid"),
    ] {
        std::fs::write(valid.path(), &invalid).unwrap();
        let runtime = TodosRuntime::open(dir.path().into(), "ses-a", None);
        assert!(runtime.progress().is_none());
        for reset in [false, true] {
            let error = runtime
                .commit(reset, |_| Ok::<_, AppError>(()))
                .unwrap_err();
            assert!(error.to_string().contains(runtime.path().to_str().unwrap()));
            assert_eq!(std::fs::read_to_string(runtime.path()).unwrap(), invalid);
        }
        std::fs::write(runtime.path(), &valid_text).unwrap();
        let restored = runtime.commit(false, |_| Ok::<_, AppError>(())).unwrap();
        assert_eq!(restored.draft, "kept");
    }
}

#[test]
fn todo_document_v1_is_not_imported_and_upgrades_only_on_commit() {
    let dir = tempfile::tempdir().unwrap();
    let path = todo_path_for(dir.path(), "ses-a");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let old = "---\nschema_version: 1\nsession_id: ses-a\n---\n\n## Todos\n- [x] old: old item\n";
    std::fs::write(&path, old).unwrap();
    let runtime = TodosRuntime::open(dir.path().into(), "ses-a", None);
    assert!(runtime.snapshot().is_none());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), old);
    runtime
        .commit(false, |file| {
            file.draft = "new".into();
            Ok::<_, AppError>(())
        })
        .unwrap();
    let new = std::fs::read_to_string(path).unwrap();
    assert!(new.contains("schema_version: 2"));
    assert!(!new.contains("old item"));
}

#[test]
fn todo_document_sessions_are_isolated_and_rejection_does_not_commit() {
    let dir = tempfile::tempdir().unwrap();
    let a = TodosRuntime::open(dir.path().into(), "session-a", None);
    let b = TodosRuntime::open(dir.path().into(), "session-b", None);
    a.commit(false, |file| {
        file.draft = "A".into();
        Ok::<_, AppError>(())
    })
    .unwrap();
    b.commit(false, |file| {
        file.draft = "B".into();
        Ok::<_, AppError>(())
    })
    .unwrap();
    let before = a.snapshot().unwrap();
    let text = std::fs::read(a.path()).unwrap();
    a.commit(false, |file| {
        file.draft = "wrong".into();
        Err::<(), _>(AppError::Config("reject".into()))
    })
    .unwrap_err();
    assert_eq!(a.snapshot().unwrap(), before);
    assert_eq!(std::fs::read(a.path()).unwrap(), text);
    assert_eq!(
        TodosRuntime::open(dir.path().into(), "session-b", None)
            .snapshot()
            .unwrap()
            .draft,
        "B"
    );
}
