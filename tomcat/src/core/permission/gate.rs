//! # PermissionGate trait + DefaultPermissionGate
//!
//! 三层决策引擎（与 plan §3 对齐）：
//!
//! 1. **Layer 1 — Forbidden**：硬否决，不可被 confirm 推翻。
//!    - 命中 `path_rules` `Deny`（builtin ∪ user TOML ∪ session）；
//!    - 写/编辑命中 `path_rules` `Readonly`；
//!    - bash 命令命中 `bash_forbidden` regex（builtin ∪ user TOML）。
//! 2. **Layer 2 — NeedConfirm**：路径在 `EffectiveRoots` 之外，弹 confirm，
//!    给出 `suggested_root`（默认是父目录或本身）。
//!    bash 命中 `bash_approval_required` 同样落到此层。
//! 3. **Layer 3 — Allow**：路径在 `EffectiveRoots.read_write` / `read_only` /
//!    `agent_trail_dir`（仅 read）；bash 未命中 forbidden / approval_required 时 Allow。
//!
//! `auto_confirm = true` 仅短路 Layer-2 NeedConfirm（写入审计标记 `AutoConfirmFlag`），
//! Layer-1 Forbidden 永远不可被绕过。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use globset::Glob;
use regex::Regex;

use super::defaults::{
    builtin_default_rules, BUILTIN_BASH_APPROVAL_REQUIRED, BUILTIN_BASH_FORBIDDEN,
    BUILTIN_DEFAULT_PATH_RULES,
};
use super::path_rule::PathRule;
use super::session_grants::{SessionGrants, SessionPathRules};
use super::types::{
    path_starts_with, EffectiveRoots, GrantTrace, GrantTrigger, GrantType, PathRuleMode,
    PermissionDecision, PermissionScope,
};
use crate::core::tools::primitive::PrimitiveOperation;
use crate::infra::error::AppError;
use crate::infra::platform::normalize_path;

// ─────────────────────────────────────────────────────────────────────────────
// PermissionGate Trait
// ─────────────────────────────────────────────────────────────────────────────

/// 路径与 bash 命令的统一权限检查抽象。
pub trait PermissionGate: Send + Sync {
    /// 检查一个原语对路径的访问。
    fn check(&self, op: PrimitiveOperation, path: &str) -> Result<PermissionDecision, AppError>;

    /// 检查一条 bash 命令（可能包含路径，由实现内部解析）。
    fn check_bash(&self, command: &str) -> Result<PermissionDecision, AppError>;

    /// 暴露一份 EffectiveRoots 快照（用于 system_prompt / 审计）。
    fn effective_roots(&self) -> EffectiveRoots;

    /// 暴露当前生效的 path_rules（合并后）—— 用于注入 system_prompt。
    fn effective_path_rules(&self) -> Vec<PathRule>;

    /// 把一个会话级授权写入 SessionGrants（confirm 通过 / 拖入路径）。
    fn grant_session(&self, path: PathBuf, trigger: GrantTrigger);

    /// 把当前会话新增的 path_rule 写入运行时规则集，让 deny / readonly 立即生效。
    fn grant_path_rule(&self, rule: PathRule);
}

// ─────────────────────────────────────────────────────────────────────────────
// DefaultPermissionGate
// ─────────────────────────────────────────────────────────────────────────────

/// 默认实现：不可变快照式（构造时把 cfg / agent_definition_dir / agent_dir / workspace_roots
/// 全部冻结），共享 `SessionGrants` 走 `Arc` + 内部 Mutex。
pub struct DefaultPermissionGate {
    /// Agent 设计态目录（`workspace-<agentId>/`）—— 默认 writable 根，
    /// 仅承载 AGENTS.md / SOUL.md / skills / memory 等 agent 长期配置。
    /// 注意：用户启动 `tomcat chat` 时的 shell cwd 不会自动放进 writable 集合，
    /// 需通过 `workspace_roots` / `session_grants` 才能访问。
    agent_definition_dir: PathBuf,
    /// 配置中显式声明的额外根（writable）。
    workspace_roots: Vec<PathBuf>,
    /// `{work_dir}/agents/{id}` 一系列只读目录（read only）。
    agent_trail_readonly_dirs: Vec<PathBuf>,
    /// 构造时冻结的 path_rules（builtin ∪ user TOML）。
    path_rules: Vec<PathRule>,
    /// 编译好的 bash 三档 regex；构造期一次性编译，bad regex 跳过 + warn。
    bash_forbidden: Vec<Regex>,
    bash_approval: Vec<Regex>,
    /// 是否对 Layer-2 NeedConfirm 短路（不影响 Layer-1）。
    auto_confirm: bool,
    /// 共享会话授权。
    session_grants: SessionGrants,
    /// 当前会话新增的 deny / readonly 规则。
    session_path_rules: SessionPathRules,
}

/// `DefaultPermissionGate::new` 构造参数。
#[derive(Debug, Clone)]
pub struct GateConfig {
    /// Agent 设计态目录（`workspace-<agentId>/`）；作为默认 writable 根。
    /// 启动 cwd 不在此处，访问启动 cwd 子树需要 `workspace_roots` 或 session 授权。
    pub agent_definition_dir: PathBuf,
    pub workspace_roots: Vec<PathBuf>,
    pub agent_trail_readonly_dirs: Vec<PathBuf>,
    pub user_path_rules: Vec<PathRule>,
    pub user_bash_forbidden: Vec<String>,
    pub user_bash_approval: Vec<String>,
    pub auto_confirm: bool,
}

impl DefaultPermissionGate {
    pub fn new(cfg: GateConfig, session_grants: SessionGrants) -> Self {
        // path_rules 合并：builtin ∪ user TOML（user 不能弱化 builtin，
        // 但可以追加；`Deny` 覆盖 `Readonly` 由匹配阶段保证：
        // 我们把所有规则放入同一个 vec，然后在匹配时优先 Deny 命中）。
        let mut path_rules = builtin_default_rules();
        path_rules.extend(cfg.user_path_rules);

        // bash 三档 regex 编译：builtin ∪ user。
        let bash_forbidden = compile_regex_list(BUILTIN_BASH_FORBIDDEN, &cfg.user_bash_forbidden);
        let bash_approval =
            compile_regex_list(BUILTIN_BASH_APPROVAL_REQUIRED, &cfg.user_bash_approval);

        Self {
            agent_definition_dir: cfg.agent_definition_dir,
            workspace_roots: cfg.workspace_roots,
            agent_trail_readonly_dirs: cfg.agent_trail_readonly_dirs,
            path_rules,
            bash_forbidden,
            bash_approval,
            auto_confirm: cfg.auto_confirm,
            session_grants,
            session_path_rules: SessionPathRules::new(),
        }
    }

    /// 构造一份 `Arc<dyn PermissionGate>`。
    pub fn into_arc(self) -> Arc<dyn PermissionGate> {
        Arc::new(self)
    }

    /// 把字符串路径规范化：先 `normalize_path`（处理 `~` + canonicalize），
    /// 若返回值仍是 symlink 链未解析的形式，则尝试 canonicalize 最长存在的祖先并拼回剩余子路径，
    /// 以保证不存在的目标也能被前缀匹配命中（macOS `/var` -> `/private/var` 等场景）。
    fn normalize(&self, raw: &str) -> Result<PathBuf, AppError> {
        let p = normalize_path(raw)?;
        Ok(canonicalize_with_existing_ancestor(&p))
    }

    /// 路径是否在 `workspace_roots` 或 `agent_definition_dir` 之中（writable 集合）。
    fn in_writable_set(&self, target: &Path) -> bool {
        let s = target.to_string_lossy();
        let def = canonicalize_with_existing_ancestor(&self.agent_definition_dir);
        if path_starts_with(&s, &def.to_string_lossy()) {
            return true;
        }
        for r in &self.workspace_roots {
            let rc = canonicalize_with_existing_ancestor(r);
            if path_starts_with(&s, &rc.to_string_lossy()) {
                return true;
            }
        }
        false
    }

    /// 路径是否在 agent_trail_readonly_dirs 中（仅 read）。
    fn in_agent_readonly_set(&self, target: &Path) -> bool {
        let s = target.to_string_lossy();
        self.agent_trail_readonly_dirs.iter().any(|r| {
            let rc = canonicalize_with_existing_ancestor(r);
            path_starts_with(&s, &rc.to_string_lossy())
        })
    }

    /// 路径是否在 `~/.tomcat/plans` 默认计划目录内。
    fn in_agent_plans_set(&self, target: &Path) -> bool {
        let Some(plans_dir) = agent_plans_dir_path() else {
            return false;
        };
        let s = target.to_string_lossy();
        let plans = canonicalize_with_existing_ancestor(&plans_dir);
        path_starts_with(&s, &plans.to_string_lossy())
    }

    fn path_rules_snapshot(&self) -> Vec<PathRule> {
        let mut rules = self.path_rules.clone();
        rules.extend(self.session_path_rules.snapshot());
        rules
    }

    /// 命中第一条 Deny 规则；否则返回第一条 Readonly 命中（用于 read 通过）。
    fn match_path_rule(&self, target: &Path) -> Option<PathRule> {
        let rules = self.path_rules_snapshot();
        // 先 Deny（最高优先级）。
        if let Some(r) = rules
            .iter()
            .find(|r| r.mode == PathRuleMode::Deny && r.matches(target))
        {
            return Some(r.clone());
        }
        // 再 Readonly。
        rules
            .iter()
            .find(|r| r.mode == PathRuleMode::Readonly && r.matches(target))
            .cloned()
    }
}

impl PermissionGate for DefaultPermissionGate {
    fn check(&self, op: PrimitiveOperation, path: &str) -> Result<PermissionDecision, AppError> {
        let target = self.normalize(path)?;

        // ── Layer 1：path_rules Deny / Readonly+write ──
        if let Some(rule) = self.match_path_rule(&target) {
            match rule.mode {
                PathRuleMode::Deny => {
                    return Ok(PermissionDecision::Deny {
                        reason: format!("path_rule deny: {}", rule.path),
                    });
                }
                PathRuleMode::Readonly => {
                    if matches!(
                        op,
                        PrimitiveOperation::Write
                            | PrimitiveOperation::Edit
                            | PrimitiveOperation::Bash
                    ) {
                        return Ok(PermissionDecision::Deny {
                            reason: format!("path_rule readonly: {}", rule.path),
                        });
                    }
                    if is_builtin_default_path_rule(&rule) && self.in_agent_readonly_set(&target) {
                        return Ok(PermissionDecision::Allow {
                            grant: GrantTrace::new(
                                GrantType::AgentTrailDir,
                                GrantTrigger::BuiltinDefault,
                            ),
                            scope: PermissionScope::Read,
                        });
                    }
                    // read 通过 readonly。
                    return Ok(PermissionDecision::Allow {
                        grant: GrantTrace::new(
                            GrantType::PathRuleReadOnly,
                            if is_builtin_default_path_rule(&rule) {
                                GrantTrigger::BuiltinDefault
                            } else {
                                GrantTrigger::PathRulesConfig
                            },
                        ),
                        scope: PermissionScope::Read,
                    });
                }
            }
        }

        // ── Layer 3 第一波：在 writable 集合内（agent_definition_dir / workspace_roots） ──
        if self.in_writable_set(&target) {
            return Ok(PermissionDecision::Allow {
                grant: grant_for_writable(
                    &target,
                    &self.agent_definition_dir,
                    &self.workspace_roots,
                ),
                scope: scope_for_op(op),
            });
        }

        // ── Layer 3 第二波：agent_plans_dir 默认授权根 ──
        if self.in_agent_plans_set(&target) {
            return Ok(PermissionDecision::Allow {
                grant: GrantTrace::new(GrantType::AgentPlansDir, GrantTrigger::BuiltinDefault),
                scope: scope_for_op(op),
            });
        }

        // ── Layer 3 第三波：session grant ──
        if self.session_grants.contains(&target) {
            let trigger = self
                .session_grants
                .trigger_for(&target)
                .unwrap_or(GrantTrigger::UserConfirm);
            return Ok(PermissionDecision::Allow {
                grant: GrantTrace::new(GrantType::SessionScope, trigger),
                scope: scope_for_op(op),
            });
        }

        // ── Layer 3 第四波：agent 数据目录（仅 read） ──
        if matches!(op, PrimitiveOperation::Read) && self.in_agent_readonly_set(&target) {
            return Ok(PermissionDecision::Allow {
                grant: GrantTrace::new(GrantType::AgentTrailDir, GrantTrigger::BuiltinDefault),
                scope: PermissionScope::Read,
            });
        }

        // ── Layer 2：NeedConfirm（auto_confirm 短路） ──
        if self.auto_confirm {
            return Ok(PermissionDecision::Allow {
                grant: GrantTrace::new(GrantType::SessionScope, GrantTrigger::AutoConfirmFlag),
                scope: scope_for_op(op),
            });
        }
        let suggested_root = target
            .parent()
            .map(|p| p.to_path_buf())
            .or(Some(target.clone()));
        Ok(PermissionDecision::NeedConfirm {
            reason: format!("路径 `{}` 不在已授权范围内", target.display()),
            suggested_root,
        })
    }

    fn check_bash(&self, command: &str) -> Result<PermissionDecision, AppError> {
        // Layer 1：bash_forbidden（builtin ∪ user）。
        for re in &self.bash_forbidden {
            if re.is_match(command) {
                return Ok(PermissionDecision::Deny {
                    reason: format!("bash_forbidden 命中: {}", re.as_str()),
                });
            }
        }

        // Layer 2：bash_approval_required（命中弹 confirm）。
        for re in &self.bash_approval {
            if re.is_match(command) {
                if self.auto_confirm {
                    return Ok(PermissionDecision::Allow {
                        grant: GrantTrace::new(
                            GrantType::BashPolicy,
                            GrantTrigger::AutoConfirmFlag,
                        ),
                        scope: PermissionScope::BashApproval,
                    });
                }
                return Ok(PermissionDecision::NeedConfirm {
                    reason: format!("bash_approval_required 命中: {}", re.as_str()),
                    suggested_root: None,
                });
            }
        }

        // 默认：bash 命令本身不强制 confirm（路径检查由调用方在 parse 后逐一调用 `check`）。
        Ok(PermissionDecision::Allow {
            grant: GrantTrace::new(GrantType::BashPolicy, GrantTrigger::BashRegexConfig),
            scope: PermissionScope::Bash,
        })
    }

    fn effective_roots(&self) -> EffectiveRoots {
        let mut read_write = vec![self.agent_definition_dir.clone()];
        if let Some(plans_dir) = agent_plans_dir_path() {
            read_write.push(plans_dir);
        }
        read_write.extend(self.workspace_roots.iter().cloned());
        read_write.extend(self.session_grants.snapshot());

        let mut read_only = Vec::new();
        read_only.extend(self.agent_trail_readonly_dirs.clone());
        for r in self.path_rules_snapshot() {
            if r.mode == PathRuleMode::Readonly {
                if let Ok(s) = r.expanded_path() {
                    read_only.push(PathBuf::from(s));
                }
            }
        }
        EffectiveRoots {
            read_write,
            read_only,
        }
    }

    fn effective_path_rules(&self) -> Vec<PathRule> {
        self.path_rules_snapshot()
    }

    fn grant_session(&self, path: PathBuf, trigger: GrantTrigger) {
        self.session_grants.add(path, trigger);
    }

    fn grant_path_rule(&self, rule: PathRule) {
        self.session_path_rules.add(rule);
    }
}

fn is_builtin_default_path_rule(rule: &PathRule) -> bool {
    BUILTIN_DEFAULT_PATH_RULES
        .iter()
        .any(|(path, mode)| *path == rule.path && *mode == rule.mode)
}

fn agent_plans_dir_path() -> Option<PathBuf> {
    crate::infra::config::resolve_plans_dir()
        .ok()
        .map(|path| canonicalize_with_existing_ancestor(&path))
}

// ─────────────────────────────────────────────────────────────────────────────
// 辅助
// ─────────────────────────────────────────────────────────────────────────────

fn scope_for_op(op: PrimitiveOperation) -> PermissionScope {
    match op {
        PrimitiveOperation::Read => PermissionScope::Read,
        PrimitiveOperation::Write | PrimitiveOperation::Edit => PermissionScope::Write,
        PrimitiveOperation::Bash => PermissionScope::Bash,
    }
}

fn grant_for_writable(
    target: &Path,
    agent_definition_dir: &Path,
    workspace_roots: &[PathBuf],
) -> GrantTrace {
    let s = target.to_string_lossy();
    let def = canonicalize_with_existing_ancestor(agent_definition_dir);
    if path_starts_with(&s, &def.to_string_lossy()) {
        return GrantTrace::new(GrantType::AgentDefinitionDir, GrantTrigger::BuiltinDefault);
    }
    for r in workspace_roots {
        let rc = canonicalize_with_existing_ancestor(r);
        if path_starts_with(&s, &rc.to_string_lossy()) {
            return GrantTrace::new(
                GrantType::AgentWorkspaceRoot,
                GrantTrigger::WorkspaceRootsConfig,
            );
        }
    }
    GrantTrace::new(GrantType::AgentDefinitionDir, GrantTrigger::BuiltinDefault)
}

/// 编译 `builtin ∪ user` 两层 regex；非法 regex 静默跳过 + tracing warn，
/// 避免一条坏配置牵连整个权限系统。
fn compile_regex_list(builtin: &[&str], user: &[String]) -> Vec<Regex> {
    let mut out = Vec::with_capacity(builtin.len() + user.len());
    for s in builtin {
        match Regex::new(s) {
            Ok(re) => out.push(re),
            Err(e) => tracing::warn!(target: "permission", "builtin regex 编译失败: {} ({})", s, e),
        }
    }
    for s in user {
        match Regex::new(s) {
            Ok(re) => out.push(re),
            Err(e) => tracing::warn!(target: "permission", "user regex 编译失败: {} ({})", s, e),
        }
    }
    out
}

/// 用 globset 把 `~/...` 风格的 glob 编译为 matcher（供调用方临时使用）。
#[allow(dead_code)]
pub(crate) fn compile_glob(pattern: &str) -> Option<globset::GlobMatcher> {
    Glob::new(pattern).ok().map(|g| g.compile_matcher())
}

/// 找到 `path` 最长存在的祖先并 canonicalize，再拼回剩余的子路径。
///
/// 例如 `/var/tmp/foo/k.txt`（k.txt 不存在）：canonicalize `/var/tmp/foo` ->
/// `/private/var/tmp/foo`，最终返回 `/private/var/tmp/foo/k.txt`。
pub(crate) fn canonicalize_with_existing_ancestor(path: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(path) {
        return c;
    }
    let mut tail = Vec::new();
    let mut cur = path.to_path_buf();
    while let Some(parent) = cur.parent().map(|p| p.to_path_buf()) {
        if let Some(name) = cur.file_name() {
            tail.push(name.to_os_string());
        }
        if parent.as_os_str().is_empty() {
            break;
        }
        if let Ok(c) = std::fs::canonicalize(&parent) {
            let mut out = c;
            for seg in tail.iter().rev() {
                out.push(seg);
            }
            return out;
        }
        cur = parent;
    }
    path.to_path_buf()
}
