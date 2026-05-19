use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CheckpointId(String);

impl CheckpointId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn null() -> Self {
        Self("__null__".to_string())
    }

    pub fn is_null(&self) -> bool {
        self.0 == "__null__"
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn short(&self) -> String {
        self.0.chars().take(8).collect()
    }
}

impl Default for CheckpointId {
    fn default() -> Self {
        Self::null()
    }
}

impl std::fmt::Display for CheckpointId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CheckpointKind {
    TurnEnd,
    Interrupt,
    Manual { label: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointMeta {
    pub id: CheckpointId,
    pub session_id: String,
    pub turn_id: String,
    pub kind: CheckpointKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_anchor: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointRecordRequest {
    pub session_id: String,
    pub turn_id: String,
    pub kind: CheckpointKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_anchor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
pub struct ListOptions {
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct RestoreOptions {
    pub paths: Vec<PathBuf>,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CheckpointDiff {
    pub text: String,
    pub changed_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct CheckpointRestoreReport {
    pub checkpoint_id: CheckpointId,
    pub changed_paths: Vec<PathBuf>,
    pub dry_run: bool,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct RetentionPolicy {
    pub retention_max: usize,
    pub retention_days: u32,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            retention_max: 50,
            retention_days: 7,
        }
    }
}

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error("checkpoint io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("checkpoint serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("checkpoint command failed: {0}")]
    CommandFailed(String),
    #[error("checkpoint invalid path: {0}")]
    InvalidPath(String),
    #[error("checkpoint not found: {0}")]
    NotFound(String),
    #[error("checkpoint unsupported: {0}")]
    Unsupported(String),
}
