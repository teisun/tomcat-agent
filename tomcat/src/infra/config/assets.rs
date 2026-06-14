//! 资产目录管理：SHA-256 辅助、原子写入、文件锁与 `assets/` 目录初始化。

use std::path::Path;

use super::super::error::AppError;
use super::load::get_work_dir;
use super::types::AppConfig;

#[cfg(test)]
use sha2::{Digest, Sha256};

#[cfg(test)]
pub(super) fn compute_file_sha256(path: &Path) -> Result<String, AppError> {
    let data = std::fs::read(path).map_err(AppError::Io)?;
    Ok(format!("{:x}", Sha256::digest(&data)))
}

#[cfg(test)]
pub(super) fn compute_dir_sha256(dir: &Path) -> Result<String, AppError> {
    let mut entries: Vec<(String, String)> = Vec::new();
    collect_dir_hashes(dir, dir, &mut entries)?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (rel, file_hash) in &entries {
        hasher.update(rel.as_bytes());
        hasher.update(file_hash.as_bytes());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
fn collect_dir_hashes(
    base: &Path,
    current: &Path,
    out: &mut Vec<(String, String)>,
) -> Result<(), AppError> {
    let entries = std::fs::read_dir(current).map_err(AppError::Io)?;
    for entry in entries {
        let entry = entry.map_err(AppError::Io)?;
        let path = entry.path();
        if path.is_dir() {
            collect_dir_hashes(base, &path, out)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let hash = compute_file_sha256(&path)?;
            out.push((rel, hash));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn write_atomic(target: &Path, content: &[u8]) -> Result<(), AppError> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }
    let tmp = target.with_extension("tmp");
    std::fs::write(&tmp, content).map_err(AppError::Io)?;
    std::fs::rename(&tmp, target).or_else(|_| {
        std::fs::copy(&tmp, target).map_err(AppError::Io)?;
        let _ = std::fs::remove_file(&tmp);
        Ok(())
    })
}

pub(super) fn acquire_assets_lock(work_dir: &Path) -> Result<std::fs::File, AppError> {
    use fs2::FileExt;
    let lock_dir = work_dir.join("assets");
    std::fs::create_dir_all(&lock_dir).map_err(AppError::Io)?;
    let lock_path = lock_dir.join(".lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(AppError::Io)?;
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(10);
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(_) if start.elapsed() < timeout => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => {
                return Err(AppError::Config(
                    "资源锁超时（10s），请检查是否有其他 tomcat 进程卡住，或手动删除 ~/.tomcat/assets/.lock"
                        .to_string(),
                ));
            }
        }
    }
}

/// 确保 `work_dir/assets/` 已就绪。
/// 在 `ensure_work_dir_structure` 之后、正式业务逻辑之前调用。
/// 当前 rquickjs 后端不再抽取 Wasm/Node 兼容资产，仅保留目录初始化与多进程锁。
pub fn ensure_embedded_assets(cfg: &AppConfig) -> Result<(), AppError> {
    let work_dir = get_work_dir(cfg)?;
    let _lock = acquire_assets_lock(&work_dir)?;
    std::fs::create_dir_all(work_dir.join("assets")).map_err(AppError::Io)?;
    Ok(())
}
