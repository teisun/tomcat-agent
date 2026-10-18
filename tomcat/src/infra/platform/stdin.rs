//! 可取消的 stdin 单行读取：每 50ms 检查一次是否该停下；这是检查间隔，不是超时。
//!
//! 不预读：管道/文件每次读 1 字节，控制台每次读 1 行；停下后剩余输入留给下一个
//! 读取者（CLI 主输入框）。同一时刻只允许一个读取者。

use std::sync::{Mutex, OnceLock};

const POLL_INTERVAL_MS: u32 = 50;

/// 读取没有拿到一整行。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReadLineError<T> {
    /// `stop` 返回了 `Some`。
    Stopped(T),
    /// 输入端关闭（EOF、Windows 控制台 Ctrl+Z）、读取失败或内容不是合法文本。
    Disconnected,
}

/// 输入端：Unix 是文件描述符，Windows 是句柄。
#[cfg(unix)]
pub(crate) type Input<'a> = std::os::fd::BorrowedFd<'a>;
#[cfg(windows)]
pub(crate) type Input<'a> = std::os::windows::io::BorrowedHandle<'a>;

/// 从进程 stdin 读一行（含结尾 `\n`）。
pub(crate) fn read_line_cancellable<T>(
    stop: impl FnMut() -> Option<T>,
) -> Result<String, ReadLineError<T>> {
    let stdin = std::io::stdin();
    #[cfg(unix)]
    let input = std::os::fd::AsFd::as_fd(&stdin);
    #[cfg(windows)]
    let input = std::os::windows::io::AsHandle::as_handle(&stdin);
    read_line_from(input, stop)
}

/// 同 [`read_line_cancellable`]，但从指定输入端读。
pub(crate) fn read_line_from<T>(
    input: Input<'_>,
    mut stop: impl FnMut() -> Option<T>,
) -> Result<String, ReadLineError<T>> {
    static OWNER: OnceLock<Mutex<()>> = OnceLock::new();
    let _owner = OWNER
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    imp::read_line(input, &mut stop)
}

#[cfg(unix)]
mod imp {
    use std::os::fd::AsRawFd;

    use super::{Input, ReadLineError, POLL_INTERVAL_MS};

    pub(super) fn read_line<T>(
        input: Input<'_>,
        stop: &mut impl FnMut() -> Option<T>,
    ) -> Result<String, ReadLineError<T>> {
        let fd = input.as_raw_fd();
        let mut bytes = Vec::new();
        loop {
            if let Some(value) = stop() {
                return Err(ReadLineError::Stopped(value));
            }
            let mut descriptor = libc::pollfd {
                fd,
                events: libc::POLLIN | libc::POLLHUP,
                revents: 0,
            };
            // SAFETY: descriptor points to one initialized pollfd for the whole call.
            let ready = unsafe { libc::poll(&mut descriptor, 1, POLL_INTERVAL_MS as i32) };
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(ReadLineError::Disconnected);
            }
            if ready == 0 {
                continue;
            }
            let mut byte = 0u8;
            // SAFETY: byte is writable and fd stays open for the borrowed lifetime.
            let count = unsafe { libc::read(fd, (&mut byte as *mut u8).cast::<libc::c_void>(), 1) };
            if count <= 0 {
                return Err(ReadLineError::Disconnected);
            }
            bytes.push(byte);
            if byte == b'\n' {
                return String::from_utf8(bytes).map_err(|_| ReadLineError::Disconnected);
            }
        }
    }
}

/// Windows：把进程 stdin 交给调用者独占，进程自己的 stdin 换成 NUL。
///
/// 一个线程阻塞在同步管道的 ReadFile 上时，继承同一管道的子进程会在启动时卡住，
/// 直到管道里来了下一条数据。serve 一直这样读协议输入，所以子进程只能继承 NUL；
/// 此后 `Stdio::inherit()` 与 `std::io::stdin()` 拿到的都是 NUL。
#[cfg(windows)]
pub(crate) fn take_process_stdin() -> std::io::Result<std::fs::File> {
    use std::os::windows::io::{FromRawHandle, IntoRawHandle};

    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{GetStdHandle, SetStdHandle, STD_INPUT_HANDLE};

    // SAFETY: GetStdHandle has no preconditions.
    let original = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if original.is_null() || original == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "process has no stdin",
        ));
    }
    let nul = std::fs::File::open("NUL")?.into_raw_handle();
    // SAFETY: `nul` is an open handle the process keeps as its stdin from now on.
    if unsafe { SetStdHandle(STD_INPUT_HANDLE, nul) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the process no longer refers to the original handle; this File is its only owner.
    Ok(unsafe { std::fs::File::from_raw_handle(original) })
}

/// std 的控制台读取在 Ctrl+C 中止（ERROR_OPERATION_ABORTED）后会自动重试，永远不会
/// 因 Ctrl+C 返回，所以这里自己调用 ReadConsoleW，中止后回到循环检查 `stop`。
#[cfg(windows)]
mod imp {
    use std::io::Read;
    use std::mem::ManuallyDrop;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};

    use windows_sys::Win32::Foundation::{
        GetLastError, SetLastError, ERROR_OPERATION_ABORTED, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Storage::FileSystem::{GetFileType, FILE_TYPE_CHAR, FILE_TYPE_PIPE};
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, PeekConsoleInputW, ReadConsoleInputW, ReadConsoleW, INPUT_RECORD, KEY_EVENT,
    };
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    use super::{Input, ReadLineError, POLL_INTERVAL_MS};

    const CTRL_Z: u16 = 0x1a;

    pub(super) fn read_line<T>(
        input: Input<'_>,
        stop: &mut impl FnMut() -> Option<T>,
    ) -> Result<String, ReadLineError<T>> {
        let handle = input.as_raw_handle() as HANDLE;
        let mut mode = 0;
        // SAFETY: handle stays open for the borrowed lifetime; mode is writable.
        let kind = unsafe { GetFileType(handle) };
        if kind == FILE_TYPE_CHAR && unsafe { GetConsoleMode(handle, &mut mode) } != 0 {
            return read_console_line(handle, stop);
        }
        // SAFETY: ManuallyDrop never closes the borrowed handle.
        let mut file =
            ManuallyDrop::new(unsafe { std::fs::File::from_raw_handle(input.as_raw_handle()) });
        read_byte_line(&mut file, (kind == FILE_TYPE_PIPE).then_some(handle), stop)
    }

    fn read_console_line<T>(
        handle: HANDLE,
        stop: &mut impl FnMut() -> Option<T>,
    ) -> Result<String, ReadLineError<T>> {
        loop {
            if let Some(value) = stop() {
                return Err(ReadLineError::Stopped(value));
            }
            // SAFETY: handle is a console input handle borrowed for this call.
            match unsafe { WaitForSingleObject(handle, POLL_INTERVAL_MS) } {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => continue,
                _ => return Err(ReadLineError::Disconnected),
            }
            if !next_event_types_a_character(handle)? {
                continue;
            }
            if let Some(line) = read_console_until_newline(handle)? {
                return Ok(line);
            }
        }
    }

    /// 只有"按下了产生字符的键"才值得进入 ReadConsoleW；松开按键、窗口缩放、焦点等
    /// 事件直接丢弃，否则会带着空缓冲区卡在 ReadConsoleW 里，Ctrl+C 之外无法打断。
    fn next_event_types_a_character<T>(handle: HANDLE) -> Result<bool, ReadLineError<T>> {
        let mut record = INPUT_RECORD::default();
        let mut count = 0;
        // SAFETY: record holds exactly one INPUT_RECORD; count is writable.
        if unsafe { PeekConsoleInputW(handle, &mut record, 1, &mut count) } == 0 {
            return Err(ReadLineError::Disconnected);
        }
        if count == 0 {
            return Ok(false);
        }
        // SAFETY: the KeyEvent union member is valid when EventType is KEY_EVENT.
        let typed = u32::from(record.EventType) == KEY_EVENT
            && unsafe {
                record.Event.KeyEvent.bKeyDown != 0 && record.Event.KeyEvent.uChar.UnicodeChar != 0
            };
        if !typed {
            // SAFETY: same buffer contract as the peek above.
            if unsafe { ReadConsoleInputW(handle, &mut record, 1, &mut count) } == 0 {
                return Err(ReadLineError::Disconnected);
            }
        }
        Ok(typed)
    }

    /// 读到换行返回整行；被 Ctrl+C 中止返回 `None`（控制台已丢弃这行）。
    fn read_console_until_newline<T>(handle: HANDLE) -> Result<Option<String>, ReadLineError<T>> {
        let mut units = Vec::new();
        loop {
            let mut buffer = [0u16; 256];
            let mut read = 0;
            // SAFETY: buffer has `len` writable UTF-16 units; read is writable.
            let ok = unsafe {
                SetLastError(0);
                ReadConsoleW(
                    handle,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    &mut read,
                    std::ptr::null(),
                )
            };
            if read == 0 && unsafe { GetLastError() } == ERROR_OPERATION_ABORTED {
                return Ok(None);
            }
            if ok == 0 || read == 0 {
                return Err(ReadLineError::Disconnected);
            }
            units.extend_from_slice(&buffer[..read as usize]);
            if units.first() == Some(&CTRL_Z) {
                return Err(ReadLineError::Disconnected);
            }
            if units.last() == Some(&u16::from(b'\n')) {
                let line = String::from_utf16(&units).map_err(|_| ReadLineError::Disconnected)?;
                return Ok(Some(match line.strip_suffix("\r\n") {
                    Some(text) => format!("{text}\n"),
                    None => line,
                }));
            }
        }
    }

    /// 管道（`pipe` 为 Some）先用 PeekNamedPipe 确认有数据再读，避免阻塞；文件直接读。
    fn read_byte_line<T>(
        file: &mut std::fs::File,
        pipe: Option<HANDLE>,
        stop: &mut impl FnMut() -> Option<T>,
    ) -> Result<String, ReadLineError<T>> {
        let mut bytes = Vec::new();
        loop {
            if let Some(value) = stop() {
                return Err(ReadLineError::Stopped(value));
            }
            if let Some(handle) = pipe {
                let mut available = 0;
                // SAFETY: only the available-bytes out pointer is used; others may be null.
                let ok = unsafe {
                    PeekNamedPipe(
                        handle,
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null_mut(),
                        &mut available,
                        std::ptr::null_mut(),
                    )
                };
                if ok == 0 {
                    return Err(ReadLineError::Disconnected);
                }
                if available == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(u64::from(
                        POLL_INTERVAL_MS,
                    )));
                    continue;
                }
            }
            let mut byte = [0u8; 1];
            match file.read(&mut byte) {
                Ok(1) => {}
                _ => return Err(ReadLineError::Disconnected),
            }
            bytes.push(byte[0]);
            if byte[0] == b'\n' {
                return String::from_utf8(bytes).map_err(|_| ReadLineError::Disconnected);
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/stdin_test.rs"]
mod tests;
