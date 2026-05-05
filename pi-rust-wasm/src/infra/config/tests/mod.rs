//! # `infra::config::tests` 测试目录
//!
//! 历史 `tests.rs` 431 行已超过 RUST_FILE_LINES_SPEC §A 红线，按主题拆分为：
//!
//! - `mocks`：`cfg_with_work_dir` 共享 fixture。
//! - `defaults_test`：`AppConfig` / `SecurityConfig` 默认值与 round-trip。
//! - `validate_test`：`validate_config` + `resolve_workspace_roots_paths`。
//! - `load_test`：`load_config` 路径与 `pi.config.toml.example` 同步保护。
//! - `assets_test`：`assets_test` 子模块的 SHA / 原子写 / 锁 / 嵌入式抽取。
//! - `context_cfg_test`：`ContextConfig` 默认值、budget 计算与 toml override。

mod assets_test;
mod context_cfg_test;
mod defaults_test;
mod load_test;
mod mocks;
mod tools_cfg_test;
mod validate_test;
