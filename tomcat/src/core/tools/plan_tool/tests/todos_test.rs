use crate::core::plan_runtime::todo_runtime::TodosRuntime;
use crate::core::tools::plan_tool::{todos, ToolError};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::Arc;

fn call(runtime: &TodosRuntime, args: Value) -> Result<Value, ToolError> {
    todos::execute(runtime, todos::TodosArgs::from_json(&args)?)
}

#[test]
fn todos_body_title_identity_replace_and_reset() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = TodosRuntime::open(dir.path().into(), "session-a", None);
    let initial = call(
        &runtime,
        json!({"title":"work", "draft":"# body\n", "ops":[
            {"kind":"upsert","id":"a","content":"A"},
            {"kind":"upsert","id":"b","content":"B"}
        ]}),
    )
    .unwrap();
    let before = runtime.snapshot().unwrap();
    assert_eq!(initial["todos_path"], runtime.path().to_str().unwrap());
    for absent in ["draft", "persisted_path", "mode", "panel_snapshot_id"] {
        assert!(initial.get(absent).is_none());
    }
    let changed = call(
        &runtime,
        json!({"ops":[{"kind":"set_status","id":"a","status":"in_progress"}]}),
    )
    .unwrap();
    assert_eq!(changed["items"].as_array().unwrap().len(), 2);
    assert_eq!(changed["title"], "work");
    assert_eq!(runtime.snapshot().unwrap().draft, before.draft);
    assert_eq!(runtime.snapshot().unwrap().created_at, before.created_at);
    call(&runtime, json!({"draft":"new body"})).unwrap();
    call(
        &runtime,
        json!({"replace":true,"ops":[{"kind":"upsert","id":"c","content":"C"}]}),
    )
    .unwrap();
    let replaced = runtime.snapshot().unwrap();
    assert_eq!(replaced.todos_id, before.todos_id);
    assert_eq!(replaced.created_at, before.created_at);
    assert_eq!(replaced.title, before.title);
    assert_eq!(replaced.draft, "new body");
    assert_eq!(replaced.items.len(), 1);
    call(&runtime, json!({"draft":"", "title":""})).unwrap();
    assert!(runtime.snapshot().unwrap().draft.is_empty());
    assert!(runtime.snapshot().unwrap().title.is_none());
    let reset = call(&runtime, json!({"new_todos":true})).unwrap();
    assert_ne!(reset["active_todos_id"], initial["active_todos_id"]);
    assert_eq!(reset["todos_path"], initial["todos_path"]);
    assert!(runtime.snapshot().unwrap().items.is_empty());
}

#[test]
fn todos_completion_evidence_roundtrips_and_replace_discards_it() {
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let runtime = TodosRuntime::open(
        dir.path().into(),
        "session-a",
        Some(Arc::new(move |items| sink.lock().push(items.to_vec()))),
    );
    call(
        &runtime,
        json!({"draft":"kept", "ops":[{"kind":"upsert","id":"a","content":"A"}]}),
    )
    .unwrap();
    for proofs in [
        json!(["file:a", " "]),
        json!(["file:a", "task:1"]),
        json!([]),
    ] {
        let out = call(
            &runtime,
            json!({"ops":[{"kind":"set_status","id":"a","status":"completed","evidence":proofs}]}),
        )
        .unwrap();
        assert_eq!(out["todos_path"], runtime.path().to_str().unwrap());
    }
    call(&runtime, json!({"draft":"updated"})).unwrap();
    let before = runtime.snapshot().unwrap();
    assert_eq!(before.items[0].evidence, ["file:a", "task:1"]);
    assert_eq!(events.lock().last().unwrap(), &before.items);
    let restored = TodosRuntime::open(dir.path().into(), "session-a", None);
    assert_eq!(restored.snapshot().unwrap(), before);
    call(&restored, json!({"title":"restored"})).unwrap();
    assert_eq!(restored.snapshot().unwrap().items, before.items);
    call(
        &restored,
        json!({"replace":true,"ops":[{"kind":"upsert","id":"a","content":"A"}]}),
    )
    .unwrap();
    assert!(restored.snapshot().unwrap().items[0].evidence.is_empty());
}

#[test]
fn todos_invalid_batch_preserves_memory_disk_and_notifications() {
    let dir = tempfile::tempdir().unwrap();
    let notifications = Arc::new(Mutex::new(0));
    let count = notifications.clone();
    let runtime = TodosRuntime::open(
        dir.path().into(),
        "session-a",
        Some(Arc::new(move |_| *count.lock() += 1)),
    );
    call(
        &runtime,
        json!({"draft":"kept","ops":[{"kind":"upsert","id":"a","content":"A"}]}),
    )
    .unwrap();
    let before = runtime.snapshot().unwrap();
    let bytes = std::fs::read(runtime.path()).unwrap();
    for invalid in [
        json!({}),
        json!({"ops":[]}),
        json!({"replace":true}),
        json!({"replace":true,"draft":"wrong"}),
        json!({"replace":true,"ops":[{"kind":"remove","id":"a"}]}),
        json!({"draft":"wrong","ops":[{"kind":"upsert","id":"b","content":"B"},{"kind":"set_status","id":"missing","status":"completed"}]}),
        json!({"ops":[{"kind":"set_status","id":"a","status":"in_progress","evidence":["bad"]}]}),
    ] {
        assert!(call(&runtime, invalid).is_err());
        assert_eq!(runtime.snapshot().unwrap(), before);
        assert_eq!(std::fs::read(runtime.path()).unwrap(), bytes);
        assert_eq!(*notifications.lock(), 1);
    }
    // Rename cannot replace a directory. No successful state or notification is published.
    std::fs::remove_file(runtime.path()).unwrap();
    std::fs::create_dir(runtime.path()).unwrap();
    assert!(call(&runtime, json!({"new_todos":true,"draft":"not committed"})).is_err());
    assert_eq!(runtime.snapshot().unwrap(), before);
    assert!(runtime.path().is_dir());
    assert_eq!(*notifications.lock(), 1);
}

#[test]
fn todos_publisher_runs_outside_document_lock() {
    let dir = tempfile::tempdir().unwrap();
    let slot: Arc<Mutex<Option<std::sync::Weak<TodosRuntime>>>> = Arc::new(Mutex::new(None));
    let callback_slot = slot.clone();
    let runtime = Arc::new(TodosRuntime::open(
        dir.path().into(),
        "session-a",
        Some(Arc::new(move |items| {
            let runtime = callback_slot.lock().as_ref().unwrap().upgrade().unwrap();
            assert_eq!(runtime.snapshot_items(), items);
        })),
    ));
    *slot.lock() = Some(Arc::downgrade(&runtime));
    call(&runtime, json!({"draft":"safe to query"})).unwrap();
}
