//! bash 工具使用的 POSIX shell：调用方统一以 `<program> -c <command>` 执行。
//!
//! - Unix：`sh`。
//! - Windows：Git for Windows 的 `<Git 根>\bin\bash.exe`（启动器会把 `/usr/bin`、`/mingw64/bin`
//!   放进 PATH，非 login 模式下 ls/grep 也可用）。**绝不**按名字在 PATH 里找 `bash`：
//!   `C:\Windows\System32\bash.exe` 是 WSL，不是 Git Bash。找不到时 fail-closed，
//!   不回落到 cmd，避免权限层按 POSIX 语义检查、实际却交给另一种 shell 执行。

use std::path::PathBuf;

use crate::infra::error::AppError;

/// 返回 bash 工具使用的 shell 程序。
///
/// # Errors
/// * [`AppError::Primitive`] - Windows 上找不到 Git Bash（`bash.gitBashMissing`）。
#[cfg(unix)]
pub fn posix_shell() -> Result<PathBuf, AppError> {
    Ok(PathBuf::from("sh"))
}

/// 返回 bash 工具使用的 shell 程序；进程内只解析一次。
///
/// # Errors
/// * [`AppError::Primitive`] - 找不到 Git Bash（`bash.gitBashMissing`）。
#[cfg(windows)]
pub fn posix_shell() -> Result<PathBuf, AppError> {
    static GIT_BASH: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    GIT_BASH
        .get_or_init(|| {
            locate_git_bash(
                git_exec_path().as_deref(),
                |key| std::env::var_os(key),
                |path| path.is_file(),
            )
        })
        .clone()
        .ok_or_else(|| AppError::Primitive(crate::infra::i18n::tr("bash.gitBashMissing", &[])))
}

#[cfg(windows)]
fn git_exec_path() -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("--exec-path")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// 纯函数：由 `git --exec-path` 的输出与标准安装目录推导 Git Bash。
///
/// `exec_path` 形如 `C:/Program Files/Git/mingw64/libexec/git-core`，向上 3 级即 Git 根目录；
/// MinGit 没有 bash.exe，此时继续查 `%ProgramFiles%\Git`、`%LOCALAPPDATA%\Programs\Git`。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn locate_git_bash(
    exec_path: Option<&str>,
    env: impl Fn(&str) -> Option<std::ffi::OsString>,
    is_file: impl Fn(&std::path::Path) -> bool,
) -> Option<PathBuf> {
    let derived = exec_path
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .and_then(|path| PathBuf::from(path).ancestors().nth(3).map(PathBuf::from));
    let fallbacks = [
        env("ProgramFiles").map(|dir| PathBuf::from(dir).join("Git")),
        env("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("Programs").join("Git")),
    ];
    std::iter::once(derived)
        .chain(fallbacks)
        .flatten()
        .map(|root| root.join("bin").join("bash.exe"))
        .find(|candidate| is_file(candidate))
}

#[cfg(test)]
#[path = "tests/shell_test.rs"]
mod tests;
