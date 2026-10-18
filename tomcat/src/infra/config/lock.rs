//! Stable advisory file locks for configuration and session I/O.
//!
//! The lock has its own inode so atomic replacement of the data file cannot split ownership.
//! Keep lock files in place: unlinking one while callers hold/wait on it defeats exclusion.
//!
//! 用法：
//!
//! ```text
//! use crate::infra::config::lock::with_config_lock;
//! with_config_lock(&config_path, || {
//!     // 在锁内：读 -> 改 -> write_file_atomic
//!     ...
//! })?;
//! ```

use fs2::FileExt;
use std::fs::OpenOptions;
use std::path::Path;

use super::super::error::AppError;

/// 在 `<config_path>.lock` 上抢独占锁，调用 `f` 完成读改写后释放。
///
/// - 锁文件位于配置文件同目录，文件名后缀 `.lock`；自动创建。
/// - 锁是 advisory：仅同样使用 `with_config_lock` 的进程才会等待。
/// - 调用方应在 `f` 内做完整的「读 -> 修改 -> 写」，避免外部状态漂移。
pub fn with_config_lock<R>(
    config_path: &Path,
    f: impl FnOnce() -> Result<R, AppError>,
) -> Result<R, AppError> {
    let lock = acquire_config_lock(config_path)?;
    let res = f();
    let _ = FileExt::unlock(&lock);
    res
}

/// Hold the existing advisory lock across a caller-owned synchronous operation.
/// Closing the returned file releases the lock, including on early return or panic.
pub(crate) fn acquire_config_lock(config_path: &Path) -> Result<std::fs::File, AppError> {
    let lock = open_lock_file(config_path)?;
    lock.lock_exclusive().map_err(AppError::Io)?;
    Ok(lock)
}

/// Background housekeeping skips an already-busy file instead of delaying its writer.
pub(crate) fn try_acquire_config_lock(
    config_path: &Path,
) -> Result<Option<std::fs::File>, AppError> {
    let lock = open_lock_file(config_path)?;
    match FileExt::try_lock_exclusive(&lock) {
        Ok(()) => Ok(Some(lock)),
        Err(error) if crate::infra::platform::is_lock_contended(&error) => Ok(None),
        Err(error) => Err(AppError::Io(error)),
    }
}

fn open_lock_file(config_path: &Path) -> Result<std::fs::File, AppError> {
    let lock_path = lock_path_for(config_path);
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(AppError::Io)?;
    Ok(lock)
}

fn lock_path_for(config_path: &Path) -> std::path::PathBuf {
    let parent = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let name = config_path
        .file_name()
        .map(|n| format!("{}.lock", n.to_string_lossy()))
        .unwrap_or_else(|| ".tomcat.config.toml.lock".to_string());
    parent.join(name)
}
