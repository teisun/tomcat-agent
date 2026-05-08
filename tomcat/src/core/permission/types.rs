//! # 工作区权限分级 - 核心类型
//!
//! 与 `.cursor/plans/workspace_permission_tiers_design_1ad7681a.plan.md` §2 对齐：
//! `PermissionDecision` / `GrantTrace` / `PermissionScope` / `PathRuleMode` /
//! `EffectiveRoots`。
//!
//! - `PermissionDecision` 描述 [`PermissionGate::check`](super::PermissionGate)
//!   返回的三态结果。
//! - `GrantTrace` 同时记录授权类型与触发来源，供审计/溯源使用。
//! - `PermissionScope` 描述操作权限范围；与目录来源解耦。
//! - `PathRuleMode` 仅两种合法模式：`Deny` / `ReadOnly`；"未命中"自然表达 allow。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 操作权限范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionScope {
    /// 含默认定义目录 read / extra_root read / readonly path_rule / agent_trail_dir。
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

/// 授权类型——说明“为什么允许”。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantType {
    /// `agent_definition_dir`（`workspace-<agentId>/`）—— 默认 writable 根。
    AgentDefinitionDir,
    /// `tomcat.config.toml` 中 `[workspace] workspace_roots`（用户工作区根，持久）。
    AgentWorkspaceRoot,
    /// 仅本会话生效的授权范围。
    SessionScope,
    /// 命中 path_rules readonly + read 操作。
    PathRuleReadOnly,
    /// `agent_trail_dir` 运行态目录（仅 read）。
    AgentTrailDir,
    /// Bash 命令未命中 forbidden / approval_required 后按策略放行。
    BashPolicy,
}

/// 触发来源——说明“这次授权从哪里来”。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantTrigger {
    /// 内置默认策略。
    BuiltinDefault,
    /// `[workspace] workspace_roots` 配置。
    WorkspaceRootsConfig,
    /// `path_rules` 配置或运行时追加。
    PathRulesConfig,
    /// bash forbidden / approval regex 策略。
    BashRegexConfig,
    /// 用户在普通确认菜单中选择允许。
    UserConfirm,
    /// cwd lazy prompt 产生的授权。
    CwdLazyPrompt,
    /// 路径授权菜单产生的授权；名字为兼容历史审计中的 dragged_path_menu 保留。
    DraggedPathMenu,
    /// `primitive.auto_confirm = true` 时自动允许。
    AutoConfirmFlag,
}

/// 审计/溯源信息：授权类型 + 触发来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantTrace {
    pub grant_type: GrantType,
    pub trigger: GrantTrigger,
}

impl GrantTrace {
    pub const fn new(grant_type: GrantType, trigger: GrantTrigger) -> Self {
        Self {
            grant_type,
            trigger,
        }
    }
}

/// 检查结果（三态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// 通过；附带审计来源与操作等级。
    Allow {
        grant: GrantTrace,
        scope: PermissionScope,
    },
    /// 需要 confirm（Layer-2 外部路径）；
    /// `suggested_root` 为可建议持久化为 `workspace_roots` 的父目录。
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

/// 当前生效的路径范围（从 agent_definition_dir + 配置 + session_grants 派生）。
#[derive(Debug, Clone, Default)]
pub struct EffectiveRoots {
    /// `agent_definition_dir + workspace_roots + session_grants`。
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
