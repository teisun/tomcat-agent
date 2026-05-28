use serde::{Deserialize, Serialize};

use super::core::default_true;

/// 4 原语配置：bash 两档列表 + path_rules 结构化规则。
///
/// **schema 升级（plan §5）**：
/// - 删除 `path_blacklist`（被 `path_rules` 替代，模式更明确）
/// - 删除 `require_approval_for_all_write` / `require_approval_for_all_bash`
///   （`workspace-in-default-allow, workspace-out-confirm` 模型已让它们冗余）
/// - 新增 `path_rules`: `Vec<PathRule>`（结构化路径规则，模式 `deny` / `readonly`）
/// - `bash_forbidden` / `bash_approval_required` 默认转为 regex 字符串列表
///   （编译由 `permission::gate` 在构造时完成）
///
/// 删除 legacy whitelist 配置后，路径允许根只由 `workspace.workspace_roots` 表达；
/// bash 只保留 forbidden / approval_required 两类策略。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PrimitiveConfig {
    /// 结构化路径规则。每条 `path` + `mode`（`deny` / `readonly`）。
    /// 在 gate 模式下与 builtin 规则合并；仅生效，不可弱化 builtin。
    #[serde(default)]
    pub path_rules: Vec<crate::core::permission::PathRule>,
    /// bash 高危但可允许：regex 列表，命中后弹 confirm；与 builtin 合并。
    #[serde(default)]
    pub bash_approval_required: Vec<String>,
    /// bash 禁止：regex 列表，命中即拒绝；与 builtin 合并。
    #[serde(default)]
    pub bash_forbidden: Vec<String>,
    #[serde(default = "default_true")]
    pub auto_confirm: bool,
    /// `bash` 在 Unix 上 `sh -c` 前可选 source 的 env 脚本路径；`None` 时默认 `$HOME/.wasmedge/env`。
    #[serde(default)]
    pub wasmedge_env_path: Option<String>,
}

impl Default for PrimitiveConfig {
    fn default() -> Self {
        Self {
            path_rules: Vec::new(),
            bash_approval_required: Vec::new(),
            bash_forbidden: Vec::new(),
            auto_confirm: true,
            wasmedge_env_path: None,
        }
    }
}
