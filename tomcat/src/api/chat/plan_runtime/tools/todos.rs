//! `todos` 工具实现（plan-runtime.md §5.3 / [todos.md] D 方案）。
//!
//! 语义（2026-05 D 方案最终态）：
//! - 任何模式可见；返回完整 items snapshot + applied 计数。
//! - **所有模式（含 EXEC）**：写 session 本地 `Vec<TodoItem>`（`PlanRuntime.session_todos`）
//!   并落盘 session TodoFile（G3 持久化在 `todo_runtime.rs` 接管）。
//! - **绝不**写入 `PlanFile.frontmatter.todos[]`——推进 PlanFile 由 `update_plan` 负责。

use serde::Deserialize;

use crate::api::chat::plan_runtime::{
    file_store::{TodoItem, TodoStatus},
    mode::PlanMode,
    ops,
    todo_runtime::{self, TodoFile},
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

    // D 方案：所有模式（含 EXEC）都走 session TodoFile；PlanFile 推进由 update_plan 负责。
    let _ = &mode;
    session_path(runtime, &todo_ops, mode)
}

fn session_path(
    runtime: &PlanRuntime,
    todo_ops: &[ops::TodoOp],
    mode: PlanMode,
) -> Result<serde_json::Value, ToolError> {
    let mut todos = runtime.snapshot_session_todos();
    ops::apply_todos_ops(&mut todos, todo_ops)?;
    runtime.replace_session_todos(todos.clone());

    // G3：若 ChatContext 已注入 todos_persist_base，落盘到
    // `<base>/sessions/<session_key>/todos/<active_todos_id>.todo.md`。
    // 持久化失败仅日志，不阻塞主流程（D 防御：磁盘异常不影响 in-memory 推进）。
    let mut persisted_path: Option<String> = None;
    let active_todos_id = if runtime.todos_persist_base().is_some() {
        Some(runtime.ensure_active_todos_id())
    } else {
        None
    };
    if let (Some(base), Some(id)) = (runtime.todos_persist_base(), active_todos_id.clone()) {
        let mut file = TodoFile::new(id, runtime.session_key());
        file.items = todos.clone();
        match todo_runtime::persist(&base, &file) {
            Ok(p) => persisted_path = Some(p.display().to_string()),
            Err(e) => tracing::warn!(target: "plan_runtime::todos",
                "持久化 session todos 失败（仅警告，不阻塞）：{e}"),
        }
    }

    let in_progress = todos
        .iter()
        .find(|t| matches!(t.status, TodoStatus::InProgress))
        .map(|t| t.id.clone());
    let mut out = serde_json::json!({
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
    });
    if let Some(id) = active_todos_id {
        out["active_todos_id"] = serde_json::Value::String(id);
    }
    if let Some(p) = persisted_path {
        out["persisted_path"] = serde_json::Value::String(p);
    }

    // E：fanout UI 刷新——session 作用域；items 由本工具维护，milestones 此处恒空
    // （milestones 仅生于 PlanFile，由 update_plan 通知）。
    let snapshot = crate::api::chat::plan_runtime::todos_panel::TodosPanelSnapshot::new_session(
        todos.clone(),
    );
    out["panel_snapshot_id"] =
        serde_json::Value::Number(serde_json::Number::from(snapshot.panel_snapshot_id));
    runtime.refresh_notifier().notify(&snapshot);

    Ok(out)
}

// `exec_path` 已移除：D 方案规定 `todos` 永远不写 PlanFile；EXEC 期推进任务仅走 `update_plan`。
