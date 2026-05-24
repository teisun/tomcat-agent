//! `api::chat` 父目录模块的单元测试（默认在此 `mod.rs` 聚合；预检例外见下）。
//!
//! **预检 / CLI turn renderer / cwd_lazy**：物理文件 [`preflight_test.rs`](preflight_test.rs)
//! 与 [`cli_turn_renderer_test.rs`](cli_turn_renderer_test.rs) 曾经在本目录内通过 `#[path]`
//! 挂载；现仅保留 `preflight_test.rs` / `cli_turn_renderer_test.rs` 在本目录。
//! `cwd_lazy_prompt_test.rs` 已收回到 `permission/tests/`，此处**不得**再声明对应 `mod`。

mod context_overrides_test;
mod show_thinking_resolve_test;
mod suite_test;
mod thinking_persist_test;
