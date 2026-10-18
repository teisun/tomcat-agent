//! Windows Job Object：一次终止 bash 任务派生的整棵进程树。
//!
//! Git Bash 执行命令时要经过一个随即退出的 fork 中转进程，`taskkill /T` 依赖的父子 PID 链
//! 在这里断开，后台 `sleep` 之类的孙子进程会成为孤儿。Job 成员关系在创建进程时由系统继承，
//! 与中转进程是否还活着无关。

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    OpenThread, ResumeThread, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME,
};

/// 一个 bash 任务独占的 Job。关闭句柄不会终止成员进程，与 Unix 进程组语义一致。
pub struct ProcessJob(OwnedHandle);

impl ProcessJob {
    /// 创建一个空的匿名 Job。
    pub(crate) fn new() -> io::Result<Self> {
        // SAFETY: 两个参数都允许为 null（默认安全属性、匿名 Job）。
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw` 是刚创建的有效句柄，所有权只交给这里。
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(raw) }))
    }

    /// 挂起启动 `command`，加入新 Job 后再恢复：子进程执行第一条指令前就已在 Job 内，
    /// 它派生的后代全部自动继承。加入或恢复失败时杀掉子进程。
    pub fn spawn(
        command: &mut tokio::process::Command,
    ) -> io::Result<(tokio::process::Child, Self)> {
        let job = Self::new()?;
        let mut child = command.creation_flags(CREATE_SUSPENDED).spawn()?;
        if let Err(error) = job.adopt(&child) {
            let _ = child.start_kill();
            return Err(error);
        }
        Ok((child, job))
    }

    fn adopt(&self, child: &tokio::process::Child) -> io::Result<()> {
        let (Some(process), Some(pid)) = (child.raw_handle(), child.id()) else {
            return Err(io::Error::other(
                "suspended child exited before joining its job",
            ));
        };
        // SAFETY: Job 与子进程句柄在调用期间都有效。
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), process) } == 0 {
            return Err(io::Error::last_os_error());
        }
        resume_threads(pid)
    }

    /// 终止 Job 内所有进程，包括已经成为孤儿的后代；Job 已空时同样成功。
    pub fn terminate(&self) -> io::Result<()> {
        // SAFETY: 句柄由 `self` 持有，始终有效。
        if unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// 标准库不暴露挂起进程的主线程句柄，只能按 PID 枚举线程逐个恢复。
fn resume_threads(pid: u32) -> io::Result<()> {
    // SAFETY: TH32CS_SNAPTHREAD 忽略 pid 参数，返回全系统线程快照。
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: 快照句柄有效且只在这里持有。
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut resumed = false;
    // SAFETY: `entry.dwSize` 已按结构体大小初始化。
    let mut more = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: 线程 ID 来自同一份快照；返回句柄立即交给 OwnedHandle。
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if thread.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `thread` 是刚打开的有效句柄。
            let thread = unsafe { OwnedHandle::from_raw_handle(thread) };
            // SAFETY: 句柄带 THREAD_SUSPEND_RESUME 权限。
            if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
                return Err(io::Error::last_os_error());
            }
            resumed = true;
        }
        // SAFETY: 同上。
        more = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    if resumed {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "no thread found for suspended process {pid}"
        )))
    }
}
