//! Agent-local working document. One runtime and one file per Agent session.
//! Save before committing memory; publish after unlocking. Plan lifecycle is unrelated.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::file_store::{TodoItem, TodoStatus};
use crate::infra::error::AppError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoFile {
    pub todos_id: String,
    pub title: Option<String>,
    pub created_at: String,
    pub items: Vec<TodoItem>,
    #[serde(skip)]
    pub draft: String,
}

impl TodoFile {
    fn new() -> Self {
        Self {
            todos_id: format!("td_{}", uuid::Uuid::new_v4()),
            title: None,
            created_at: chrono::Local::now().to_rfc3339(),
            items: Vec::new(),
            draft: String::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Frontmatter {
    schema_version: u32,
    session_id: String,
    #[serde(flatten)]
    document: TodoFile,
}

/// Main-session projection only. Child Agents deliberately have no publisher.
pub type TodosPublisher = Arc<dyn Fn(&[TodoItem]) + Send + Sync>;

#[derive(Default)]
struct DocumentState {
    document: Option<TodoFile>,
    load_error: Option<String>,
}

pub struct TodosRuntime {
    path: PathBuf,
    session_id: String,
    state: Mutex<DocumentState>,
    publish: Option<TodosPublisher>,
}

impl TodosRuntime {
    /// A bad document blocks Todos updates, not the rest of the chat session.
    pub fn open(
        base_dir: PathBuf,
        session_id: impl Into<String>,
        publish: Option<TodosPublisher>,
    ) -> Self {
        let session_id = session_id.into();
        let path = todo_path_for(&base_dir, &session_id);
        let mut runtime = Self {
            path,
            session_id,
            state: Mutex::new(DocumentState::default()),
            publish,
        };
        let state = runtime.load_state();
        *runtime.state.get_mut() = state;
        runtime
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn snapshot(&self) -> Option<TodoFile> {
        self.state.lock().document.clone()
    }

    /// UI and compaction projections never clone the potentially long Markdown body.
    pub fn snapshot_items(&self) -> Vec<TodoItem> {
        self.state
            .lock()
            .document
            .as_ref()
            .map(|doc| doc.items.clone())
            .unwrap_or_default()
    }

    pub fn has_unfinished(&self) -> bool {
        self.state.lock().document.as_ref().is_some_and(|doc| {
            doc.items
                .iter()
                .any(|item| matches!(item.status, TodoStatus::Pending | TodoStatus::InProgress))
        })
    }

    /// Even a draft-only document is a useful recovery entry.
    pub fn progress(&self) -> Option<(PathBuf, Vec<TodoItem>)> {
        self.state
            .lock()
            .document
            .as_ref()
            .map(|doc| (self.path.clone(), doc.items.clone()))
    }

    /// Each runtime has one AgentLoop writer. Do not call publishers while holding the
    /// document lock: they may query the committed snapshot themselves.
    pub fn commit<E: From<AppError>>(
        &self,
        new_document: bool,
        update: impl FnOnce(&mut TodoFile) -> Result<(), E>,
    ) -> Result<TodoFile, E> {
        let mut state = self.state.lock();
        if state.load_error.is_some() {
            *state = self.load_state();
        }
        if let Some(error) = &state.load_error {
            return Err(self
                .error(format!("{error}; repair the file and retry"))
                .into());
        }
        let mut candidate = if new_document {
            TodoFile::new()
        } else {
            state.document.clone().unwrap_or_else(TodoFile::new)
        };
        update(&mut candidate)?;
        let header = Frontmatter {
            schema_version: 2,
            session_id: self.session_id.clone(),
            document: candidate.clone(),
        };
        let yaml = serde_yaml::to_string(&header).map_err(|error| E::from(self.error(error)))?;
        let markdown = format!("---\n{yaml}---\n{}", candidate.draft);
        crate::infra::platform::write_file_atomic(&self.path, markdown.as_bytes())
            .map_err(|error| E::from(self.error(error)))?;
        state.document = Some(candidate.clone());
        drop(state);
        if let Some(publish) = &self.publish {
            publish(&candidate.items);
        }
        Ok(candidate)
    }

    fn error(&self, error: impl std::fmt::Display) -> AppError {
        AppError::Config(format!("Todos file {}: {error}", self.path.display()))
    }

    fn load_state(&self) -> DocumentState {
        match self.load() {
            Ok(document) => DocumentState {
                document,
                load_error: None,
            },
            Err(error) => {
                tracing::warn!(path = %self.path.display(), %error, "Todos document unavailable");
                DocumentState {
                    document: None,
                    load_error: Some(error.to_string()),
                }
            }
        }
    }

    fn load(&self) -> Result<Option<TodoFile>, AppError> {
        let markdown = match std::fs::read_to_string(&self.path) {
            Ok(markdown) => markdown,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(self.error(error)),
        };
        let (yaml, draft) = markdown
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .ok_or_else(|| self.error("missing YAML frontmatter delimiters"))?;
        #[derive(Deserialize)]
        struct Version {
            schema_version: u32,
        }
        let version: Version = serde_yaml::from_str(yaml).map_err(|error| self.error(error))?;
        // v1 was a write-only projection, not a restorable document. Upgrade on next commit.
        if version.schema_version == 1 {
            return Ok(None);
        }
        if version.schema_version != 2 {
            return Err(self.error(format!(
                "unsupported schema version {}",
                version.schema_version
            )));
        }
        let header: Frontmatter = serde_yaml::from_str(yaml).map_err(|error| self.error(error))?;
        if header.session_id != self.session_id {
            return Err(self.error("session_id does not match this Agent session"));
        }
        let mut document = header.document;
        document.draft = draft.to_string();
        Ok(Some(document))
    }
}

pub fn todo_path_for(base_dir: &Path, session_id: &str) -> PathBuf {
    base_dir.join("todos").join(format!("{session_id}.todo.md"))
}
