//! 测试共享辅助。
//!
//! 当前提供两把全局锁：
//! - `home_env_lock()`：给会临时覆写 `HOME` 或依赖 `dirs::home_dir()` 稳定值的单测复用。
//! - `cwd_lock()`：给会临时覆写进程级 `current_dir` 的单测复用。
//!   两者都用于避免 `cargo test` 并行时互相污染。

use std::sync::{Mutex, OnceLock};

pub(crate) fn home_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub(crate) fn cwd_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
