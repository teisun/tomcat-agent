//! # `core::session::manager::tests` 测试目录
//!
//! 历史 `tests.rs` 845 行已超过 RUST_FILE_LINES_SPEC §A 的红线，按主题拆分为：
//!
//! - `mocks`：跨用例共享的 `temp_sessions_dir`。
//! - `crud`：会话条目 CRUD、store 路径、transcript 路径与只读查询。
//! - `append`：`append_*` 写入路径与 `try_append_message` 校验、`generate_entry_id`。
//! - `hydrate`：`init_context_state` 与 `build_context_from_state`
//!   六种场景的状态还原。
//! - `fold`：`compute_fold_start` / `filter_turns_by_day` 纯函数等价类。
//! - `context_state`：`ContextState::estimated_token_count` / `usage_ratio` /
//!   `invalidate_api_usage` / `persist_context_observability`。

mod append;
mod context_state;
mod crud;
mod fold;
mod hydrate;
mod mocks;
