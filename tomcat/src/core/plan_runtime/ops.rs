//! # `apply_todos_op` 共享引擎（plan-runtime.md §R2 D 方案）
//!
//! `todos`（session scratchpad）与 `update_plan`（PlanFile.frontmatter.todos[]）共享同一
//! op 引擎：增/改/删/标状态。语义不变量：
//!
//! - 同一列表最多一个 `in_progress`；尝试设第二个 → `OpError::MultipleInProgress`。
//! - `id` 单列表内唯一；`AddTodo` 重复 id → `OpError::DuplicateId`。
//! - `RemoveTodo` 不存在 id → `OpError::TodoNotFound`。
//! - 操作按入参顺序串行；任一失败立即返回，已应用的 op 不回滚（调用方负责拷贝原始
//!   `Vec<TodoItem>` 后再 apply，失败时丢弃即可）。
//!
//! op 与 [`file_store::TodoItem`] / [`file_store::TodoStatus`] 解耦，便于复用到
//! session todo 文件（`~/.tomcat/agents/.../todos/*.todo.md`）。

use super::file_store::{TodoItem, TodoStatus};

/// 增量操作。`update_plan` 把 OpenAI tool args 直接反序列化到本枚举。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoOp {
    /// 新增一条 todo（id 必须唯一）。
    AddTodo(TodoItem),
    /// 改状态。
    SetStatus { id: String, status: TodoStatus },
    /// 改正文。
    SetContent { id: String, content: String },
    /// 删除 todo。
    RemoveTodo { id: String },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OpError {
    #[error("todo 不存在: {0}")]
    TodoNotFound(String),
    #[error("todo id 已存在: {0}")]
    DuplicateId(String),
    #[error("最多允许一个 in_progress，本次操作会产生 {count} 个")]
    MultipleInProgress { count: usize },
}

/// 在 `todos` 上顺序 apply ops；任一失败立即返回，**不**回滚。
///
/// 调用方语义：
/// - `todos`：可传 `&mut Vec<TodoItem>`（session scratchpad）
/// - `update_plan`：可传 `&mut PlanFileFrontmatter.todos`
pub fn apply_todos_ops(todos: &mut Vec<TodoItem>, ops: &[TodoOp]) -> Result<(), OpError> {
    for op in ops {
        apply_one(todos, op)?;
    }
    enforce_single_in_progress(todos)?;
    Ok(())
}

fn apply_one(todos: &mut Vec<TodoItem>, op: &TodoOp) -> Result<(), OpError> {
    match op {
        TodoOp::AddTodo(item) => {
            if todos.iter().any(|t| t.id == item.id) {
                return Err(OpError::DuplicateId(item.id.clone()));
            }
            todos.push(item.clone());
            Ok(())
        }
        TodoOp::SetStatus { id, status } => {
            let t = todos
                .iter_mut()
                .find(|t| &t.id == id)
                .ok_or_else(|| OpError::TodoNotFound(id.clone()))?;
            t.status = *status;
            Ok(())
        }
        TodoOp::SetContent { id, content } => {
            let t = todos
                .iter_mut()
                .find(|t| &t.id == id)
                .ok_or_else(|| OpError::TodoNotFound(id.clone()))?;
            t.content = content.clone();
            Ok(())
        }
        TodoOp::RemoveTodo { id } => {
            let before = todos.len();
            todos.retain(|t| &t.id != id);
            if todos.len() == before {
                return Err(OpError::TodoNotFound(id.clone()));
            }
            Ok(())
        }
    }
}

/// 收尾不变量：在 apply 完所有 op 后强制单 in_progress。
fn enforce_single_in_progress(todos: &[TodoItem]) -> Result<(), OpError> {
    let count = todos
        .iter()
        .filter(|t| matches!(t.status, TodoStatus::InProgress))
        .count();
    if count > 1 {
        return Err(OpError::MultipleInProgress { count });
    }
    Ok(())
}

/// 派生：所有 todo 都 completed → true；否则 false。空列表返回 false（`completed` 模式
/// 需要至少完成一件事才转入，避免空 plan 误判已完成）。
pub fn all_completed(todos: &[TodoItem]) -> bool {
    !todos.is_empty()
        && todos
            .iter()
            .all(|t| matches!(t.status, TodoStatus::Completed))
}

