use serde::Deserialize;

use crate::core::plan_runtime::{
    file_store::{TodoItem, TodoKind, TodoStatus},
    ops,
};

use super::ToolError;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind")]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum SharedTodoOpArg {
    Upsert {
        id: String,
        #[serde(default)]
        content: Option<String>,
        #[serde(default)]
        status: Option<TodoStatus>,
        /// Todo semantic type. The operation discriminator already owns the
        /// `kind` wire key, so this deliberately uses a distinct name.
        #[serde(default)]
        todo_kind: Option<TodoKind>,
    },
    SetStatus {
        id: String,
        /// `content` was historically tolerated but ignored. Keep accepting it for old callers;
        /// the `evidence` wire key aliases to this field and is persisted when it is an array.
        #[serde(default)]
        #[serde(alias = "evidence")]
        content: Option<StatusUpdateMetadata>,
        status: TodoStatus,
    },
    Remove {
        id: String,
        #[serde(default)]
        content: Option<String>,
        #[serde(default)]
        status: Option<TodoStatus>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum StatusUpdateMetadata {
    LegacyContent(String),
    Evidence(Vec<String>),
}

impl StatusUpdateMetadata {
    pub(super) fn evidence(&self) -> Option<&[String]> {
        match self {
            Self::LegacyContent(_) => None,
            Self::Evidence(evidence) => Some(evidence),
        }
    }
}

impl From<String> for StatusUpdateMetadata {
    fn from(value: String) -> Self {
        Self::LegacyContent(value)
    }
}

impl From<Vec<String>> for StatusUpdateMetadata {
    fn from(value: Vec<String>) -> Self {
        Self::Evidence(value)
    }
}

pub fn apply_shared_todo_ops(
    todos: &mut Vec<TodoItem>,
    ops_list: &[SharedTodoOpArg],
    replace: bool,
) -> Result<(), ToolError> {
    if replace {
        let mut rebuilt = Vec::new();
        for op in ops_list {
            match op {
                SharedTodoOpArg::Upsert {
                    id,
                    content,
                    status,
                    todo_kind,
                } => {
                    apply_upsert(&mut rebuilt, id, content.as_ref(), *status, *todo_kind)?;
                }
                SharedTodoOpArg::SetStatus { .. } | SharedTodoOpArg::Remove { .. } => {
                    return Err(ToolError::BadArgs(
                        "replace=true allows only kind=upsert ops".into(),
                    ));
                }
            }
        }
        *todos = rebuilt;
        return Ok(());
    }

    for op in ops_list {
        match op {
            SharedTodoOpArg::Upsert {
                id,
                content,
                status,
                todo_kind,
            } => apply_upsert(todos, id, content.as_ref(), *status, *todo_kind)?,
            SharedTodoOpArg::SetStatus {
                id,
                status,
                content,
                ..
            } => {
                let evidence = content.as_ref().and_then(StatusUpdateMetadata::evidence);
                if evidence.is_some() && *status != TodoStatus::Completed {
                    return Err(ToolError::BadArgs(
                        "evidence may only accompany set_status completed".into(),
                    ));
                }
                ops::apply_todos_ops(
                    todos,
                    &[ops::TodoOp::SetStatus {
                        id: id.clone(),
                        status: *status,
                    }],
                )?;
                if let Some(evidence) = evidence {
                    let todo = todos
                        .iter_mut()
                        .find(|todo| todo.id == id.as_str())
                        .expect("set_status already verified todo exists");
                    for item in evidence {
                        if !item.trim().is_empty() && !todo.evidence.contains(item) {
                            todo.evidence.push(item.clone());
                        }
                    }
                }
            }
            SharedTodoOpArg::Remove { id, .. } => {
                ops::apply_todos_ops(todos, &[ops::TodoOp::RemoveTodo { id: id.clone() }])?;
            }
        }
    }
    Ok(())
}

fn apply_upsert(
    todos: &mut Vec<TodoItem>,
    id: &str,
    content: Option<&String>,
    status: Option<TodoStatus>,
    todo_kind: Option<TodoKind>,
) -> Result<(), ToolError> {
    if let Some(existing) = todos.iter_mut().find(|todo| todo.id == id) {
        if let Some(content) = content {
            existing.content = content.clone();
        }
        if let Some(status) = status {
            existing.status = status;
        }
        if let Some(todo_kind) = todo_kind {
            existing.kind = todo_kind;
        }
        return Ok(());
    }

    let Some(content) = content else {
        return Err(ToolError::BadArgs(format!(
            "upsert requires content when creating todo `{id}`"
        )));
    };
    ops::apply_todos_ops(
        todos,
        &[ops::TodoOp::AddTodo(TodoItem {
            id: id.to_string(),
            content: content.clone(),
            status: status.unwrap_or(TodoStatus::Pending),
            evidence: Vec::new(),
            kind: todo_kind.unwrap_or(TodoKind::Work),
        })],
    )?;
    Ok(())
}

pub fn items_json(todos: &[TodoItem]) -> Vec<serde_json::Value> {
    todos
        .iter()
        .map(|t| {
            serde_json::json!({
                "id": t.id,
                "content": t.content,
                "status": t.status.as_str(),
                "evidence": t.evidence,
                "kind": t.kind.as_str(),
            })
        })
        .collect()
}
