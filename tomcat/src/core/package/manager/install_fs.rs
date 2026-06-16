use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::infra::AppError;

use super::super::model::PreparedInstallResource;
use super::super::paths::LayerPaths;
use super::registry::RegistrySnapshot;

#[derive(Debug, Clone)]
pub(super) enum InstallFsMutation {
    Installed {
        destination_dir: PathBuf,
        backup_dir: Option<PathBuf>,
        stage_dir: PathBuf,
    },
    Removed {
        original_path: PathBuf,
        backup_path: PathBuf,
    },
}

pub(super) fn install_resource(
    resource: &PreparedInstallResource,
    force: bool,
) -> Result<InstallFsMutation, AppError> {
    let parent = resource
        .destination_dir
        .parent()
        .ok_or_else(|| AppError::Config("目标目录无父目录".to_string()))?;
    fs::create_dir_all(parent).map_err(AppError::Io)?;

    let stage_dir = hidden_sibling_path(parent, &resource.id, "staging");
    copy_dir_recursive(&resource.source_dir, &stage_dir).inspect_err(|_| {
        let _ = remove_path_if_exists(&stage_dir);
    })?;

    let backup_dir = if resource.destination_dir.exists() {
        if !force {
            let _ = remove_path_if_exists(&stage_dir);
            return Err(AppError::Config(format!(
                "目标已存在且未开启 force: {}",
                resource.destination_dir.display()
            )));
        }
        let backup = hidden_sibling_path(parent, &resource.id, "backup");
        fs::rename(&resource.destination_dir, &backup).map_err(AppError::Io)?;
        Some(backup)
    } else {
        None
    };

    if let Err(error) = fs::rename(&stage_dir, &resource.destination_dir) {
        let _ = remove_path_if_exists(&stage_dir);
        if let Some(backup) = &backup_dir {
            let _ = fs::rename(backup, &resource.destination_dir);
        }
        return Err(AppError::Io(error));
    }

    Ok(InstallFsMutation::Installed {
        destination_dir: resource.destination_dir.clone(),
        backup_dir,
        stage_dir,
    })
}

pub(super) fn prepare_force_remove_path(
    path: &Path,
) -> Result<Option<InstallFsMutation>, AppError> {
    if !path.exists() {
        return Ok(None);
    }
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("待移除资源无父目录".to_string()))?;
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::Config("待移除资源名称无效".to_string()))?;
    let backup_path = hidden_sibling_path(parent, stem, "backup");
    fs::rename(path, &backup_path).map_err(AppError::Io)?;
    Ok(Some(InstallFsMutation::Removed {
        original_path: path.to_path_buf(),
        backup_path,
    }))
}

pub(super) fn cleanup_install_artifacts(mutations: &[InstallFsMutation]) {
    for mutation in mutations {
        match mutation {
            InstallFsMutation::Installed {
                backup_dir,
                stage_dir,
                ..
            } => {
                if let Some(backup_dir) = backup_dir {
                    let _ = remove_path_if_exists(backup_dir);
                }
                let _ = remove_path_if_exists(stage_dir);
            }
            InstallFsMutation::Removed { backup_path, .. } => {
                let _ = remove_path_if_exists(backup_path);
            }
        }
    }
}

pub(super) fn rollback_install(
    layer_paths: &LayerPaths,
    package_snapshot: &RegistrySnapshot,
    plugin_snapshot: &RegistrySnapshot,
    mutations: &[InstallFsMutation],
) -> Vec<String> {
    let mut errors = Vec::new();
    for mutation in mutations.iter().rev() {
        match mutation {
            InstallFsMutation::Installed {
                destination_dir,
                backup_dir,
                stage_dir,
            } => {
                if let Err(error) = remove_path_if_exists(destination_dir) {
                    errors.push(format!(
                        "remove {} failed: {error}",
                        destination_dir.display()
                    ));
                }
                if let Some(backup_dir) = backup_dir {
                    if backup_dir.exists() {
                        if let Err(error) = fs::rename(backup_dir, destination_dir) {
                            errors.push(format!(
                                "restore {} failed: {error}",
                                destination_dir.display()
                            ));
                        }
                    }
                }
                if stage_dir.exists() {
                    if let Err(error) = remove_path_if_exists(stage_dir) {
                        errors.push(format!(
                            "cleanup stage {} failed: {error}",
                            stage_dir.display()
                        ));
                    }
                }
            }
            InstallFsMutation::Removed {
                original_path,
                backup_path,
            } => {
                if backup_path.exists() {
                    if let Err(error) = fs::rename(backup_path, original_path) {
                        errors.push(format!(
                            "restore removed {} failed: {error}",
                            original_path.display()
                        ));
                    }
                }
            }
        }
    }

    if let Err(error) = package_snapshot.restore(&layer_paths.package_registry_path) {
        errors.push(format!("restore package registry failed: {error}"));
    }
    if let Err(error) = plugin_snapshot.restore(&layer_paths.plugin_registry_path) {
        errors.push(format!("restore plugin registry failed: {error}"));
    }
    errors
}

pub(super) fn remove_path_if_exists(path: &Path) -> Result<bool, AppError> {
    if !path.exists() {
        return Ok(false);
    }
    let metadata = fs::symlink_metadata(path).map_err(AppError::Io)?;
    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(AppError::Io)?;
    } else {
        fs::remove_file(path).map_err(AppError::Io)?;
    }
    Ok(true)
}

fn hidden_sibling_path(parent: &Path, stem: &str, suffix: &str) -> PathBuf {
    parent.join(format!(".{stem}.{suffix}.{}", Uuid::new_v4()))
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<(), AppError> {
    let metadata = fs::symlink_metadata(source).map_err(AppError::Io)?;
    if metadata.file_type().is_symlink() {
        return Err(AppError::Permission(format!(
            "不支持复制符号链接目录: {}",
            source.display()
        )));
    }
    if !metadata.is_dir() {
        return Err(AppError::Config(format!(
            "待安装资源必须是目录: {}",
            source.display()
        )));
    }
    fs::create_dir_all(target).map_err(AppError::Io)?;
    for entry in fs::read_dir(source).map_err(AppError::Io)? {
        let entry = entry.map_err(AppError::Io)?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = entry.file_type().map_err(AppError::Io)?;
        if file_type.is_symlink() {
            return Err(AppError::Permission(format!(
                "不支持复制符号链接文件: {}",
                source_path.display()
            )));
        }
        if file_type.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &target_path).map_err(AppError::Io)?;
        }
    }
    Ok(())
}
