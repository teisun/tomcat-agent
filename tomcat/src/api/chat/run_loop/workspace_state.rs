use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::api::chat::ChatContext;
use crate::core::llm::system_prompt::{PathRuleSummary, WorkspaceRootDescriptor, WorkspaceState};
use crate::core::permission::PathRuleMode;
use crate::resolve_workspace_roots_paths;

pub(super) fn compute_workspace_state(ctx: &ChatContext) -> WorkspaceState {
    let cfg = &ctx.config;
    let agent_definition_dir = ctx.scope_services.agent_definition_dir.clone();
    let workspace_roots = resolve_workspace_roots_paths(cfg).unwrap_or_default();
    let agent_plans_dir = crate::infra::config::resolve_plans_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string());

    let agent_trail_readonly_dirs: Vec<PathBuf> = vec![
        Some(ctx.scope_services.agent_trail_dir.clone()),
        crate::infra::config::resolve_sessions_dir(cfg).ok(),
        crate::infra::config::resolve_log_dir(cfg).ok(),
        crate::infra::config::resolve_audit_dir(cfg).ok(),
        crate::infra::config::resolve_agent_dir(cfg).ok(),
    ]
    .into_iter()
    .flatten()
    .collect();

    let mut entry_meta: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    for entry in &cfg.workspace.entries {
        if !entry.path.trim().is_empty() {
            let key = crate::infra::platform::normalize_path(&entry.path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| entry.path.clone());
            entry_meta.insert(key, (entry.alias.clone(), entry.description.clone()));
        }
    }

    let agent_definition_canon = agent_definition_dir.to_string_lossy().to_string();
    let workspace_root_set: HashSet<String> = workspace_roots
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let session_set: HashSet<String> = ctx
        .session_runtime
        .session_grants
        .snapshot()
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let effective_roots = ctx.global_services.gate.effective_roots();
    let mut read_write = Vec::new();
    let mut seen_rw = HashSet::new();
    for path in effective_roots.read_write {
        let path_string = path.to_string_lossy().to_string();
        if !seen_rw.insert(path_string.clone()) {
            continue;
        }
        let label = if path_string == agent_definition_canon {
            "agent_definition_dir"
        } else if workspace_root_set.contains(&path_string) {
            "agent_workspace_root"
        } else if session_set.contains(&path_string) {
            "session_grant"
        } else {
            "workspace_root"
        };
        let (alias, description) = entry_meta
            .get(&path_string)
            .cloned()
            .unwrap_or((None, None));
        read_write.push(WorkspaceRootDescriptor {
            path: path_string,
            label: label.to_string(),
            alias,
            description,
        });
    }

    let mut read_only = Vec::new();
    let mut seen_ro = HashSet::new();
    let agent_trail_set: HashSet<String> = agent_trail_readonly_dirs
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    for path in effective_roots.read_only {
        let path_string = path.to_string_lossy().to_string();
        if !seen_ro.insert(path_string.clone()) {
            continue;
        }
        let label = if agent_trail_set.contains(&path_string) {
            "agent_trail_dir"
        } else if agent_plans_dir.as_deref() == Some(&path_string) {
            "agent_plans_dir"
        } else {
            "path_rule_readonly"
        };
        read_only.push(WorkspaceRootDescriptor {
            path: path_string,
            label: label.to_string(),
            alias: None,
            description: None,
        });
    }

    let user_paths: HashSet<String> = cfg
        .primitive
        .path_rules
        .iter()
        .map(|rule| rule.path.clone())
        .collect();
    let mut path_rules = Vec::new();
    for rule in ctx.global_services.gate.effective_path_rules() {
        path_rules.push(PathRuleSummary {
            path: rule.path.clone(),
            mode: match rule.mode {
                PathRuleMode::Deny => "deny".to_string(),
                PathRuleMode::Readonly => "readonly".to_string(),
            },
            builtin: !user_paths.contains(&rule.path),
        });
    }

    WorkspaceState {
        read_write,
        read_only,
        path_rules,
    }
}
