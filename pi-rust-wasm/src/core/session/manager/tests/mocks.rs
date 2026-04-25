//! # `SessionManager` 测试共享 fixture
//!
//! 当前仅含 `temp_sessions_dir`：返回一个进程/线程/计数器三元组的临时目录路径，
//! 避免并发测试之间互相污染，并方便每个用例自己 `remove_dir_all`。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn temp_sessions_dir() -> PathBuf {
    let c = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    std::env::temp_dir().join(format!("pi_wasm_mgr_{}_{}_{}", std::process::id(), ms, c))
}
