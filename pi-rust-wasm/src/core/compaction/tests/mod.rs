//! `core::compaction` 单元测试（从旧 `tests.rs` 迁出并按主题拆分）。
//!
//! `preheat_test.rs` 需要测私有 `snapshot_message_bounds_for_preheat`，按
//! [RUST_FILE_LINES_SPEC §A 第 9 条] 走 `#[cfg(test)] #[path] mod tests;`
//! 挂载（测试文件物理位置仍在本目录 `preheat_test.rs`，但模块挂在被测源文件下，
//! 故此处**不**声明 `mod preheat;`）。

mod apply_and_after_reply_test;
mod context_layer0_v2_test;
mod layer0_cleanup_test;
mod legacy_transcript_compat_test;
mod messages_to_text_test;
mod mocks;
mod preheat_and_truncation_test;
mod prompt_snapshot_test;
mod turn_boundaries_l3_test;
