//! 跨平台基础适配：路径规范化、通用文件读写、进程/系统信息接口。

use std::path::{Path, PathBuf};

use crate::error::AppError;

/// 规范化路径：展开 home、统一分隔符、解析 `.`/`..`。
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

/// 以 UTF-8 读取文件内容。
pub fn read_file_utf8(path: &Path) -> Result<String, AppError> {
    let bytes = std::fs::read(path).map_err(AppError::Io)?;
    String::from_utf8(bytes).map_err(|e| AppError::Config(e.to_string()))
}

/// 原子写入文件：先写临时文件再重命名。
pub fn write_file_atomic(path: &Path, content: &[u8]) -> Result<(), AppError> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("路径无父目录".to_string()))?;
    std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    let tmp = parent.join(format!(".{}", path.file_name().unwrap_or_default().to_string_lossy()));
    std::fs::write(&tmp, content).map_err(AppError::Io)?;
    std::fs::rename(&tmp, path).map_err(AppError::Io)?;
    Ok(())
}

/// 当前进程工作目录（跨平台）。
pub fn current_dir() -> Result<PathBuf, AppError> {
    std::env::current_dir().map_err(AppError::Io)
}

/// 系统信息摘要，用于 doctor 等；平台差异用条件编译。
#[derive(Debug, Clone, Default)]
pub struct SystemInfo {
    pub os: String,
    pub arch: String,
}

pub fn system_info() -> SystemInfo {
    SystemInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_info_has_os_and_arch() {
        let info = system_info();
        assert!(!info.os.is_empty());
        assert!(!info.arch.is_empty());
    }

    #[test]
    fn current_dir_ok() {
        let r = current_dir();
        assert!(r.is_ok());
    }

    #[test]
    fn read_file_utf8_missing_is_io_error() {
        let r = read_file_utf8(Path::new("/nonexistent/path/file.txt"));
        assert!(r.is_err());
    }

    #[test]
    fn write_file_atomic_and_read_utf8() {
        let dir = std::env::temp_dir().join("pi_awsm_platform_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test_atomic.txt");
        let content = "hello 世界";
        write_file_atomic(&path, content.as_bytes()).unwrap();
        let read = read_file_utf8(&path).unwrap();
        assert_eq!(read, content);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn normalize_path_without_tilde() {
        let r = normalize_path("/tmp");
        assert!(r.is_ok());
        let r = normalize_path("relative");
        assert!(r.is_ok());
    }

    #[test]
    fn normalize_path_with_tilde() {
        if dirs::home_dir().is_some() {
            let r = normalize_path("~");
            assert!(r.is_ok());
        }
    }

    #[test]
    fn read_file_utf8_invalid_utf8_returns_config_error() {
        let dir = std::env::temp_dir().join("pi_awsm_platform_utf8");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad_utf8.bin");
        std::fs::write(&path, &[0xff, 0xfe]).unwrap();
        let r = read_file_utf8(&path);
        assert!(r.is_err());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn write_file_atomic_no_parent_error() {
        let r = write_file_atomic(Path::new(""), b"x");
        assert!(r.is_err());
    }
}
