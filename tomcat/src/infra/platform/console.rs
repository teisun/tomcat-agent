//! 控制台：进程启动时的终端初始化，以及终端输入期间的输出能力。

/// 用户正在输入时，能否从别的线程往终端打印（rustyline 的 `ExternalPrinter`）。
/// macOS 中文输入法在这条输入路径下更容易出现回显异常，所以关掉。
pub const ASYNC_PRINT_DURING_INPUT: bool = !cfg!(target_os = "macos");

/// CLI 启动时调用一次，准备好当前终端。Unix 不需要做任何事。
pub(crate) fn prepare_console() {
    #[cfg(windows)]
    {
        enable_virtual_terminal_output();
        restore_ctrl_c_input();
        stop_std_handle_inheritance();
    }
}

/// 让 Windows 控制台解释颜色/光标转义序列：Windows 10 的 conhost 需要程序自己开启
/// VT 处理，否则显示成 `←[32m`。输出被重定向或系统不支持时调用会失败，保持原样即可。
#[cfg(windows)]
fn enable_virtual_terminal_output() {
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
fn restore_ctrl_c_input() {
    // SAFETY: a null handler with FALSE only clears the inherited ignore flag.
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(None, 0);
    }
}

/// 让本进程从父进程继承来的 std 句柄不再被每个子进程隐式继承（相当于 Unix 的 CLOEXEC）。
/// 否则比 tomcat 活得久的后台任务会一直握着父进程给的管道，父进程等不到 EOF。
/// `Stdio::inherit()` 在创建子进程时会另外复制一份可继承句柄，不受影响。
#[cfg(windows)]
fn stop_std_handle_inheritance() {
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
