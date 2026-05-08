//! 跨平台基础适配：路径规范化、通用文件读写、进程/系统信息接口。

use std::path::{Path, PathBuf};

use super::error::AppError;

/// 规范化路径：展开 `~` 为当前用户 home、解析 `.`/`..`，失败时返回未 canonicalize 的路径。
///
/// # Arguments
/// * `path` - 原始路径字符串，支持以 `~` 或 `~/` 开头的 home 缩写。
///
/// # Returns
/// 规范化后的 [`PathBuf`]；若 `canonicalize` 失败（如路径不存在），则返回展开后的路径而不报错。
pub fn normalize_path(path: &str) -> Result<PathBuf, AppError> {
    let expanded = if path.starts_with("~") {
        let rest = path.trim_start_matches('~').trim_start_matches('/');
        if let Some(home) = dirs::home_dir() {
            home.join(rest)
        } else {
            PathBuf::from(path)
        }
    } else {
        PathBuf::from(path)
    };
    Ok(expanded.canonicalize().unwrap_or(expanded))
}

/// 以 UTF-8 读取文件全部内容。
///
/// # Arguments
/// * `path` - 文件路径。
///
/// # Errors
/// * [`AppError::Io`] - 文件不存在或读取失败。
/// * [`AppError::Config`] - 文件内容非合法 UTF-8 时返回（复用 Config 表示编码错误）。
pub fn read_file_utf8(path: &Path) -> Result<String, AppError> {
    let bytes = std::fs::read(path).map_err(AppError::Io)?;
    String::from_utf8(bytes).map_err(|e| AppError::Config(e.to_string()))
}

/// 原子写入文件：先写临时文件再重命名，避免写入中途崩溃导致文件损坏。
///
/// # Arguments
/// * `path` - 目标文件路径。
/// * `content` - 要写入的字节内容。
///
/// # Errors
/// * [`AppError::Config`] - 路径无父目录（如空路径）时返回。
/// * [`AppError::Io`] - 创建目录、写临时文件或重命名失败时返回。
pub fn write_file_atomic(path: &Path, content: &[u8]) -> Result<(), AppError> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("路径无父目录".to_string()))?;
    std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    let tmp = parent.join(format!(
        ".{}",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&tmp, content).map_err(AppError::Io)?;
    std::fs::rename(&tmp, path).map_err(AppError::Io)?;
    Ok(())
}

/// 当前进程工作目录（跨平台）。
///
/// # Errors
/// * [`AppError::Io`] - 获取当前目录失败时返回（如进程无权限或已被删除）。
#[allow(dead_code)]
pub fn current_dir() -> Result<PathBuf, AppError> {
    std::env::current_dir().map_err(AppError::Io)
}

/// 系统信息摘要，用于 doctor 等；平台差异由标准库常量提供。
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct SystemInfo {
    /// 操作系统标识，如 `macos`、`linux`、`windows`。
    pub os: String,
    /// 架构标识，如 `x86_64`、`aarch64`。
    pub arch: String,
}

/// 获取当前运行环境的系统信息（OS 与架构）。
#[allow(dead_code)]
pub fn system_info() -> SystemInfo {
    SystemInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    }
}

#[cfg(test)]
mod tests;
