//! Windows 私有文件权限：用只含当前用户一条完全控制的受保护 DACL 整体替换原有权限，
//! 相当于 Unix 的 0600。
//!
//! 只去掉继承的权限并不够：父目录没有可继承权限时，新文件会按令牌的默认 DACL
//! 带上 SYSTEM、登录会话等显式授权；用户手工给过的授权也是显式的。整体替换才能保证
//! 文件上只剩当前用户。

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;

use windows_sys::Win32::Security::Authorization::{SetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    AddAccessAllowedAce, GetLengthSid, GetTokenInformation, InitializeAcl, TokenUser,
    ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, DACL_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub(super) fn current_user_acl() -> io::Result<Vec<u32>> {
    let token_user = current_token_user()?;
    // SAFETY: 缓冲区由 GetTokenInformation(TokenUser) 填充，开头就是 TOKEN_USER，且按 u64 对齐。
    let sid = unsafe { (*token_user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    // SAFETY: `sid` 指向 `token_user` 内部，在本函数内一直有效。
    let sid_len = unsafe { GetLengthSid(sid) } as usize;
    // ACCESS_ALLOWED_ACE 末尾的 SidStart 占位 u32 由真实 SID 取代。
    let acl_len = size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>() + sid_len;
    let mut acl_buffer = vec![0u32; acl_len.div_ceil(size_of::<u32>())];
    let acl = acl_buffer.as_mut_ptr().cast::<ACL>();
    // SAFETY: `acl` 指向至少 `acl_len` 字节、按 DWORD 对齐的缓冲区。
    if unsafe { InitializeAcl(acl, acl_len as u32, ACL_REVISION) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ACL 已初始化且留足一条 ACE 的空间；`sid` 有效。
    if unsafe { AddAccessAllowedAce(acl, ACL_REVISION, FILE_ALL_ACCESS, sid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(acl_buffer)
}

pub(super) fn restrict_to_current_user(path: &Path) -> io::Result<()> {
    let mut acl_buffer = current_user_acl()?;
    let acl = acl_buffer.as_mut_ptr().cast::<ACL>();
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` 以 NUL 结尾；只设置 DACL，属主、属组与 SACL 传 null 表示不改。
    let status = unsafe {
        SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl,
            std::ptr::null(),
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(())
}

/// 读取当前进程令牌的 TOKEN_USER（含 SID），缓冲区按 u64 对齐以容纳其中的指针。
fn current_token_user() -> io::Result<Vec<u64>> {
    // SAFETY: query of the current process pseudo-handle; it must not be closed.
    token_user_for_process(unsafe { GetCurrentProcess() })
}

pub(super) fn verify_process_user(pid: u32) -> io::Result<()> {
    use windows_sys::Win32::Security::EqualSid;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: query-only access to the server PID supplied by the kernel.
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: takes ownership of the successfully opened process handle.
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    let peer = token_user_for_process(process.as_raw_handle())?;
    let own = current_token_user()?;
    // SAFETY: both aligned TOKEN_USER buffers and their SID payloads remain live.
    if unsafe {
        EqualSid(
            (*peer.as_ptr().cast::<TOKEN_USER>()).User.Sid,
            (*own.as_ptr().cast::<TOKEN_USER>()).User.Sid,
        )
    } == 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "local service belongs to another user",
        ));
    }
    Ok(())
}

fn token_user_for_process(process: windows_sys::Win32::Foundation::HANDLE) -> io::Result<Vec<u64>> {
    let mut raw = std::ptr::null_mut();
    // SAFETY: the caller keeps the process handle live; `raw` is an output parameter.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` 是刚打开的令牌句柄，所有权只交给这里。
    let token = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut len = 0u32;
    // 第一次调用只为取得所需长度，按约定以 ERROR_INSUFFICIENT_BUFFER 失败。
    // SAFETY: 缓冲区为 null、长度为 0 时只写 `len`。
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut len,
        )
    };
    let mut buffer = vec![0u64; (len as usize).div_ceil(size_of::<u64>())];
    // SAFETY: 缓冲区至少 `len` 字节。
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            len,
            &mut len,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(buffer)
}
