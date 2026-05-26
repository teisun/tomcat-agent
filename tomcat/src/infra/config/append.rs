//! `tomcat.config.toml` 共享追加写入辅助。
//!
//! 这些函数被多个入口共享：
//!
//! - `tomcat pathrules add` / `tomcat workspace add` CLI（用户特权通道）
//! - `config_set` LLM 工具（受白名单约束）
//! - `/path` 路径授权菜单 `[w]/[r]/[d]`（chat_loop 内部）
//!
//! 三个入口共享 [`super::lock::with_config_lock`] —— 进程间并发安全。
//! 操作语义为「追加 only」：不去重不替换，重复条目由 [`load_config`] 的 normalize 阶段消解。

use std::path::Path;

use super::lock::with_config_lock;
use super::types::WorkspaceEntry;
use super::{load_config_toml_file, validate_config};
use crate::core::permission::PathRule;
use crate::infra::error::AppError;
use crate::infra::platform::write_file_atomic;

/// 把 `workspace_roots` 单条目追加到 `config_path` 指向的 TOML 文件。
///
/// 调用方应预先做绝对路径化 / 存在性校验。函数本身仅做：从磁盘原文 load -> push -> validate -> 原子写。
pub fn append_workspace_root_to_disk(config_path: &Path, abs_path: String) -> Result<(), AppError> {
    with_config_lock(config_path, || {
        let mut cfg = load_config_toml_file(config_path)?;
        if cfg.workspace.workspace_roots.iter().any(|s| s == &abs_path) {
            return Ok(());
        }
        cfg.workspace.workspace_roots.push(abs_path);
        validate_config(&cfg)?;
        let toml_str = toml::to_string_pretty(&cfg)
            .map_err(|e| AppError::Config(format!("序列化配置失败: {}", e)))?;
        write_file_atomic(config_path, toml_str.as_bytes())?;
        Ok(())
    })
}

/// 把一条 `path_rule` 追加到 `[primitive]` `path_rules` 数组。
pub fn append_path_rule_to_disk(config_path: &Path, rule: PathRule) -> Result<(), AppError> {
    with_config_lock(config_path, || {
        let mut cfg = load_config_toml_file(config_path)?;
        if cfg
            .primitive
            .path_rules
            .iter()
            .any(|r| r.path == rule.path && r.mode == rule.mode)
        {
            return Ok(());
        }
        cfg.primitive.path_rules.push(rule);
        validate_config(&cfg)?;
        let toml_str = toml::to_string_pretty(&cfg)
            .map_err(|e| AppError::Config(format!("序列化配置失败: {}", e)))?;
        write_file_atomic(config_path, toml_str.as_bytes())?;
        Ok(())
    })
}

/// 把一条 `[[workspace.entries]]` 追加到配置；若已存在同 path 则跳过。
#[allow(dead_code)]
pub fn append_workspace_entry_to_disk(
    config_path: &Path,
    entry: WorkspaceEntry,
) -> Result<(), AppError> {
    with_config_lock(config_path, || {
        let mut cfg = load_config_toml_file(config_path)?;
        if cfg.workspace.entries.iter().any(|e| e.path == entry.path) {
            return Ok(());
        }
        cfg.workspace.entries.push(entry);
        validate_config(&cfg)?;
        let toml_str = toml::to_string_pretty(&cfg)
            .map_err(|e| AppError::Config(format!("序列化配置失败: {}", e)))?;
        write_file_atomic(config_path, toml_str.as_bytes())?;
        Ok(())
    })
}
