use std::fs;
use std::path::Path;

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
    pub(super) fn capture_package(path: &Path) -> Self {
        Self::capture(path)
    }

    pub(super) fn capture_plugin(path: &Path) -> Self {
        Self::capture(path)
    }

    fn capture(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(raw_json) => Self {
                existed: true,
                raw_json,
            },
            Err(_) => Self {
                existed: false,
                raw_json: String::new(),
            },
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
    if !path.exists() {
        return Ok(T::default());
    }
    let raw = fs::read_to_string(path).map_err(AppError::Io)?;
    serde_json::from_str(&raw)
        .map_err(|error| AppError::Config(format!("registry 损坏: {} ({error})", path.display())))
}

fn save_registry<T>(path: &Path, registry: &T) -> Result<(), AppError>
where
    T: serde::Serialize,
{
    let json = serde_json::to_vec_pretty(registry).map_err(AppError::Serialize)?;
    write_file_atomic(path, &json)
}
