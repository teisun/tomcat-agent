//! # `infra::config::tests` 测试目录
//!
//! 历史 `tests.rs` 431 行已超过 RUST_FILE_LINES_SPEC §A 红线，按主题拆分为：
//!
//! - `mocks`：`cfg_with_work_dir` 共享 fixture。
//! - `defaults`：`AppConfig` / `SecurityConfig` 默认值与 round-trip。
//! - `validate`：`validate_config` + `resolve_workspace_roots_paths`。
//! - `load`：`load_config` 路径与 `pi.config.toml.example` 同步保护。
//! - `assets`：`assets` 子模块的 SHA / 原子写 / 锁 / 嵌入式抽取。
//! - `context_cfg`：`ContextConfig` 默认值、budget 计算与 toml override。

mod assets;
mod context_cfg;
mod defaults;
mod load;
mod mocks;
mod validate;
