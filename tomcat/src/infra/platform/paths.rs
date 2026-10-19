//! 路径：home 目录、规范化、展示、比较规则。

use std::borrow::Cow;
use std::path::{Path, PathBuf, MAIN_SEPARATOR};

use crate::infra::error::AppError;

/// 系统默认的路径长度上限是否仍是 260 字符（Windows 的 MAX_PATH）。为 true 时，git 这类工具
/// 需要显式开启长路径支持。
pub const LEGACY_MAX_PATH: bool = cfg!(windows);

/// tomcat 比较路径时是否忽略大小写（只有 Windows 为 true）。
///
/// 这是 tomcat 的比较规则，不是"文件系统是否区分大小写"：macOS 默认的 APFS 也不区分，
/// 但一直按区分处理。
pub const PATH_COMPARE_IGNORES_CASE: bool = cfg!(windows);

/// 把路径分隔符统一成 `/`。Windows 上替换 `\`；Unix 上 `\` 是合法的文件名字符，原样借用。
pub fn to_slash(path: &str) -> Cow<'_, str> {
    if cfg!(windows) {
        Cow::Owned(path.replace('\\', "/"))
    } else {
        Cow::Borrowed(path)
    }
}

/// `target` 是否就是 `prefix` 或位于其下。只做字符串比较，不做 canonicalize（调用方负责），
/// 容忍结尾分隔符。[`PATH_COMPARE_IGNORES_CASE`] 时忽略大小写：目标目录尚不存在时
/// canonicalize 无法纠正大小写，而 NTFS 把 `.SSH` 与 `.ssh` 视为同一目录。
pub fn path_starts_with(target: &str, prefix: &str) -> bool {
    let (target, prefix) = if PATH_COMPARE_IGNORES_CASE {
        (
            Cow::Owned(target.to_lowercase()),
            Cow::Owned(prefix.to_lowercase()),
        )
    } else {
        (Cow::Borrowed(target), Cow::Borrowed(prefix))
    };
    let t = target.trim_end_matches(MAIN_SEPARATOR);
    let p = prefix.trim_end_matches(MAIN_SEPARATOR);
    if t == p {
        return true;
    }
    t.starts_with(&format!("{p}{MAIN_SEPARATOR}"))
}

/// 返回当前进程应使用的 HOME 目录。
///
/// 测试会临时覆写 `HOME` 指向隔离目录；这里优先读取进程环境变量，
/// 再回退到平台默认的 home 目录解析。
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
}

/// 解析符号链接并返回绝对路径；Windows 上去掉 `\\?\` 前缀（仅在去掉后语义不变时）。
///
/// 项目内统一用它代替 `std::fs::canonicalize`（`clippy.toml` 禁用了后者），
/// 保证展示给模型/界面的路径与参与前缀比较的路径是同一种写法。
///
/// # Errors
/// * [`std::io::Error`] - 路径不存在或无法访问。
pub fn canonicalize(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// 规范化路径：展开 `~` 为当前用户 home、解析 `.`/`..`，失败时返回未 canonicalize 的路径。
///
/// # Arguments
/// * `path` - 原始路径字符串，支持以 `~` 开头的 home 缩写。
///   `/` 在两平台均为分隔符；`\` 仅在 Windows 上作为分隔符，Unix 上保留为文件名字符。
///
/// # Returns
/// 规范化后的 [`PathBuf`]；若 `canonicalize` 失败（如路径不存在），则返回展开后的路径而不报错。
pub fn normalize_path(path: &str) -> Result<PathBuf, AppError> {
    let expanded = if path.starts_with("~") {
        let rest = path
            .trim_start_matches('~')
            .trim_start_matches(std::path::is_separator);
        if let Some(home) = home_dir() {
            home.join(rest)
        } else {
            PathBuf::from(path)
        }
    } else {
        PathBuf::from(path)
    };
    // 路径尚不存在时 canonicalize 会失败，原样返回会把 `C:\Users\x\.tomcat/a.toml` 这类混合分隔符展示给用户。
    #[cfg(windows)]
    let expanded: PathBuf = expanded.components().collect();
    Ok(canonicalize(&expanded).unwrap_or(expanded))
}

/// 把绝对路径格式化为对 HOME 友好的展示串：
/// - 位于 HOME 下 → `~/<rest>`（各级之间统一用 `/`）
/// - 恰好等于 HOME → `~`
/// - 其它路径 → 原样 `display()`
pub fn format_home_path(path: &Path) -> String {
    let Some(home) = home_dir() else {
        return path.display().to_string();
    };
    if path == home {
        return "~".to_string();
    }
    if let Ok(rest) = path.strip_prefix(&home) {
        if rest.as_os_str().is_empty() {
            return "~".to_string();
        }
        return format!("~/{}", to_slash(&rest.display().to_string()));
    }
    path.display().to_string()
}

#[cfg(test)]
#[path = "tests/paths_test.rs"]
mod tests;
