use std::fs;
use std::path::Path;

#[cfg(test)]
use std::path::PathBuf;

#[cfg(test)]
use std::sync::{Mutex, OnceLock};

use crate::infra::{write_file_atomic, AppError};

use super::super::model::{PackageRegistryFile, PluginRegistryFile};

pub fn load_package_registry(path: &Path) -> Result<PackageRegistryFile, AppError> {
    let mut registry: PackageRegistryFile = load_registry(path)?;
    registry.normalize();
    Ok(registry)
}

pub fn save_package_registry(path: &Path, registry: &PackageRegistryFile) -> Result<(), AppError> {
    save_registry(path, registry)
}

pub fn load_plugin_registry(path: &Path) -> Result<PluginRegistryFile, AppError> {
    load_registry(path)
}

pub fn save_plugin_registry(path: &Path, registry: &PluginRegistryFile) -> Result<(), AppError> {
    save_registry(path, registry)
}

#[derive(Debug, Clone)]
pub(super) struct RegistrySnapshot {
    existed: bool,
    raw_json: String,
}

impl RegistrySnapshot {
    pub(super) fn capture_package(path: &Path) -> Result<Self, AppError> {
        Self::capture(path)
    }

    pub(super) fn capture_plugin(path: &Path) -> Result<Self, AppError> {
        Self::capture(path)
    }

    fn capture(path: &Path) -> Result<Self, AppError> {
        match fs::read_to_string(path) {
            Ok(raw_json) => Ok(Self {
                existed: true,
                raw_json,
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                existed: false,
                raw_json: String::new(),
            }),
            Err(error) => Err(AppError::Io(error)),
        }
    }

    pub(super) fn package_value(&self) -> Result<PackageRegistryFile, AppError> {
        if !self.existed {
            return Ok(PackageRegistryFile::default());
        }
        let mut registry: PackageRegistryFile =
            serde_json::from_str(&self.raw_json).map_err(|error| {
                AppError::Config(format!("package registry snapshot 损坏: {error}"))
            })?;
        registry.normalize();
        Ok(registry)
    }

    pub(super) fn plugin_value(&self) -> Result<PluginRegistryFile, AppError> {
        if !self.existed {
            return Ok(PluginRegistryFile::default());
        }
        serde_json::from_str(&self.raw_json)
            .map_err(|error| AppError::Config(format!("plugin registry snapshot 损坏: {error}")))
    }

    pub(super) fn restore(&self, path: &Path) -> Result<(), AppError> {
        if self.existed {
            write_file_atomic(path, self.raw_json.as_bytes())
        } else if path.exists() {
            fs::remove_file(path).map_err(AppError::Io)
        } else {
            Ok(())
        }
    }
}

fn load_registry<T>(path: &Path) -> Result<T, AppError>
where
    T: serde::de::DeserializeOwned + Default,
{
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(error) => return Err(AppError::Io(error)),
    };
    serde_json::from_str(&raw)
        .map_err(|error| AppError::Config(format!("registry 损坏: {} ({error})", path.display())))
}

#[cfg(test)]
static FAIL_NEXT_SAVE_PATH: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

#[cfg(test)]
pub(crate) fn fail_next_save_for_test(path: &Path) {
    *FAIL_NEXT_SAVE_PATH
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = Some(path.to_path_buf());
}

#[cfg(test)]
fn consume_save_failure(path: &Path) -> bool {
    let slot = FAIL_NEXT_SAVE_PATH.get_or_init(|| Mutex::new(None));
    let mut pending = slot.lock().unwrap();
    if pending.as_deref() == Some(path) {
        *pending = None;
        true
    } else {
        false
    }
}

fn save_registry<T>(path: &Path, registry: &T) -> Result<(), AppError>
where
    T: serde::Serialize,
{
    #[cfg(test)]
    if consume_save_failure(path) {
        return Err(AppError::Config(format!(
            "测试注入的 registry 保存失败: {}",
            path.display()
        )));
    }
    let json = serde_json::to_vec_pretty(registry).map_err(AppError::Serialize)?;
    write_file_atomic(path, &json)
}
