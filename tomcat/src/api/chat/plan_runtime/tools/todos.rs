//! `todos` 工具实现（plan-runtime.md §5.3 / [todos.md]）。
//!
//! 语义：
//! - 任何模式可见；返回完整 items snapshot + applied 计数。
//! - **CHAT / Planning / Pending / Completed**：写 session 本地 `Vec<TodoItem>`
//!   （`PlanRuntime.session_todos`，**不**落盘 PlanFile）。
//! - **EXEC**：写 active PlanFile.frontmatter.todos[]（与 update_plan 共享 file_store lock）。

use serde::Deserialize;

use crate::api::chat::plan_runtime::{
    file_store::{plan_path_for_id, read_plan, write_plan, TodoItem, TodoStatus},
    mode::PlanMode,
    ops,
    PlanRuntime,
};

use super::ToolError;

#[derive(Debug, Deserialize)]
pub struct TodosArgs {
    pub ops: Vec<TodoOpArg>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op")]
#[serde(rename_all = "snake_case")]
pub enum TodoOpArg {
    AddTodo {
        id: String,
        content: String,
        #[serde(default = "default_pending")]
        status: TodoStatus,
        #[serde(default)]
        milestone_id: Option<String>,
    },
    SetStatus {
        id: String,
        status: TodoStatus,
    },
    SetContent {
        id: String,
        content: String,
    },
    RemoveTodo {
        id: String,
    },
}

fn default_pending() -> TodoStatus {
    TodoStatus::Pending
}

impl TodosArgs {
    pub fn from_json(raw: &serde_json::Value) -> Result<Self, ToolError> {
        serde_json::from_value(raw.clone())
            .map_err(|e| ToolError::BadArgs(format!("todos args: {e}")))
    }
}

pub fn execute(
    runtime: &PlanRuntime,
    args: TodosArgs,
) -> Result<serde_json::Value, ToolError> {
    let mode = runtime.mode();
    let todo_ops: Vec<ops::TodoOp> = args
        .ops
        .into_iter()
        .map(|o| match o {
            TodoOpArg::AddTodo {
                id,
                content,
                status,
                milestone_id,
            } => ops::TodoOp::AddTodo(TodoItem {
                id,
                content,
                status,
                milestone_id,
            }),
            TodoOpArg::SetStatus { id, status } => ops::TodoOp::SetStatus { id, status },
            TodoOpArg::SetContent { id, content } => ops::TodoOp::SetContent { id, content },
            TodoOpArg::RemoveTodo { id } => ops::TodoOp::RemoveTodo { id },
        })
        .collect();

    match &mode {
        PlanMode::Executing { plan_id } => exec_path(runtime, plan_id, &todo_ops),
        _ => session_path(runtime, &todo_ops, mode),
    }
}

fn session_path(
    runtime: &PlanRuntime,
    todo_ops: &[ops::TodoOp],
    mode: PlanMode,
) -> Result<serde_json::Value, ToolError> {
    let mut todos = runtime.snapshot_session_todos();
    ops::apply_todos_ops(&mut todos, todo_ops)?;
    runtime.replace_session_todos(todos.clone());
    let in_progress = todos
        .iter()
        .find(|t| matches!(t.status, TodoStatus::InProgress))
        .map(|t| t.id.clone());
    Ok(serde_json::json!({
        "scope": "session",
        "mode": mode.as_str(),
        "applied": todo_ops.len(),
        "active_in_progress": in_progress,
        "items": todos
            .iter()
            .map(|t| serde_json::json!({
                "id": t.id,
                "content": t.content,
                "status": t.status.as_str(),
                "milestone_id": t.milestone_id,
            }))
            .collect::<Vec<_>>(),
    }))
}

fn exec_path(
    runtime: &PlanRuntime,
    plan_id: &str,
    todo_ops: &[ops::TodoOp],
) -> Result<serde_json::Value, ToolError> {
    let path = plan_path_for_id(plan_id)?;
    let mut plan = read_plan(&path)?;
    ops::apply_todos_ops(&mut plan.frontmatter.todos, todo_ops)?;
    let auto_complete = ops::all_completed(&plan.frontmatter.todos);
    if auto_complete {
        plan.frontmatter.mode = crate::api::chat::plan_runtime::file_store::PlanFileMode::Completed;
    }
    write_plan(&path, &plan, runtime.lock_timeout_ms())?;
    if auto_complete {
        runtime.set_mode_completed(plan_id.to_string());
    }
    let in_progress = plan
        .frontmatter
        .todos
        .iter()
        .find(|t| matches!(t.status, TodoStatus::InProgress))
        .map(|t| t.id.clone());
    Ok(serde_json::json!({
        "scope": "plan",
        "mode": plan.frontmatter.mode.as_str(),
        "plan_id": plan_id,
        "applied": todo_ops.len(),
        "active_in_progress": in_progress,
        "items": plan
            .frontmatter
            .todos
            .iter()
            .map(|t| serde_json::json!({
                "id": t.id,
                "content": t.content,
                "status": t.status.as_str(),
                "milestone_id": t.milestone_id,
            }))
            .collect::<Vec<_>>(),
    }))
}
