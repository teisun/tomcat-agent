//! 测试共享辅助。
//!
//! 当前仅提供一把全局 `HOME` 环境锁，给会临时覆写 `HOME` 或依赖 `dirs::home_dir()`
//! 稳定值的单测复用，避免 `cargo test` 并行时互相污染。

use std::sync::{Mutex, OnceLock};

pub(crate) fn home_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
