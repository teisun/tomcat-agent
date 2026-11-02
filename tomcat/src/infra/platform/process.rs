//! 子进程：整棵进程树的结束方式、退出码、系统启动进程必需的环境变量。
//!
//! bash 后台任务和 MCP 子进程都需要"一次结束命令派生的全部进程"，不论孙进程是否已成孤儿。

use std::io;
use std::process::ExitStatus;

/// 清空环境变量启动子进程时，系统仍要求保留的变量（调用方自己需要的 PATH、HOME 不在其中）。
///
/// Windows 取自官方 MCP SDK 的 win32 清单（`stdio.ts` 的 `DEFAULT_INHERITED_ENV_VARS`）：
/// 缺了 SYSTEMROOT 等变量，Node 写的服务一启动就崩溃。
#[cfg(unix)]
pub const CHILD_STARTUP_ENV_VARS: &[&str] = &[];
#[cfg(windows)]
pub const CHILD_STARTUP_ENV_VARS: &[&str] = &[
    "APPDATA",
    "COMSPEC",
    "HOMEDRIVE",
    "HOMEPATH",
    "LOCALAPPDATA",
    "PATHEXT",
    "PROCESSOR_ARCHITECTURE",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMW6432",
    "SYSTEMDRIVE",
    "SYSTEMROOT",
    "TEMP",
    "USERNAME",
    "USERPROFILE",
    "WINDIR",
];

/// 包装 `command`，让它派生的整棵进程树在包装被丢弃（或本进程退出）时一起结束。
///
/// Windows 上 `kill_on_drop` 只杀得到 `npx.cmd` 外层的 cmd.exe，真正的服务（node、它启动的浏览器）
/// 继续握着 stdout 管道；tokio 在 Windows 上用阻塞线程读子进程管道，运行时关闭会一直等它们退出。
/// 所以放进关闭即终止的 Job。Unix 上由调用方设置的 `kill_on_drop` 结束子进程，保持原样。
pub fn kill_tree_on_drop(command: tokio::process::Command) -> process_wrap::tokio::CommandWrap {
    #[cfg_attr(unix, allow(unused_mut))]
    let mut command = process_wrap::tokio::CommandWrap::from(command);
    #[cfg(windows)]
    command
        .wrap(process_wrap::tokio::KillOnDrop)
        .wrap(process_wrap::tokio::JobObject);
    command
}

/// 一棵可以整体结束的进程树：Unix 是新进程组，Windows 是独占的 Job Object。
/// 丢弃它不会结束任何进程。
pub struct ProcessTree {
    #[cfg(unix)]
    group: Option<u32>,
    #[cfg(windows)]
    job: super::process_job::ProcessJob,
}

impl ProcessTree {
    /// 启动 `command`，子进程从第一条指令起就在树内，它派生的后代全部自动加入。
    pub fn spawn(
        command: &mut tokio::process::Command,
    ) -> io::Result<(tokio::process::Child, Self)> {
        #[cfg(unix)]
        {
            command.process_group(0);
            let child = command.spawn()?;
            let group = child.id();
            Ok((child, Self { group }))
        }
        #[cfg(windows)]
        {
            let (child, job) = super::process_job::ProcessJob::spawn(command)?;
            Ok((child, Self { job }))
        }
    }

    /// 结束树内所有进程；树里已经没有进程时同样成功。
    pub fn kill(&self) -> io::Result<()> {
        #[cfg(unix)]
        {
            let Some(group) = self.group else {
                return Ok(());
            };
            // SAFETY: POSIX 信号 API，参数是 spawn 时创建的进程组 ID。
            if unsafe { libc::killpg(group as libc::pid_t, libc::SIGKILL) } == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            // EPERM：组里没有一个进程能收到信号。macOS 上组里只剩已退出、未回收的进程时
            // 就报这个而不是 ESRCH；和 ESRCH 一样，再发也杀不到任何进程。
            if matches!(error.raw_os_error(), Some(libc::ESRCH | libc::EPERM)) {
                Ok(())
            } else {
                Err(error)
            }
        }
        #[cfg(windows)]
        {
            self.job.terminate()
        }
    }

    /// 等待子进程的一方没等到结束就被丢弃（只发生在运行时关闭）时调用。
    ///
    /// Windows 上 `kill_on_drop` 只杀得到 Git Bash 启动器（`bin\bash.exe`），真正执行命令的
    /// `usr\bin\bash.exe` 会继续运行并占住输出管道，而运行时关闭要等这些管道读取结束，
    /// 所以结束整棵树。Unix 没有这两个问题，由 `kill_on_drop` 结束直接子进程即可。
    pub fn kill_if_abandoned(&self) {
        #[cfg(windows)]
        {
            let _ = self.kill();
        }
    }

    /// 不对应任何进程的空树，供只需要构造任务结构体的测试夹具使用。
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        #[cfg(unix)]
        {
            Self { group: None }
        }
        #[cfg(windows)]
        {
            Self {
                job: super::process_job::ProcessJob::new().expect("create empty job object"),
            }
        }
    }
}

/// 启动一条脱离本进程的后台命令，不等它结束：输出追加写进 `log`，本进程退出、用户关掉终端都不影响它。
/// `envs` 只加给这条命令。
///
/// - Unix：`/bin/sh -c "nohup <命令> >> <log> 2>&1 &"`。
/// - Windows：`CREATE_NO_WINDOW` 给它一个隐藏的控制台（它再启动的控制台程序会继承，不会弹窗；
///   不用 `DETACHED_PROCESS`，那样孙进程会弹出可见窗口）；`CREATE_NEW_PROCESS_GROUP` 让它收不到
///   本终端的 Ctrl+C。
///
/// # Errors
/// * [`io::Error`] - 无法打开日志（仅 Windows），或无法启动进程。
pub fn spawn_detached(
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
    log: &std::path::Path,
) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(nohup_shell_command(program, args, envs, log))
            .spawn()
            .map(|_| ())
    }
    #[cfg(windows)]
    {
        use std::io::Seek;
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};

        // 不用 `append(true)`：那样的句柄只有追加权限，Git Bash 等 MSYS 程序写入会失败。
        let mut stdout = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(log)?;
        stdout.seek(io::SeekFrom::End(0))?;
        let stderr = stdout.try_clone()?;
        std::process::Command::new(program)
            .args(args)
            .envs(envs.iter().copied())
            .stdin(std::process::Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
            .spawn()
            .map(|_| ())
    }
}

#[cfg(unix)]
fn nohup_shell_command(
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
    log: &std::path::Path,
) -> String {
    let env_prefix: String = envs
        .iter()
        .map(|(key, value)| format!("{key}={} ", shell_words::quote(value)))
        .collect();
    let command = shell_words::join(std::iter::once(program).chain(args.iter().copied()));
    let log = shell_words::quote(&log.to_string_lossy()).into_owned();
    format!("{env_prefix}nohup {command} >> {log} 2>&1 &")
}

/// Launch a shared service outside the starter's process group / Windows Job.
/// Keep its cwd out of project/worktree directories. Never silently retry inside a Job.
pub fn spawn_service_process(
    command: &mut std::process::Command,
    service_dir: &std::path::Path,
) -> io::Result<std::process::Child> {
    command.current_dir(service_dir);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe; the closure does not allocate or lock.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW};
        // Own a hidden console so console children (and grandchildren) do not open windows.
        command.creation_flags(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB);
    }
    command.spawn()
}

/// 子进程的退出码；Unix 上被信号结束时按 shell 惯例返回 128 + 信号值，取不到时返回 -1。
pub fn exit_code(status: &ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .or_else(|| status.signal().map(|signal| 128 + signal))
            .unwrap_or(-1)
    }
    #[cfg(not(unix))]
    {
        status.code().unwrap_or(-1)
    }
}

#[cfg(test)]
#[path = "tests/process_test.rs"]
mod tests;
