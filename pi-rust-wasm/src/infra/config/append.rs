//! `pi.config.toml` 共享追加写入辅助。
//!
//! 这些函数被多个入口共享：
//!
//! - `pi pathrules add` / `pi workspace add` CLI（用户特权通道）
//! - `config_set` LLM 工具（受白名单约束）
//! - 拖拽菜单 `[w]/[r]/[d]`（chat_loop 内部）
//!
//! 三个入口共享 [`super::lock::with_config_lock`] —— 进程间并发安全。
//! 操作语义为「追加 only」：不去重不替换，重复条目由 [`load_config`] 的 normalize 阶段消解。

use std::path::Path;

use super::lock::with_config_lock;
use super::types::WorkspaceEntry;
use super::{load_config, validate_config};
use crate::core::permission::PathRule;
use crate::infra::error::AppError;
use crate::infra::platform::write_file_atomic;

/// 把 `extra_roots` 单条目追加到 `config_path` 指向的 TOML 文件。
///
/// 调用方应预先做绝对路径化 / 存在性校验。函数本身仅做：load -> push -> validate -> 原子写。
pub fn append_extra_root_to_disk(config_path: &Path, abs_path: String) -> Result<(), AppError> {
    with_config_lock(config_path, || {
        let mut cfg = load_config(Some(config_path))?;
        if cfg.workspace.extra_roots.iter().any(|s| s == &abs_path) {
            return Ok(());
        }
        cfg.workspace.extra_roots.push(abs_path);
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
        let mut cfg = load_config(Some(config_path))?;
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
        let mut cfg = load_config(Some(config_path))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::permission::PathRuleMode;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn empty_config_file(dir: &TempDir) -> PathBuf {
        let p = dir.path().join("pi.config.toml");
        std::fs::write(
            &p,
            "[agent]\nid='main'\nworkspace='/tmp'\n\n[storage]\nwork_dir='/tmp'\n\n[llm]\nprovider='openai'\ndefault_model='gpt-4o'\n\n[workspace]\nextra_roots=[]\nentries=[]\n\n[primitive]\npath_rules=[]\nbash_approval_required=[]\nbash_forbidden=[]\nauto_confirm=true",
        )
        .unwrap();
        p
    }

    #[test]
    fn append_extra_root_appends_once() {
        let dir = TempDir::new().unwrap();
        let p = empty_config_file(&dir);
        let extra = dir.path().join("extra");
        std::fs::create_dir_all(&extra).unwrap();
        let s = extra.to_string_lossy().into_owned();
        append_extra_root_to_disk(&p, s.clone()).unwrap();
        append_extra_root_to_disk(&p, s.clone()).unwrap();
        let cfg = load_config(Some(&p)).unwrap();
        assert_eq!(cfg.workspace.extra_roots, vec![s]);
    }

    #[test]
    fn append_path_rule_dedupes() {
        let dir = TempDir::new().unwrap();
        let p = empty_config_file(&dir);
        let rule = PathRule {
            path: "~/.foo".to_string(),
            mode: PathRuleMode::Deny,
        };
        append_path_rule_to_disk(&p, rule.clone()).unwrap();
        append_path_rule_to_disk(&p, rule).unwrap();
        let cfg = load_config(Some(&p)).unwrap();
        assert_eq!(cfg.primitive.path_rules.len(), 1);
    }

    #[test]
    fn append_workspace_entry_dedupes_by_path() {
        let dir = TempDir::new().unwrap();
        let p = empty_config_file(&dir);
        let entry = WorkspaceEntry {
            path: "/tmp/proj".into(),
            alias: Some("proj".into()),
            description: None,
        };
        append_workspace_entry_to_disk(&p, entry.clone()).unwrap();
        append_workspace_entry_to_disk(&p, entry).unwrap();
        let cfg = load_config(Some(&p)).unwrap();
        assert_eq!(cfg.workspace.entries.len(), 1);
    }
}
