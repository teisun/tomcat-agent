//! # 工作区权限分级 - 核心类型
//!
//! 与 `.cursor/plans/workspace_permission_tiers_design_1ad7681a.plan.md` §2 对齐：
//! `PermissionDecision` / `GrantSource` / `PermissionLevel` / `PathRuleMode` /
//! `EffectiveRoots`。
//!
//! - `PermissionDecision` 描述 [`PermissionGate::check`](super::PermissionGate)
//!   返回的三态结果。
//! - `GrantSource` 是审计/溯源字段；运行时行为以 `Allow` 自身为准。
//! - `PermissionLevel` 描述操作权限等级；与目录来源解耦。审计字段
//!   `in_working_dir` 是历史字段名，当前仅保留兼容。
//! - `PathRuleMode` 仅两种合法模式：`Deny` / `ReadOnly`；"未命中"自然表达 allow。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 操作权限等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    /// 含默认定义目录 read / extra_root read / readonly path_rule / agent_data_dir。
    Read,
    /// 含默认定义目录 write / extra_root write / session 授权后写。
    Write,
    /// bash 命令通过命令策略。
    Bash,
    /// bash 命令命中 approval_required（需用户确认）。
    BashApproval,
    /// 命中 path_rules deny / bash_forbidden / hardcoded write deny。
    Forbidden,
}

/// 授权来源——主要供审计/溯源；运行时行为以 `Allow` 自身为准。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantSource {
    /// `agent_definition_dir`（`workspace-<agentId>/`）—— 默认 writable 根。
    /// 历史名 `AgentWorkspace` 保留以兼容审计 schema；
    /// 启动 cwd（即 `agent_workspace_dir`）不再属于此来源，需要 `extra_roots` /
    /// `session_grants` / `dragged_paths` 显式授权。
    AgentWorkspace,
    /// `{work_dir}/agents/{id}` 数据目录（仅 read）。
    AgentDataDir,
    /// `pi.config.toml` 中 `[workspace] extra_roots`（持久）。
    ConfigExtraRoot,
    /// 用户在 confirm 弹窗显式选了"本次允许"（知情授权）。
    SessionGrant,
    /// 拖拽菜单 [a] 产生的临时授权。
    DraggedPath,
    /// 命中 path_rules readonly + read 操作。
    PathRuleReadOnly,
    /// Bash 命令未命中 forbidden / approval_required 后按策略放行。
    BashPolicy,
    /// `primitive.auto_confirm = true` 时确认阶段自动允许。
    AutoConfirmFlag,
}

/// 检查结果（三态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// 通过；附带审计来源与操作等级。
    Allow {
        source: GrantSource,
        level: PermissionLevel,
    },
    /// 需要 confirm（Layer-2 外部路径）；
    /// `suggested_root` 为可建议持久化为 `extra_roots` 的父目录。
    NeedConfirm {
        reason: String,
        suggested_root: Option<PathBuf>,
    },
    /// 拒绝（Layer-1 path_rules deny / bash_forbidden / 用户拒绝）。
    Deny { reason: String },
}

/// `PathRule` 模式：仅 deny / readonly 两态；"未命中"等价于 allow。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathRuleMode {
    /// 拒绝任何 read/write/edit/bash 写。
    Deny,
    /// 仅 read 通过；write/edit/bash 写都拒绝。
    Readonly,
}

/// 当前生效的路径范围（从 agent_definition_dir + 配置 + session_grants + dragged 派生）。
#[derive(Debug, Clone, Default)]
pub struct EffectiveRoots {
    /// `agent_definition_dir + extra_roots + session_grants + dragged`。
    pub read_write: Vec<PathBuf>,
    /// `agents/{id}`（除凭据外） + `path_rules` 中的 `Readonly` 命中。
    pub read_only: Vec<PathBuf>,
}

/// `path_rule.matches` 与 `EffectiveRoots` 的最小化前缀匹配辅助。
///
/// 不做 canonicalize（调用方负责），只做规范化的字符串前缀比较；
/// trail-slash 容错。
pub(crate) fn path_starts_with(target: &str, prefix: &str) -> bool {
    let t = target.trim_end_matches(std::path::MAIN_SEPARATOR);
    let p = prefix.trim_end_matches(std::path::MAIN_SEPARATOR);
    if t == p {
        return true;
    }
    t.starts_with(&format!("{}{}", p, std::path::MAIN_SEPARATOR))
}
