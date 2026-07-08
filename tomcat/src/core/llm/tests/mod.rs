//! # `core::llm::tests` 测试目录
//!
//! 与 `core::agent_loop` 一致：整个 `core::llm` 模块只有**一个** `tests/` 目录，
//! 历史上挂在 `openai/`、`token_usage/`、`types/` 三处子模块下的 `tests/`
//! 已上抬合并到此处，按主题拆分为多个文件。
//!
//! ## 文件分组
//!
//! 1. **`mocks`**：跨用例共享的 fixture（如 `.env` 加载）。
//! 2. **被本入口 `mod` 直接挂载**——只测公共 API、不依赖私有项的文件：
//!    - `registry_test`：`resolve_llm` 路由 + 未知 provider 报错。
//!    - `system_prompt_test`、`token_usage_test`、`types_test`。
//! 3. **`#[path]` 内挂在被测源文件下**——需要看见私有 fn / struct 的文件，按
//!    `UNIT_TEST_LAYOUT_SPEC §9` 由各 Provider 文件末尾自带挂载，**不在本入口**：
//!    - `openai_test` → 挂在 `openai.rs`（文件内再分 `provider` / `stream` 子模块）；
//!    - `openai_responses_test` → 挂在 `openai_responses.rs`。

// `mocks` 需被 `tests/openai_test.rs` 通过 crate path 复用（该文件已由 openai.rs 内挂，
// 不再隶属 `tests` 父模块的兄弟链），故提升到 `pub(crate)`。
mod auth_test;
mod admin_test;
mod catalog_test;
pub(crate) mod mocks;
mod multimodal_test;
mod registry_test;
mod replay_policy_test;
mod resolver_test;
mod system_prompt_test;
mod thinking_policy_test;
mod token_usage_test;
mod types_test;
// `openai_test` / `openai_responses_test` 由各自被测源文件通过 `#[cfg(test)] #[path]`
// 内挂为子模块，避免为测试放宽可见性
// （RUST_FILE_LINES_SPEC §A 第 9 条）。这里**不再**声明，避免同一文件被两处 `mod` 引用。
