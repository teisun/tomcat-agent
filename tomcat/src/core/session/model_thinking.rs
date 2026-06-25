use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::core::llm::thinking_policy::ThinkingLevel;
use crate::infra::error::AppError;
use crate::infra::platform::{read_file_utf8, write_file_atomic};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelThinkingFile {
    #[serde(default)]
    models: HashMap<String, ThinkingLevel>,
}

pub struct ModelThinkingStore {
    default_level: ThinkingLevel,
    models: Mutex<HashMap<String, ThinkingLevel>>,
    path: PathBuf,
}

impl ModelThinkingStore {
    pub fn load(path: impl Into<PathBuf>, default_level: ThinkingLevel) -> Result<Self, AppError> {
        let path = path.into();
        let models = load_models(&path)?;
        Ok(Self {
            default_level,
            models: Mutex::new(models),
            path,
        })
    }

    pub fn default_level(&self) -> ThinkingLevel {
        self.default_level
    }

    pub fn get(&self, model: &str) -> ThinkingLevel {
        let normalized = model.trim();
        if normalized.is_empty() {
            return self.default_level;
        }
        *self
            .models
            .lock()
            .get(normalized)
            .unwrap_or(&self.default_level)
    }

    pub fn set(&self, model: &str, level: ThinkingLevel) -> Result<(), AppError> {
        let normalized = model.trim();
        if normalized.is_empty() {
            return Ok(());
        }
        let snapshot = {
            let mut guard = self.models.lock();
            guard.insert(normalized.to_string(), level);
            guard.clone()
        };
        save_models(&self.path, &snapshot)
    }

    pub fn snapshot(&self) -> HashMap<String, ThinkingLevel> {
        self.models.lock().clone()
    }
}

fn load_models(path: &Path) -> Result<HashMap<String, ThinkingLevel>, AppError> {
    let content = match read_file_utf8(path) {
        Ok(s) => s,
        Err(AppError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            return reset_store(path);
        }
        Err(err) => return Err(err),
    };
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return reset_store(path);
    }
    match serde_json::from_str::<ModelThinkingFile>(trimmed) {
        Ok(store) => Ok(store.models),
        Err(err) => {
            warn!(
                path = %path.display(),
                error = %err,
                "model thinking store parse failed; rebuilding empty store"
            );
            reset_store(path)
        }
    }
}

fn save_models(path: &Path, models: &HashMap<String, ThinkingLevel>) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }
    let content = serde_json::to_string_pretty(&ModelThinkingFile {
        models: models.clone(),
    })?;
    write_file_atomic(path, content.as_bytes())
}

fn reset_store(path: &Path) -> Result<HashMap<String, ThinkingLevel>, AppError> {
    let models = HashMap::new();
    save_models(path, &models)?;
    Ok(models)
}
