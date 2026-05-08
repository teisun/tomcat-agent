//! # `api::cli::tests` 测试目录
//!
//! 历史 `tests.rs` 750 行已超过 RUST_FILE_LINES_SPEC §A 红线，按主题拆分为：
//!
//! - `mocks`：临时 `HOME` 配置 + `test_config` 隔离 fixture。
//! - `parse_cli_test`：clap 参数解析测试。
//! - `run_basic_test`：`init` / `doctor` / `config` / `audit_test` 等不依赖
//!   会话目录隔离的简单子命令路径。
//! - `session_cmd_test`：`tomcat session ...` 全部子命令分支。
//! - `workspace_cmd_test`：`tomcat workspace ...` 在临时 `HOME` 下的 add/list/remove。
//! - `plugin_cmd_test`：`tomcat plugin ...` 子命令 + `load/save_plugin_registry`。
//! - `config_keys_test`：`resolve_toml_key` / `set_toml_key` 工具函数。
//! - `audit_test`：审计日志解析与导出工具函数。
//!
//! `chat_cmd_test.rs` 需要测私有 `DOUBLE_TAP_WINDOW`，按
//! [RUST_FILE_LINES_SPEC §A 第 9 条] 走 `#[cfg(test)] #[path] mod tests;`
//! 挂载（测试文件物理位置仍在本目录 `chat_cmd_test.rs`，但模块挂在被测源文件下，
//! 故此处**不**声明 `mod chat_cmd;`）。

mod audit_test;
mod config_keys_test;
mod mocks;
mod parse_cli_test;
mod pathrules_cmd_test;
mod plugin_cmd_test;
mod run_basic_test;
mod session_cmd_test;
mod workspace_cmd_test;
