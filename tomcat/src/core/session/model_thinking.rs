use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::core::llm::thinking_policy::ThinkingLevel;
use crate::core::llm::Speed;
use crate::infra::error::AppError;
use crate::infra::platform::{read_file_utf8, write_file_atomic};

/// A user's per-model choices. Model capabilities stay in `models.toml`; this
/// only records the user's reasoning, context-window and speed choices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPrefs {
    pub reasoning: ThinkingLevel,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub speed: Speed,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelPrefsFile {
    models: HashMap<String, ModelPrefs>,
}

/// Durable store for a user's per-model reasoning and context-window choices.
pub struct ModelPrefsStore {
    default_reasoning: ThinkingLevel,
    models: Mutex<HashMap<String, ModelPrefs>>,
    path: PathBuf,
}

impl ModelPrefsStore {
    pub fn load(
        path: impl Into<PathBuf>,
        default_reasoning: ThinkingLevel,
    ) -> Result<Self, AppError> {
        let path = path.into();
        let models = load_models(&path)?;
        Ok(Self {
            default_reasoning,
            models: Mutex::new(models),
            path,
        })
    }

    pub fn default_reasoning(&self) -> ThinkingLevel {
        self.default_reasoning
    }

    pub fn prefs_for(&self, model: &str) -> ModelPrefs {
        let normalized = model.trim();
        if normalized.is_empty() {
            return ModelPrefs {
                reasoning: self.default_reasoning,
                context_window: None,
                speed: Speed::Standard,
            };
        }
        self.models
            .lock()
            .get(normalized)
            .cloned()
            .unwrap_or(ModelPrefs {
                reasoning: self.default_reasoning,
                context_window: None,
                speed: Speed::Standard,
            })
    }

    /// Returns a persisted preference only when the user has chosen one for
    /// this model. Renderers must not present defaults as explicit selections.
    pub fn explicit_prefs_for(&self, model: &str) -> Option<ModelPrefs> {
        let normalized = model.trim();
        (!normalized.is_empty())
            .then(|| self.models.lock().get(normalized).cloned())
            .flatten()
    }

    pub fn reasoning_for(&self, model: &str) -> ThinkingLevel {
        self.prefs_for(model).reasoning
    }

    pub fn context_window_for(&self, model: &str) -> Option<u32> {
        self.prefs_for(model).context_window
    }

    pub fn set_reasoning(&self, model: &str, reasoning: ThinkingLevel) -> Result<(), AppError> {
        self.update(model, |prefs| prefs.reasoning = reasoning)
    }

    pub fn speed_for(&self, model: &str) -> Speed {
        self.prefs_for(model).speed
    }

    pub fn set_speed(&self, model: &str, speed: Speed) -> Result<(), AppError> {
        self.update(model, |prefs| prefs.speed = speed)
    }

    pub fn set_context_window(
        &self,
        model: &str,
        context_window: Option<u32>,
    ) -> Result<(), AppError> {
        self.update(model, |prefs| prefs.context_window = context_window)
    }

    pub fn snapshot(&self) -> HashMap<String, ModelPrefs> {
        self.models.lock().clone()
    }

    fn update(&self, model: &str, change: impl FnOnce(&mut ModelPrefs)) -> Result<(), AppError> {
        let normalized = model.trim();
        if normalized.is_empty() {
            return Ok(());
        }
        let mut guard = self.models.lock();
        let mut candidate = guard.clone();
        let prefs = candidate
            .entry(normalized.to_string())
            .or_insert(ModelPrefs {
                reasoning: self.default_reasoning,
                context_window: None,
                speed: Speed::Standard,
            });
        change(prefs);
        save_models(&self.path, &candidate)?;
        *guard = candidate;
        Ok(())
    }
}

fn load_models(path: &Path) -> Result<HashMap<String, ModelPrefs>, AppError> {
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
    match serde_json::from_str::<ModelPrefsFile>(trimmed) {
        Ok(store) => Ok(store.models),
        Err(err) => {
            let corrupt_path = preserve_corrupt_store(path)?;
            warn!(
                path = %path.display(),
                corrupt_path = %corrupt_path.display(),
                error = %err,
                "model preferences store parse failed; preserved corrupt store and rebuilding empty store"
            );
            reset_store(path)
        }
    }
}

fn save_models(path: &Path, models: &HashMap<String, ModelPrefs>) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }
    let content = serde_json::to_string_pretty(&ModelPrefsFile {
        models: models.clone(),
    })?;
    write_file_atomic(path, content.as_bytes())
}

fn preserve_corrupt_store(path: &Path) -> Result<PathBuf, AppError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("model-thinking.json");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let corrupt_path = parent.join(format!("{file_name}.corrupt-{timestamp}"));
    std::fs::rename(path, &corrupt_path).map_err(AppError::Io)?;
    Ok(corrupt_path)
}

fn reset_store(path: &Path) -> Result<HashMap<String, ModelPrefs>, AppError> {
    let models = HashMap::new();
    save_models(path, &models)?;
    Ok(models)
}
