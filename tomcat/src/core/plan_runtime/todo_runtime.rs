//! `TodosRuntime` — per-session todos 持久化（GAP-N12 / G3）。
//!
//! 设计口径（plan-runtime.md §G3）：
//!
//! - 每个 chat session 持有一份**当前 active TodoFile**，落盘到
//!   `~/.tomcat/agents/<id>/todos/<session_id>.todo.md`；
//! - `TodosRuntime` 不管理多文件历史；同一 session 始终覆盖写这一份文件。
//! - 落盘格式：YAML frontmatter（id / session_id / created_at）+ markdown body
//!   （`## Todos\n- [ ] id: content` 列表），与 `PlanFile` schema 一致风格；
//!   解析失败时降级为 in-memory only，**不**阻塞主流程（D 防御）。
//! - 原子写：先 `<file>.tmp` 再 `rename`，与 [`crate::core::plan_runtime::file_store::atomic_write_string`] 同口径，
//!   避免半态。
//!
//! 当前实现：`ChatContext` 在装配阶段为当前 session 构造一份 `TodosRuntime`，
//! `tools/todos.rs` 每次成功执行后通过它落盘；未注入时降级为内存-only。

use std::path::{Path, PathBuf};

use crate::core::plan_runtime::file_store::{TodoItem, TodoStatus};

/// 序列化到磁盘的 `.todo.md` 文件（最小 schema）。
#[derive(Debug, Clone)]
pub struct TodoFile {
    /// 该 todos 文件 id（区别于单条 todo 的 id）。
    pub todos_id: String,
    /// 可选标题；用于给新 scratchpad 命名。
    pub title: Option<String>,
    /// 创建时间（RFC3339 字符串）。
    pub created_at: String,
    /// items 列表。
    pub items: Vec<TodoItem>,
}

impl TodoFile {
    /// 新建空 TodoFile（`created_at` 自动取当前时间）。
    pub fn new(todos_id: impl Into<String>, title: Option<String>) -> Self {
        Self {
            todos_id: todos_id.into(),
            title,
            created_at: chrono::Local::now().to_rfc3339(),
            items: Vec::new(),
        }
    }

    /// 序列化为 markdown 文本（YAML frontmatter + body）。
    fn to_markdown(&self, session_id: &str) -> String {
        let mut out = String::from("---\n");
        out.push_str(&format!("todos_id: {}\n", self.todos_id));
        out.push_str(&format!("session_id: {}\n", session_id));
        if let Some(title) = &self.title {
            out.push_str(&format!("title: {}\n", title));
        }
        out.push_str(&format!("created_at: {}\n", self.created_at));
        out.push_str("schema_version: 1\n");
        out.push_str("---\n\n## Todos\n\n");
        if self.items.is_empty() {
            out.push_str("_(empty)_\n");
        } else {
            for it in &self.items {
                let checkbox = match it.status {
                    TodoStatus::Completed => "x",
                    TodoStatus::InProgress => "~",
                    TodoStatus::Cancelled => "-",
                    TodoStatus::Pending => " ",
                };
                out.push_str(&format!("- [{checkbox}] {}: {}\n", it.id, it.content));
            }
        }
        out
    }
}

/// `todos` 会话级持久化 runtime：持有当前 session 的 base 目录与 session_id。
#[derive(Debug, Clone)]
pub struct TodosRuntime {
    base_dir: PathBuf,
    session_id: String,
}

impl TodosRuntime {
    pub fn new(base_dir: PathBuf, session_id: impl Into<String>) -> Self {
        Self {
            base_dir,
            session_id: session_id.into(),
        }
    }

    fn todo_path(&self) -> PathBuf {
        todo_path_for(&self.base_dir, &self.session_id)
    }

    /// 原子写：tmp → rename。父目录不存在时自动创建。
    pub fn persist(&self, file: &TodoFile) -> std::io::Result<PathBuf> {
        let path = self.todo_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("todo.md.tmp");
        std::fs::write(&tmp, file.to_markdown(&self.session_id))?;
        std::fs::rename(&tmp, &path)?;
        Ok(path)
    }
}

/// 计算 todos 文件路径：`base_dir/todos/<session_id>.todo.md`。
pub fn todo_path_for(base_dir: &Path, session_id: &str) -> PathBuf {
    base_dir.join("todos").join(format!("{session_id}.todo.md"))
}
