//! `api::chat` 父目录模块的单元测试（默认在此 `mod.rs` 聚合；预检例外见下）。
//!
//! **预检**：物理文件 [`preflight_test.rs`](preflight_test.rs) 由上级 [`preflight.rs`](../preflight.rs) 内 `#[path = "tests/preflight_test.rs"]` 挂载（测私有符号，`RUST_FILE_LINES_SPEC` §A.9）；本文件**不得**再 `mod preflight`。

mod show_thinking_resolve_test;
mod suite_test;
mod thinking_persist_test;
