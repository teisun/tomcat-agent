//! # 工作区权限分级
//!
//! 与 plan §2/§3/§4 对齐：将权限决策封装成 [`PermissionGate`] trait + 默认实现
//! [`DefaultPermissionGate`]，提供"3 层决策引擎"（Forbidden / NeedConfirm / Allow）。
//!
//! 子模块：
//!
//! - [`types`]：[`PermissionDecision`] / [`GrantTrace`] / [`PermissionScope`] /
//!   [`PathRuleMode`] / [`EffectiveRoots`]
//! - [`path_rule`]：[`PathRule`] 单条 path 规则与匹配逻辑（含 globset 支持）
//! - [`defaults`]：内置默认规则常量（凭据保护 / Agent 自我提权防护）
//! - [`session_grants`]：[`SessionGrants`] 会话级临时授权
//! - [`gate`]：[`PermissionGate`] trait + [`DefaultPermissionGate`]
//!
//! 调用方仅依赖 [`PermissionGate`] trait + 类型即可（PR-2 起 executor 接入）。

pub mod bash_ast;
pub mod bash_parser;
pub mod defaults;
pub mod gate;
pub mod path_rule;
pub mod session_grants;
pub mod types;
pub mod url_like;

pub use bash_ast::{
    AstReject, AstSegmentVerdict, BashAstChecker, BashSegment, NoopSandboxBackend, PersistentShell,
    SandboxBackend, ToolsBashAstConfig,
};

pub use defaults::{
    builtin_default_rules, BUILTIN_BASH_APPROVAL_REQUIRED, BUILTIN_BASH_FORBIDDEN,
    BUILTIN_DEFAULT_PATH_RULES,
};
pub use gate::{DefaultPermissionGate, GateConfig, PermissionGate};
pub use path_rule::PathRule;
pub use session_grants::{SessionGrants, SessionPathRules};
pub use types::{
    EffectiveRoots, GrantTrace, GrantTrigger, GrantType, PathRuleMode, PermissionDecision,
    PermissionScope,
};
pub use url_like::is_url_like;

#[cfg(test)]
mod tests;
