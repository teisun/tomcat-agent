//! 文件：读写、原子替换、私有权限、文件身份与权限位、锁。

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::infra::error::AppError;

/// 空设备的路径（写入即丢弃、读取即 EOF）。
pub const NULL_DEVICE: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

/// 判断 `fs2` 的 `try_lock_*` 失败是否只是"锁被别人占着"。
///
/// Unix 返回 `EWOULDBLOCK`，Windows 返回 `ERROR_LOCK_VIOLATION`（其
/// `ErrorKind` 不是 `WouldBlock`），因此不能用 `ErrorKind` 判断。
pub(crate) fn is_lock_contended(error: &std::io::Error) -> bool {
    error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
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
    write_atomic(path, false, write)
}

/// 原子写入只有当前用户能读写的文件（存放凭据）。
///
/// 临时文件从创建那一刻起就是私有的，写入任何字节之前别的用户都读不到；改名后权限跟着文件走，
/// 不必再设一次。
///
/// # Errors
/// 同 [`write_file_atomic`]；另外 Windows 上设置 DACL 失败时返回 [`AppError::Config`]。
pub fn write_private_file_atomic(path: &Path, content: &[u8]) -> Result<(), AppError> {
    write_atomic(path, true, |writer| {
        writer.write_all(content).map_err(AppError::Io)
    })
}

fn write_atomic(
    path: &Path,
    private: bool,
    write: impl FnOnce(&mut BufWriter<std::fs::File>) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config(crate::infra::i18n::tr("platform.noParent", &[])))?;
    std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    if private {
        set_create_mode(&mut options, 0o600);
    }
    let file = options.open(&tmp).map_err(AppError::Io)?;
    let result = (|| {
        // Windows 没有创建时的权限位：在写入第一个字节之前换掉 DACL。
        #[cfg(windows)]
        if private {
            set_private_file_permissions(&tmp)?;
        }
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

/// 只读打开文件或目录，最后一级是链接时不跟随、直接报 IO 错误。
///
/// Unix 用 `O_NOFOLLOW`（链接报 ELOOP）。Windows 打开链接本身而不是目标，再拒绝任何
/// reparse point（符号链接、junction 等）；BACKUP_SEMANTICS 是打开目录句柄所必需的。
pub fn open_no_follow(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        use windows_sys::Win32::Foundation::ERROR_STOPPED_ON_SYMLINK;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        };
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::from_raw_os_error(
                ERROR_STOPPED_ON_SYMLINK as i32,
            ));
        }
        Ok(file)
    }
}

/// 两个句柄是否指向同一个文件系统对象（Unix 比 dev + ino，Windows 比卷序列号 + 文件索引）。
pub fn same_file(left: &File, right: &File) -> io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let (left, right) = (left.metadata()?, right.metadata()?);
        Ok(left.dev() == right.dev() && left.ino() == right.ino())
    }
    #[cfg(windows)]
    {
        Ok(file_identity(left)? == file_identity(right)?)
    }
}

#[cfg(windows)]
fn file_identity(file: &File) -> io::Result<(u32, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: 句柄在 `file` 存活期间有效，`info` 是合法的输出指针。
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Ok((info.dwVolumeSerialNumber, index))
}

/// Unix 上返回完整的 `st_mode`（含文件类型位）；Windows 没有权限位，返回 `None`。
pub fn mode_bits(metadata: &std::fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(metadata.mode())
    }
    #[cfg(windows)]
    {
        let _ = metadata;
        None
    }
}

/// 一个文件的权限快照：可以序列化保存，之后还原到另一个文件上。
///
/// Unix 记录完整权限位（`mode`），Windows 只有只读属性（`readonly`）；字段名属于已落盘格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionSnapshot {
    #[cfg(unix)]
    mode: u32,
    #[cfg(not(unix))]
    readonly: bool,
}

impl PermissionSnapshot {
    pub fn capture(permissions: std::fs::Permissions) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            Self {
                mode: permissions.mode(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                readonly: permissions.readonly(),
            }
        }
    }

    pub fn restore(self, file: &File) -> io::Result<()> {
        #[cfg(unix)]
        let permissions = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::Permissions::from_mode(self.mode)
        };
        #[cfg(not(unix))]
        let permissions = {
            let mut permissions = file.metadata()?.permissions();
            permissions.set_readonly(self.readonly);
            permissions
        };
        file.set_permissions(permissions)
    }
}

/// 设置新建文件的权限位（仍受 umask 约束）。Windows 没有权限位，不做事。
pub fn set_create_mode(options: &mut std::fs::OpenOptions, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    #[cfg(windows)]
    let _ = (options, mode);
}

/// 把存放凭据的文件限制为仅当前用户可访问。
///
/// Unix 设为 0600。Windows 把 DACL 整体换成只给当前令牌用户完全控制的受保护 DACL，
/// 继承来的与文件上已有的显式授权都会去掉。
///
/// # Errors
/// * [`AppError::Io`] - Unix 上设置权限失败。
/// * [`AppError::Config`] - Windows 上读取令牌或设置 DACL 失败。
pub fn set_private_file_permissions(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    super::private_acl::restrict_to_current_user(path).map_err(|error| {
        AppError::Config(crate::infra::i18n::tr(
            "platform.privateAclFailed",
            &[
                ("path", &path.display().to_string()),
                ("error", &error.to_string()),
            ],
        ))
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/fs_test.rs"]
mod tests;
