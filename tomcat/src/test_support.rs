//! 测试共享辅助。
//!
//! 当前提供两把全局锁：
//! - `home_env_lock()`：给会临时覆写 `HOME` 或依赖 `dirs::home_dir()` 稳定值的单测复用。
//! - `cwd_lock()`：给会临时覆写进程级 `current_dir` 的单测复用。
//!   两者都用于避免 `cargo test` 并行时互相污染。
//!
//! 这里显式使用**可重入**锁，原因是：
//! - 某些测试先拿锁，再调用 `ChatContext::from_config()`；
//! - `from_config()` 在 `#[cfg(test)]` 下也会拿同一把锁，保证读取 HOME/current_dir
//!   时不会被并行测试污染；
//! - 若继续使用 `std::sync::Mutex`，同线程二次进入会自锁，且 panic 会 poison 后续测试。

use parking_lot::{ReentrantMutex, ReentrantMutexGuard};
use std::convert::Infallible;
use std::sync::OnceLock;

pub(crate) type TestLockGuard<'a> = ReentrantMutexGuard<'a, ()>;

pub(crate) struct TestLock(ReentrantMutex<()>);

impl TestLock {
    pub(crate) fn lock(&self) -> Result<TestLockGuard<'_>, Infallible> {
        Ok(self.0.lock())
    }
}

pub(crate) fn home_env_lock() -> &'static TestLock {
    static LOCK: OnceLock<TestLock> = OnceLock::new();
    LOCK.get_or_init(|| TestLock(ReentrantMutex::new(())))
}

pub(crate) fn cwd_lock() -> &'static TestLock {
    static LOCK: OnceLock<TestLock> = OnceLock::new();
    LOCK.get_or_init(|| TestLock(ReentrantMutex::new(())))
}
