//! # 原语执行器内部小工具
//!
//! 与 [`super::DefaultPrimitiveExecutor`] 共用的字符串化 / 二进制查找等无状态 helper。
//! 所有函数仅暴露给 `executor` 子模块（`pub(super)`），不向外扩散。

use crate::core::permission::{GrantTrigger, GrantType, PermissionScope};
use crate::core::tools::primitive::PrimitiveOperation;
use std::path::PathBuf;

pub(super) fn op_summary(op: PrimitiveOperation) -> &'static str {
    match op {
        PrimitiveOperation::Read => "读取",
        PrimitiveOperation::Write => "写入",
        PrimitiveOperation::Edit => "编辑",
        PrimitiveOperation::Bash => "执行命令",
    }
}

/// 把 [`PermissionScope`] 序列化为审计字符串（与 serde rename_all = snake_case 一致）。
pub(super) fn permission_scope_str(scope: PermissionScope) -> String {
    match scope {
        PermissionScope::Read => "read",
        PermissionScope::Write => "write",
        PermissionScope::Bash => "bash",
        PermissionScope::BashApproval => "bash_approval",
        PermissionScope::Forbidden => "forbidden",
    }
    .to_string()
}

/// 把 [`GrantType`] 序列化为审计字符串。
pub(super) fn grant_type_str(s: GrantType) -> String {
    match s {
        GrantType::AgentDefinitionDir => "agent_definition_dir",
        GrantType::AgentPlansDir => "agent_plans_dir",
        GrantType::AgentWorkspaceRoot => "agent_workspace_root",
        GrantType::SessionScope => "session_scope",
        GrantType::PathRuleReadOnly => "path_rule_read_only",
        GrantType::AgentTrailDir => "agent_trail_dir",
        GrantType::BashPolicy => "bash_policy",
    }
    .to_string()
}

/// 把 [`GrantTrigger`] 序列化为审计字符串。
pub(super) fn grant_trigger_str(s: GrantTrigger) -> String {
    match s {
        GrantTrigger::BuiltinDefault => "builtin_default",
        GrantTrigger::WorkspaceRootsConfig => "workspace_roots_config",
        GrantTrigger::PathRulesConfig => "path_rules_config",
        GrantTrigger::BashRegexConfig => "bash_regex_config",
        GrantTrigger::UserConfirm => "user_confirm",
        GrantTrigger::CwdLazyPrompt => "cwd_lazy_prompt",
        GrantTrigger::DraggedPathMenu => "dragged_path_menu",
        GrantTrigger::AutoConfirmFlag => "auto_confirm_flag",
    }
    .to_string()
}

pub(super) fn find_binary(candidates: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for candidate in candidates {
            let path = dir.join(candidate);
            if path.is_file() {
                return Some(path);
            }
            #[cfg(windows)]
            {
                let exe = dir.join(format!("{}.exe", candidate));
                if exe.is_file() {
                    return Some(exe);
                }
            }
        }
    }
    None
}
