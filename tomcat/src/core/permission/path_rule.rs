//! # PathRule 与 PathRules 集合
//!
//! `PathRule` 是对单条 path 规则的内存表示，支持两种匹配模式：
//!
//! 1. **不含 glob 字符**（`* / ** / ?`）：规范化路径前缀比较（与 `workspace_roots` 一致）；
//! 2. **含 glob 字符**：用 `globset::GlobMatcher` 匹配字符串路径（用于 `~/.tomcat/agents/*/sessions` 等）。
//!
//! 序列化用 `serde rename_all = "snake_case"`：mode 写为 `"deny"` / `"readonly"`。

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::infra::error::AppError;
use crate::infra::platform::normalize_path;

use super::types::{path_starts_with, PathRuleMode};

/// 单条 path_rule（TOML 中 `[[primitive.path_rules]]` 的反序列化目标）。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PathRule {
    /// 路径模式：支持 `~` 前缀；不含 glob 字符时按规范化路径前缀匹配；
    /// 含 `* / ** / ?` 时按 globset 匹配。
    pub path: String,
    /// 模式：`Deny` 或 `Readonly`。
    pub mode: PathRuleMode,
}

impl PathRule {
    pub fn new(path: impl Into<String>, mode: PathRuleMode) -> Self {
        Self {
            path: path.into(),
            mode,
        }
    }

    /// 是否含 glob 字符（决定走 prefix 还是 globset 匹配）。
    pub fn has_glob(&self) -> bool {
        self.path.contains('*') || self.path.contains('?')
    }

    /// 把 `~` 展开为 home，返回展开后的字符串路径。
    pub fn expanded_path(&self) -> Result<String, AppError> {
        if self.has_glob() {
            // glob 用字符串匹配：仅展开 `~`。
            if let Some(rest) = self.path.strip_prefix("~/") {
                let home = dirs::home_dir().ok_or_else(|| {
                    AppError::Config("无法解析 home 目录用于 path_rule".to_string())
                })?;
                Ok(home.join(rest).to_string_lossy().into_owned())
            } else if self.path == "~" {
                let home = dirs::home_dir().ok_or_else(|| {
                    AppError::Config("无法解析 home 目录用于 path_rule".to_string())
                })?;
                Ok(home.to_string_lossy().into_owned())
            } else {
                Ok(self.path.clone())
            }
        } else {
            // 走 normalize_path（处理 ~ + 相对 + symlink）。
            let p = normalize_path(&self.path)?;
            Ok(p.to_string_lossy().into_owned())
        }
    }

    /// 判断 `target` 是否命中本条规则。
    ///
    /// 不含 glob：规范化前缀比较（target 与 prefix 都用最长存在祖先 canonicalize）；
    /// 含 glob：globset 匹配（target 用字符串）。
    pub fn matches(&self, target: &Path) -> bool {
        let target_s = target.to_string_lossy();
        let expanded = match self.expanded_path() {
            Ok(e) => e,
            Err(_) => return false,
        };
        if self.has_glob() {
            match globset::Glob::new(&expanded) {
                Ok(g) => g.compile_matcher().is_match(&*target_s),
                Err(_) => false,
            }
        } else {
            // 直接比较 expanded（已 canonicalize）与 target 字符串。
            if path_starts_with(&target_s, &expanded) {
                return true;
            }
            // 容错：把 target 也走一遍最长存在祖先 canonicalize 后再比较。
            let target_canon = super::gate::canonicalize_with_existing_ancestor(target);
            let prefix_canon =
                super::gate::canonicalize_with_existing_ancestor(Path::new(&expanded));
            path_starts_with(
                &target_canon.to_string_lossy(),
                &prefix_canon.to_string_lossy(),
            )
        }
    }
}
