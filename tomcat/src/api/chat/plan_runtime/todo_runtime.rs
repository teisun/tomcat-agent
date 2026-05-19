//! `TodoRuntime` — per-session todos 持久化（GAP-N12 / G3）。
//!
//! 设计口径（plan-runtime.md §G3）：
//!
//! - 每个 chat session 持有一份**当前 active TodoFile**，落盘到
//!   `~/.tomcat/agents/<id>/sessions/<sid>/todos/<todos_id>.todo.md`；
//! - `TodoRuntime` 不直接管理多文件历史——历史 todos 文件由 `sessions.json` 的
//!   `activeTodosId` 指针指向当前一个；purge / 切换交由调用方控制。
//! - 落盘格式：YAML frontmatter（id / session_key / created_at）+ markdown body
//!   （`## Todos\n- [ ] id: content` 列表），与 `PlanFile` schema 一致风格；
//!   解析失败时降级为 in-memory only，**不**阻塞主流程（D 防御）。
//! - 原子写：先 `<file>.tmp` 再 `rename`，与 [`crate::api::chat::plan_runtime::file_store::atomic_write_string`] 同口径，
//!   避免半态。
//!
//! 当前 P3 实现：暴露纯 `persist` / `load` 接口，由 `tools/todos.rs` 在每次 `execute`
//! 成功后**异步**调用；activeTodosId 指针与 `sessions.json` 集成由 C 段 chat_loop 装配
//! 阶段接入。

use std::path::{Path, PathBuf};

use crate::api::chat::plan_runtime::file_store::{TodoItem, TodoStatus};

/// 序列化到磁盘的 `.todo.md` 文件（最小 schema）。
#[derive(Debug, Clone)]
pub struct TodoFile {
    /// 该 todos 文件 id（区别于单条 todo 的 id）。
    pub todos_id: String,
    /// 创建该 todos 文件的 session_key。
    pub session_key: String,
    /// 可选标题；用于给新 scratchpad 命名。
    pub title: Option<String>,
    /// 创建时间（RFC3339 字符串），用于 purge / 排序。
    pub created_at: String,
    /// items 列表。
    pub items: Vec<TodoItem>,
}

impl TodoFile {
    /// 新建空 TodoFile（`created_at` 自动取当前时间）。
    pub fn new(
        todos_id: impl Into<String>,
        session_key: impl Into<String>,
        title: Option<String>,
    ) -> Self {
        Self {
            todos_id: todos_id.into(),
            session_key: session_key.into(),
            title,
            created_at: chrono::Local::now().to_rfc3339(),
            items: Vec::new(),
        }
    }

    /// 序列化为 markdown 文本（YAML frontmatter + body）。
    pub fn to_markdown(&self) -> String {
        let mut out = String::from("---\n");
        out.push_str(&format!("todos_id: {}\n", self.todos_id));
        out.push_str(&format!("session_key: {}\n", self.session_key));
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

/// 计算 todos 文件路径：`base_dir/sessions/<session_key>/todos/<todos_id>.todo.md`。
///
/// `base_dir` 通常为 `resolve_sessions_dir` 的父级（即 `~/.tomcat/agents/<id>`）。
/// `session_key` 即 `PlanRuntime::session_key()`。
pub fn todo_path_for(base_dir: &Path, session_key: &str, todos_id: &str) -> PathBuf {
    base_dir
        .join("sessions")
        .join(session_key)
        .join("todos")
        .join(format!("{todos_id}.todo.md"))
}

/// 原子写：tmp → rename。父目录不存在时自动创建。
pub fn persist(base_dir: &Path, file: &TodoFile) -> std::io::Result<PathBuf> {
    let path = todo_path_for(base_dir, &file.session_key, &file.todos_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("todo.md.tmp");
    std::fs::write(&tmp, file.to_markdown())?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// 列出当前 session 的全部 todos 文件路径（不解析内容；仅做排序与 purge 用）。
pub fn list_session_todos_files(
    base_dir: &Path,
    session_key: &str,
) -> std::io::Result<Vec<PathBuf>> {
    let dir = base_dir.join("sessions").join(session_key).join("todos");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.ends_with(".todo.md"))
        {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// purge：删除除 `keep_id` 之外的所有 inactive todos 文件（GAP-N12 `purge_inactive_on_new_todos`）。
///
/// 调用方在生成"新 todos 文件"时按需调用。失败仅 log，不阻塞主流程。
pub fn purge_inactive(base_dir: &Path, session_key: &str, keep_id: &str) -> std::io::Result<usize> {
    let files = list_session_todos_files(base_dir, session_key)?;
    let mut removed = 0;
    for f in files {
        let id = f
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_suffix(".todo.md"))
            .unwrap_or("");
        if id == keep_id {
            continue;
        }
        if std::fs::remove_file(&f).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
