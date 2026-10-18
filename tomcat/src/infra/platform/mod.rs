//! 跨平台基础适配：路径规范化、通用文件读写、进程/系统信息接口。

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::error::AppError;

mod executable;
#[cfg(windows)]
mod process_job;
mod shell;
pub(crate) mod stdin;

pub use executable::find_executable;
#[cfg(windows)]
pub(crate) use executable::find_executable_in;
#[cfg(windows)]
pub use process_job::ProcessJob;
pub use shell::posix_shell;

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

/// 判断 `fs2` 的 `try_lock_*` 失败是否只是"锁被别人占着"。
///
/// Unix 返回 `EWOULDBLOCK`，Windows 返回 `ERROR_LOCK_VIOLATION`（其
/// `ErrorKind` 不是 `WouldBlock`），因此不能用 `ErrorKind` 判断。
pub(crate) fn is_lock_contended(error: &std::io::Error) -> bool {
    error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

/// 让 Windows 控制台解释颜色/光标转义序列：Windows 10 的 conhost 需要程序自己开启
/// VT 处理，否则显示成 `←[32m`。输出被重定向或系统不支持时调用会失败，保持原样即可。
#[cfg(windows)]
pub(crate) fn enable_virtual_terminal_output() {
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: the standard handle is owned by the process and only its console mode is
        // queried/updated; invalid or non-console handles make the calls fail harmlessly.
        unsafe {
            let handle = GetStdHandle(which);
            let mut mode = 0;
            if GetConsoleMode(handle, &mut mode) != 0 {
                SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }
}

/// 恢复 Ctrl+C 输入。"忽略 Ctrl+C" 会从父进程继承（父进程以新进程组启动时就是这样，
/// 例如从 Git Bash 运行），此时 chat 里的 Ctrl+C 既打断不了生成，也打断不了提问。
#[cfg(windows)]
pub(crate) fn restore_ctrl_c_input() {
    // SAFETY: a null handler with FALSE only clears the inherited ignore flag.
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(None, 0);
    }
}

/// 让本进程从父进程继承来的 std 句柄不再被每个子进程隐式继承（相当于 Unix 的 CLOEXEC）。
/// 否则比 tomcat 活得久的后台任务会一直握着父进程给的管道，父进程等不到 EOF。
/// `Stdio::inherit()` 在创建子进程时会另外复制一份可继承句柄，不受影响。
#[cfg(windows)]
pub(crate) fn stop_std_handle_inheritance() {
    use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: only the inherit flag of the process's own standard handle is cleared; a missing
        // or invalid handle makes the call fail harmlessly.
        unsafe {
            SetHandleInformation(GetStdHandle(which), HANDLE_FLAG_INHERIT, 0);
        }
    }
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
/// * `path` - 原始路径字符串，支持以 `~`、`~/` 或 `~\` 开头的 home 缩写。
///
/// # Returns
/// 规范化后的 [`PathBuf`]；若 `canonicalize` 失败（如路径不存在），则返回展开后的路径而不报错。
pub fn normalize_path(path: &str) -> Result<PathBuf, AppError> {
    let expanded = if path.starts_with("~") {
        let rest = path.trim_start_matches('~').trim_start_matches(['/', '\\']);
        if let Some(home) = home_dir() {
            home.join(rest)
        } else {
            PathBuf::from(path)
        }
    } else {
        PathBuf::from(path)
    };
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
        let rest = rest.display().to_string();
        #[cfg(windows)]
        let rest = rest.replace('\\', "/");
        return format!("~/{rest}");
    }
    path.display().to_string()
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
    write_file_atomic_with(path, |writer| {
        writer.write_all(content).map_err(AppError::Io)
    })
}

/// 流式版本的 [`write_file_atomic`]。调用方直接写入临时文件，随后仍通过 rename 原子替换
/// 目标文件；适合 JSONL 全文件重写，避免在内存里再拼出第二份完整字符串。
///
/// 任一步失败（如磁盘满）都会删除临时文件，目标文件保持原样。
pub fn write_file_atomic_with(
    path: &Path,
    write: impl FnOnce(&mut BufWriter<std::fs::File>) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config(crate::infra::i18n::tr("platform.noParent", &[])))?;
    std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&tmp)
        .map_err(AppError::Io)?;
    let result = (|| {
        let mut writer = BufWriter::new(file);
        write(&mut writer)?;
        writer.flush().map_err(AppError::Io)?;
        writer.get_ref().sync_all().map_err(AppError::Io)?;
        drop(writer);
        std::fs::rename(&tmp, path).map_err(AppError::Io)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// 把存放凭据的文件限制为仅当前用户可访问。
///
/// Unix 设为 0600。Windows 用 `%SystemRoot%\System32\icacls.exe`（不经 PATH）
/// 去掉继承的权限，只给 `USERDOMAIN\USERNAME` 完全控制：Entra ID 账户必须带
/// `AzureAD\` 前缀才能被识别，本机账户的 USERDOMAIN 就是计算机名。
///
/// # Errors
/// * [`AppError::Io`] - 设置权限或启动 icacls 失败。
/// * [`AppError::Config`] - Windows 上取不到 USERNAME，或 icacls 返回失败。
pub fn set_private_file_permissions(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    {
        let shown = path.display().to_string();
        let error = |key| AppError::Config(crate::infra::i18n::tr(key, &[("path", &shown)]));
        let username = std::env::var("USERNAME")
            .ok()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| error("platform.privateAclUser"))?;
        let account = match std::env::var("USERDOMAIN") {
            Ok(domain) if !domain.is_empty() => format!("{domain}\\{username}"),
            _ => username,
        };
        let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let status =
            std::process::Command::new(Path::new(&system_root).join(r"System32\icacls.exe"))
                .arg(path)
                .args(["/inheritance:r", "/grant:r"])
                .arg(format!("{account}:F"))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()?;
        if !status.success() {
            return Err(error("platform.privateAclFailed"));
        }
    }
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
