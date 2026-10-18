//! 操作系统层面的事实与设施：系统包管理器、环境变量变更何时生效、跟踪日志的命令、
//! 默认编辑器、打开浏览器。

use std::io;
use std::path::Path;

use super::find_executable;

/// 没有设置 `EDITOR` 时使用的编辑器。
pub const DEFAULT_EDITOR: &str = if cfg!(windows) { "notepad" } else { "vi" };

/// 用系统默认浏览器打开 `url`，不等浏览器退出。
///
/// # Errors
/// * [`io::ErrorKind::Unsupported`] - 不知道这个系统怎么打开浏览器。
/// * 其它 [`io::Error`] - 打开命令启动失败。
pub fn open_url(url: &str) -> io::Result<()> {
    let opener = match std::env::consts::OS {
        "macos" => "open",
        "linux" => "xdg-open",
        "windows" => "explorer.exe",
        _ => return Err(io::ErrorKind::Unsupported.into()),
    };
    std::process::Command::new(opener)
        .arg(url)
        .spawn()
        .map(|_| ())
}

/// 本机可用于自动安装软件的系统包管理器。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    /// Termux 的 `pkg`。
    TermuxPkg,
    /// macOS 的 Homebrew。
    Brew,
    /// Windows 的 winget。
    Winget,
    AptGet,
    Dnf,
    Pacman,
}

/// 系统包管理器装好的程序要在新开的终端里才能找到：Windows 把 PATH 改在注册表里，运行中的进程
/// 及其终端看不到；Unix 包管理器装进已经在 PATH 里的目录，当前会话即可使用。
pub const PATH_CHANGES_NEED_RESTART: bool = cfg!(windows);

/// 在用户终端里持续跟踪一个日志文件的命令，后面接文件路径。
pub const FOLLOW_FILE_COMMAND: &str = if cfg!(windows) {
    "Get-Content -Wait"
} else {
    "tail -f"
};

/// 按固定顺序找本机的包管理器：Termux pkg →（Android 到此为止）→ macOS brew → Windows winget
/// → apt-get → dnf → pacman。都没有时返回 `None`。
pub fn system_package_manager() -> Option<PackageManager> {
    detect_package_manager(std::env::consts::OS, is_termux(), |program| {
        find_executable(&[program]).is_some()
    })
}

fn detect_package_manager(
    os: &str,
    termux: bool,
    has_program: impl Fn(&str) -> bool,
) -> Option<PackageManager> {
    if termux && has_program("pkg") {
        return Some(PackageManager::TermuxPkg);
    }
    if os == "android" {
        return None;
    }
    if os == "macos" && has_program("brew") {
        return Some(PackageManager::Brew);
    }
    if os == "windows" && has_program("winget") {
        return Some(PackageManager::Winget);
    }
    [
        ("apt-get", PackageManager::AptGet),
        ("dnf", PackageManager::Dnf),
        ("pacman", PackageManager::Pacman),
    ]
    .into_iter()
    .find(|(program, _)| has_program(program))
    .map(|(_, manager)| manager)
}

fn is_termux() -> bool {
    std::env::var_os("TERMUX_VERSION").is_some()
        || Path::new("/data/data/com.termux/files/usr/bin/pkg").exists()
}

#[cfg(test)]
#[path = "tests/system_test.rs"]
mod tests;
