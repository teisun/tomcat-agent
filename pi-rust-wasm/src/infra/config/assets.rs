//! 内嵌资源管理：SHA-256 校验、原子写入、文件锁与资源释放。

use std::path::{Path, PathBuf};

use include_dir::{include_dir, Dir};
use sha2::{Digest, Sha256};

use super::super::error::AppError;
use super::load::get_work_dir;
use super::types::AppConfig;

// ---------------------------------------------------------------------------
// Embedded resources & compile-time SHA-256
// ---------------------------------------------------------------------------

const EMBEDDED_QUICKJS_WASM: &[u8] = include_bytes!("../../../assets/wasm/wasmedge_quickjs.wasm");
static EMBEDDED_MODULES: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets/modules");

pub(super) const EMBEDDED_WASM_SHA256: &str = env!("EMBEDDED_WASM_SHA256");
pub(super) const EMBEDDED_MODULES_SHA256: &str = env!("EMBEDDED_MODULES_SHA256");

// ---------------------------------------------------------------------------
// SHA-256 helpers
// ---------------------------------------------------------------------------

pub(super) fn compute_file_sha256(path: &Path) -> Result<String, AppError> {
    let data = std::fs::read(path).map_err(AppError::Io)?;
    Ok(format!("{:x}", Sha256::digest(&data)))
}

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

// ---------------------------------------------------------------------------
// Atomic write + file locking (6.6)
// ---------------------------------------------------------------------------

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
                    "资源锁超时（10s），请检查是否有其他 pi 进程卡住，或手动删除 ~/.pi_/assets/.lock"
                        .to_string(),
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Embedded asset extraction (6.2, 6.3)
// ---------------------------------------------------------------------------

pub(super) fn extract_wasm_if_needed(work_dir: &Path) -> Result<PathBuf, AppError> {
    let target = work_dir
        .join("assets")
        .join("wasm")
        .join("wasmedge_quickjs.wasm");
    if target.exists() && !EMBEDDED_WASM_SHA256.is_empty() {
        if let Ok(disk_sha) = compute_file_sha256(&target) {
            if disk_sha == EMBEDDED_WASM_SHA256 {
                return Ok(target);
            }
        }
    }
    std::fs::create_dir_all(target.parent().unwrap()).map_err(AppError::Io)?;
    write_atomic(&target, EMBEDDED_QUICKJS_WASM)?;
    Ok(target)
}

fn extract_modules_if_needed(work_dir: &Path) -> Result<PathBuf, AppError> {
    let target_dir = work_dir.join("assets").join("modules");
    if target_dir.is_dir() && !EMBEDDED_MODULES_SHA256.is_empty() {
        if let Ok(disk_sha) = compute_dir_sha256(&target_dir) {
            if disk_sha == EMBEDDED_MODULES_SHA256 {
                return Ok(target_dir);
            }
        }
    }
    extract_include_dir(&EMBEDDED_MODULES, &target_dir)?;
    Ok(target_dir)
}

fn extract_include_dir(dir: &Dir<'_>, base_target: &Path) -> Result<(), AppError> {
    std::fs::create_dir_all(base_target).map_err(AppError::Io)?;
    for file in dir.files() {
        let dest = base_target.join(file.path());
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(AppError::Io)?;
        }
        std::fs::write(&dest, file.contents()).map_err(AppError::Io)?;
    }
    for sub in dir.dirs() {
        extract_include_dir(sub, base_target)?;
    }
    Ok(())
}

fn write_versions_json(work_dir: &Path) -> Result<(), AppError> {
    let versions = serde_json::json!({
        "wasm_sha256": EMBEDDED_WASM_SHA256,
        "modules_sha256": EMBEDDED_MODULES_SHA256,
        "extracted_at": chrono::Utc::now().to_rfc3339(),
    });
    let content =
        serde_json::to_string_pretty(&versions).map_err(|e| AppError::Config(e.to_string()))?;
    let path = work_dir.join("assets").join(".versions.json");
    write_atomic(&path, content.as_bytes())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Unified entry point (6.4)
// ---------------------------------------------------------------------------

/// 确保内嵌资源已释放到 `work_dir/assets/`。
/// 在 `ensure_work_dir_structure` 之后、正式业务逻辑之前调用。
/// 通过文件锁保证多进程安全；SHA-256 比对避免重复写入。
pub fn ensure_embedded_assets(cfg: &AppConfig) -> Result<(), AppError> {
    let work_dir = get_work_dir(cfg)?;
    let _lock = acquire_assets_lock(&work_dir)?;
    extract_wasm_if_needed(&work_dir)?;
    extract_modules_if_needed(&work_dir)?;
    write_versions_json(&work_dir)?;
    Ok(())
}
