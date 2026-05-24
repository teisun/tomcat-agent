//! `todos` 工具实现（plan-runtime.md §5.3 / [todos.md] D 方案）。
//!
//! 语义（2026-05 D 方案最终态）：
//! - 任何模式可见；返回完整 items snapshot + applied 计数。
//! - **所有模式（含 EXEC）**：写 session 本地 `Vec<TodoItem>`（`PlanRuntime.session_todos`）
//!   并落盘 session TodoFile（G3 持久化在 `todo_runtime.rs` 接管）。
//! - **绝不**写入 `PlanFile.frontmatter.todos[]`——推进 PlanFile 由 `update_plan` 负责。

use serde::Deserialize;

use crate::core::plan_runtime::{
    file_store::TodoStatus,
    mode::PlanMode,
    todo_runtime::{self, TodoFile},
    PlanRuntime,
};

use super::shared_todo_ops::{apply_shared_todo_ops, items_json};
use super::ToolError;

#[derive(Debug, Deserialize)]
pub struct TodosArgs {
    #[serde(default)]
    pub new_todos: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub replace: bool,
    pub ops: Vec<TodoOpArg>,
}

pub use super::shared_todo_ops::SharedTodoOpArg as TodoOpArg;

impl TodosArgs {
    pub fn from_json(raw: &serde_json::Value) -> Result<Self, ToolError> {
        if let Some(ops) = raw.get("ops").and_then(|v| v.as_array()) {
            for op in ops {
                if op.get("op").is_some() && op.get("kind").is_none() {
                    return Err(ToolError::BadArgs(
                        "todos ops: 字段 `op` 已下线，请改用 `kind`（kind: upsert | set_status | remove）"
                            .into(),
                    ));
                }
            }
        }
        serde_json::from_value(raw.clone())
            .map_err(|e| ToolError::BadArgs(format!("todos args: {e}")))
    }
}

pub fn execute(runtime: &PlanRuntime, args: TodosArgs) -> Result<serde_json::Value, ToolError> {
    let mode = runtime.mode();
    // D 方案：所有模式（含 EXEC）都走 session TodoFile；PlanFile 推进由 update_plan 负责。
    let _ = &mode;
    session_path(runtime, args, mode)
}

fn session_path(
    runtime: &PlanRuntime,
    args: TodosArgs,
    mode: PlanMode,
) -> Result<serde_json::Value, ToolError> {
    let mut todos = if args.new_todos {
        Vec::new()
    } else {
        runtime.snapshot_session_todos()
    };
    apply_shared_todo_ops(&mut todos, &args.ops, args.replace)?;
    runtime.replace_session_todos(todos.clone());

    // G3：若 ChatContext 已注入 todos_persist_base，落盘到
    // `<base>/sessions/<session_key>/todos/<active_todos_id>.todo.md`。
    // 持久化失败仅日志，不阻塞主流程（D 防御：磁盘异常不影响 in-memory 推进）。
    let mut persisted_path: Option<String> = None;
    let active_todos_id = if runtime.todos_persist_base().is_some() {
        Some(if args.new_todos {
            runtime.rotate_active_todos_id()
        } else {
            runtime.ensure_active_todos_id()
        })
    } else {
        None
    };
    if let (Some(base), Some(id)) = (runtime.todos_persist_base(), active_todos_id.clone()) {
        let mut file = TodoFile::new(id.clone(), runtime.session_key(), args.title.clone());
        file.items = todos.clone();
        match todo_runtime::persist(&base, &file) {
            Ok(p) => {
                persisted_path = Some(p.display().to_string());
                if args.new_todos {
                    if let Err(e) = todo_runtime::purge_inactive(&base, runtime.session_key(), &id)
                    {
                        tracing::warn!(target: "plan_runtime::todos",
                            "清理旧 todos 文件失败（仅警告，不阻塞）：{e}");
                    }
                }
            }
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
        "applied": args.ops.len(),
        "replace": args.replace,
        "new_todos": args.new_todos,
        "active_in_progress": in_progress,
        "items": items_json(&todos),
    });
    if let Some(title) = args.title {
        out["title"] = serde_json::Value::String(title);
    }
    if let Some(id) = active_todos_id {
        out["active_todos_id"] = serde_json::Value::String(id);
    }
    if let Some(p) = persisted_path {
        out["persisted_path"] = serde_json::Value::String(p);
    }

    // E：fanout UI 刷新——session 作用域；session todos 只携带纯 todo snapshot。
    let snapshot =
        crate::core::plan_runtime::panels::TodosPanelSnapshot::new_session(todos.clone());
    out["panel_snapshot_id"] =
        serde_json::Value::Number(serde_json::Number::from(snapshot.panel_snapshot_id));
    runtime.refresh_notifier().notify(&snapshot);

    Ok(out)
}
