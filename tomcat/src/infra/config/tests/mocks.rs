//! # `infra::config::tests` 共享 fixture
//!
//! 唯一一个共享 helper `cfg_with_work_dir`：返回一个 `storage.work_dir`
//! 指向给定路径的 `AppConfig`，供 `ensure_embedded_assets` /
//! `ensure_work_dir_structure` 等需要真实 work dir 的资产抽取测试复用。

use super::super::*;

pub(super) fn cfg_with_work_dir(dir: &std::path::Path) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.to_string_lossy().to_string());
    cfg
}
