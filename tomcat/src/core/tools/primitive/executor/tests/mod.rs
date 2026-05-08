//! `executor` 子模块单测（在 `tests/mod.rs` 中按需挂载）。
//!
//! `bash` / `read` / `write_edit` / `gate` 等大型 executor 的端到端测试仍归
//! [`crate::core::tools::primitive::tests`] 父目录下的 `suite_test` /
//! `gate_suite_test` / `read_window_test`；本目录只承载与单个 executor 子文件
//! 一对一的单元测试（如 `output_accum_test`）。

mod output_accum_test;
