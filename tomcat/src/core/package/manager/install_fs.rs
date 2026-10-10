use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

use sha2::{Digest, Sha256};

use uuid::Uuid;

use crate::infra::i18n::tr;
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
        .ok_or_else(|| AppError::Config(tr("package.destinationNoParent", &[])))?;
    fs::create_dir_all(parent).map_err(AppError::Io)?;
    reject_symlink_path(parent, &tr("package.destinationParent", &[]))?;
    reject_symlink_path(&resource.destination_dir, &tr("package.destination", &[]))?;
    if paths_overlap(&resource.source_dir, &resource.destination_dir) {
        return Err(AppError::Config(tr(
            "package.overlap",
            &[
                ("source", &resource.source_dir.display().to_string()),
                (
                    "destination",
                    &resource.destination_dir.display().to_string(),
                ),
            ],
        )));
    }

    let stage_dir = hidden_sibling_path(parent, &resource.id, "staging");
    copy_dir_snapshot_checked(&resource.source_dir, &stage_dir, &resource.source_digest)
        .inspect_err(|_| {
            let _ = remove_path_if_exists(&stage_dir);
        })?;

    let backup_dir = if resource.destination_dir.exists() {
        if !force {
            let _ = remove_path_if_exists(&stage_dir);
            return Err(AppError::Config(tr(
                "package.forceRequired",
                &[("path", &resource.destination_dir.display().to_string())],
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

fn reject_symlink_path(path: &Path, label: &str) -> Result<(), AppError> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Err(AppError::Permission(tr(
                "package.pathSymlink",
                &[("label", label), ("path", &path.display().to_string())],
            )));
        }
    }
    Ok(())
}

pub(super) fn prepare_force_remove_path(
    path: &Path,
) -> Result<Option<InstallFsMutation>, AppError> {
    if !path.exists() {
        return Ok(None);
    }
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config(tr("package.removeNoParent", &[])))?;
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::Config(tr("package.removeBadName", &[])))?;
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
                    errors.push(tr(
                        "package.rollbackRemove",
                        &[
                            ("path", &destination_dir.display().to_string()),
                            ("detail", &error.to_string()),
                        ],
                    ));
                }
                if let Some(backup_dir) = backup_dir {
                    if backup_dir.exists() {
                        if let Err(error) = fs::rename(backup_dir, destination_dir) {
                            errors.push(tr(
                                "package.rollbackRestore",
                                &[
                                    ("path", &destination_dir.display().to_string()),
                                    ("detail", &error.to_string()),
                                ],
                            ));
                        }
                    }
                }
                if stage_dir.exists() {
                    if let Err(error) = remove_path_if_exists(stage_dir) {
                        errors.push(tr(
                            "package.rollbackStage",
                            &[
                                ("path", &stage_dir.display().to_string()),
                                ("detail", &error.to_string()),
                            ],
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
                        errors.push(tr(
                            "package.rollbackRemoved",
                            &[
                                ("path", &original_path.display().to_string()),
                                ("detail", &error.to_string()),
                            ],
                        ));
                    }
                }
            }
        }
    }

    if let Err(error) = package_snapshot.restore(&layer_paths.package_registry_path) {
        errors.push(tr(
            "package.rollbackRegistry",
            &[("kind", "package"), ("detail", &error.to_string())],
        ));
    }
    if let Err(error) = plugin_snapshot.restore(&layer_paths.plugin_registry_path) {
        errors.push(tr(
            "package.rollbackRegistry",
            &[("kind", "plugin"), ("detail", &error.to_string())],
        ));
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

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

/// Fingerprints the safe, regular-file-only source tree before confirmation. The
/// digest records every relative name and every byte, so a changed source cannot
/// be substituted between preparation and staging.
pub(super) fn source_tree_digest(source: &Path) -> Result<String, AppError> {
    let mut hasher = Sha256::new();
    snapshot_tree(source, Path::new(""), None, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn copy_dir_snapshot_checked(
    source: &Path,
    target: &Path,
    expected_digest: &str,
) -> Result<(), AppError> {
    let mut hasher = Sha256::new();
    snapshot_tree(source, Path::new(""), Some(target), &mut hasher)?;
    let actual_digest = format!("{:x}", hasher.finalize());
    if actual_digest != expected_digest {
        return Err(AppError::Permission(tr(
            "package.sourceChanged",
            &[("path", &source.display().to_string())],
        )));
    }
    Ok(())
}

/// Walks an already named source directory into a private staging snapshot. It
/// accepts only regular files and directories, rejects every symbolic link and
/// special file, and on Unix opens files with `O_NOFOLLOW` before copying bytes.
/// Platforms without that no-follow primitive fail closed instead of performing
/// an installation with weaker guarantees.
fn snapshot_tree(
    source: &Path,
    relative: &Path,
    target: Option<&Path>,
    hasher: &mut Sha256,
) -> Result<(), AppError> {
    let directory_handle = open_source_directory(source)?;
    let metadata = directory_handle.metadata().map_err(AppError::Io)?;
    if !metadata.is_dir() {
        return Err(AppError::Config(tr(
            "package.sourceDirectory",
            &[("path", &source.display().to_string())],
        )));
    }
    if let Some(target) = target {
        fs::create_dir_all(target).map_err(AppError::Io)?;
        reject_symlink_path(target, &tr("package.staging", &[]))?;
    }

    let mut entries = fs::read_dir(source)
        .map_err(AppError::Io)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::Io)?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            AppError::Config(tr(
                "package.sourceUtf8",
                &[("path", &entry.path().display().to_string())],
            ))
        })?;
        let source_path = entry.path();
        let child_relative = relative.join(name);
        let target_path = target.map(|target| target.join(name));
        let file_type = entry.file_type().map_err(AppError::Io)?;
        if file_type.is_symlink() {
            return Err(AppError::Permission(tr(
                "package.copySymlink",
                &[("path", &source_path.display().to_string())],
            )));
        }
        if file_type.is_dir() {
            hasher.update(b"D\0");
            hasher.update(child_relative.to_string_lossy().as_bytes());
            snapshot_tree(
                &source_path,
                &child_relative,
                target_path.as_deref(),
                hasher,
            )?;
        } else if file_type.is_file() {
            hasher.update(b"F\0");
            hasher.update(child_relative.to_string_lossy().as_bytes());
            copy_regular_file(&source_path, target_path.as_deref(), hasher)?;
        } else {
            return Err(AppError::Permission(tr(
                "package.specialFile",
                &[("path", &source_path.display().to_string())],
            )));
        }
    }
    ensure_source_directory_unchanged(source, &directory_handle)?;
    Ok(())
}

#[cfg(unix)]
fn open_source_directory(path: &Path) -> Result<File, AppError> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(AppError::Io)
}

#[cfg(not(unix))]
fn open_source_directory(_path: &Path) -> Result<File, AppError> {
    Err(AppError::Permission(tr("package.noSafeSnapshot", &[])))
}

#[cfg(unix)]
fn ensure_source_directory_unchanged(path: &Path, handle: &File) -> Result<(), AppError> {
    let opened = handle.metadata().map_err(AppError::Io)?;
    let current = fs::symlink_metadata(path).map_err(AppError::Io)?;
    if current.file_type().is_symlink()
        || !current.is_dir()
        || opened.dev() != current.dev()
        || opened.ino() != current.ino()
    {
        return Err(AppError::Permission(tr(
            "package.directoryChanged",
            &[("path", &path.display().to_string())],
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_source_directory_unchanged(_path: &Path, _handle: &File) -> Result<(), AppError> {
    Err(AppError::Permission(tr("package.noSafeSnapshot", &[])))
}

#[cfg(unix)]
fn open_source_file(path: &Path) -> Result<File, AppError> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(AppError::Io)
}

#[cfg(not(unix))]
fn open_source_file(_path: &Path) -> Result<File, AppError> {
    Err(AppError::Permission(tr("package.noSafeSnapshot", &[])))
}

fn copy_regular_file(
    source: &Path,
    target: Option<&Path>,
    hasher: &mut Sha256,
) -> Result<(), AppError> {
    let mut source_file = open_source_file(source)?;
    let metadata = source_file.metadata().map_err(AppError::Io)?;
    if !metadata.is_file() {
        return Err(AppError::Permission(tr(
            "package.notRegular",
            &[("path", &source.display().to_string())],
        )));
    }
    let mut target_file = target
        .map(|path| File::create(path).map_err(AppError::Io))
        .transpose()?;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source_file.read(&mut buffer).map_err(AppError::Io)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        if let Some(target_file) = target_file.as_mut() {
            target_file
                .write_all(&buffer[..read])
                .map_err(AppError::Io)?;
        }
    }
    Ok(())
}
