//! Session-local Todos document tool; independent of PlanRuntime and formal Plan todos.

use serde::Deserialize;

use crate::core::plan_runtime::{file_store::TodoStatus, todo_runtime::TodosRuntime};

use super::shared_todo_ops::{apply_shared_todo_ops, items_json};
use super::ToolError;

#[derive(Debug, Deserialize)]
pub struct TodosArgs {
    #[serde(default)]
    pub new_todos: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub draft: Option<String>,
    #[serde(default)]
    pub replace: bool,
    #[serde(default)]
    pub ops: Vec<TodoOpArg>,
}

pub use super::shared_todo_ops::SharedTodoOpArg as TodoOpArg;

impl TodosArgs {
    pub fn from_json(raw: &serde_json::Value) -> Result<Self, ToolError> {
        if let Some(ops) = raw.get("ops").and_then(|v| v.as_array()) {
            for op in ops {
                if op.get("op").is_some() && op.get("kind").is_none() {
                    return Err(ToolError::BadArgs(
                        "todos ops: field `op` was removed; use `kind` (kind: upsert | set_status | remove)".into(),
                    ));
                }
            }
        }
        serde_json::from_value(raw.clone())
            .map_err(|e| ToolError::BadArgs(format!("todos args: {e}")))
    }
}

pub fn execute(runtime: &TodosRuntime, args: TodosArgs) -> Result<serde_json::Value, ToolError> {
    if args.replace && args.ops.is_empty() {
        return Err(ToolError::BadArgs(
            "replace=true requires non-empty upsert ops".into(),
        ));
    }
    if args.ops.is_empty() && args.draft.is_none() && args.title.is_none() && !args.new_todos {
        return Err(ToolError::BadArgs(
            "provide draft, title, ops or new_todos=true".into(),
        ));
    }
    let file = runtime.commit(args.new_todos, |file| {
        if let Some(title) = &args.title {
            file.title = (!title.is_empty()).then(|| title.clone());
        }
        if let Some(draft) = &args.draft {
            file.draft.clone_from(draft);
        }
        apply_shared_todo_ops(&mut file.items, &args.ops, args.replace)
    })?;
    let in_progress = file
        .items
        .iter()
        .find(|item| item.status == TodoStatus::InProgress)
        .map(|item| &item.id);
    let mut out = serde_json::json!({
        "scope": "session",
        "applied": args.ops.len(),
        "replace": args.replace,
        "new_todos": args.new_todos,
        "active_in_progress": in_progress,
        "active_todos_id": file.todos_id,
        "items": items_json(&file.items),
        "todos_path": runtime.path().to_string_lossy(),
    });
    if let Some(title) = file.title {
        out["title"] = title.into();
    }
    Ok(out)
}
