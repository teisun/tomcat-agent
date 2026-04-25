//! # `core::session` 单元测试目录
//!
//! 集中存放 `core/session/` 下单文件子模块（`store.rs` / `transcript.rs`）
//! 的单元测试。`manager/` 是真目录模块，自带 `manager/tests/`，不在此处。
//!
//! `append_message_chain.rs` 需要测私有 `is_in_pending_tool_round`，按
//! [RUST_FILE_LINES_SPEC §A 第 9 条] 走 `#[cfg(test)] #[path] mod tests;`
//! 挂载（测试文件物理位置仍在本目录 `append_message_chain.rs`，但模块挂在
//! 被测源文件下，故此处**不**声明 `mod append_message_chain;`）。
//!
//! `transcript` 历史 507 行已超 350 行红线，按主题拆为
//! `transcript_header` / `transcript_read` / `transcript_lookup` /
//! `transcript_mutate` 四个文件。

mod store;
mod transcript_header;
mod transcript_lookup;
mod transcript_mutate;
mod transcript_read;
