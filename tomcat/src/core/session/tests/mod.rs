//! # `core::session` 单元测试目录
//!
//! 集中存放 `core/session/` 下单文件子模块（`store_test.rs` / `transcript.rs`）
//! 的单元测试。`manager/` 是真目录模块，自带 `manager/tests/`，不在此处。
//!
//! `append_message_chain_test.rs` 需要测私有 `is_in_pending_tool_round`，按
//! [RUST_FILE_LINES_SPEC §A 第 9 条] 走 `#[cfg(test)] #[path] mod tests;`
//! 挂载（测试文件物理位置仍在本目录 `append_message_chain_test.rs`，但模块挂在
//! 被测源文件下，故此处**不**声明 `mod append_message_chain;`）。
//!
//! `transcript` 历史 507 行已超 350 行红线，按主题拆为
//! `transcript_header_test` / `transcript_read_test` / `transcript_lookup_test` /
//! `transcript_mutate_test` 四个文件。

mod resume_index_test;
mod scope_test;
mod model_thinking_test;
mod store_test;
mod subagent_transcript_test;
mod transcript_header_test;
mod transcript_lookup_test;
mod transcript_mutate_test;
mod transcript_read_test;
