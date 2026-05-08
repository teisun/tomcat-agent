//! `tomcat.config.toml` 文件级互斥锁。
//!
//! 用 `fs2::FileExt::lock_exclusive` 在写入磁盘前抢占独占锁，
//! 防止多个 `tomcat` 进程并发写入造成 TOML 损坏。
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
    lock.lock_exclusive().map_err(AppError::Io)?;
    let res = f();
    let _ = FileExt::unlock(&lock);
    res
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
