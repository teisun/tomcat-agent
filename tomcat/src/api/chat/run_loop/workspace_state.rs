use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::api::chat::ChatContext;
use crate::core::agent_loop::EphemeralTailProvider;
use crate::core::llm::system_prompt::{PathRuleSummary, WorkspaceRootDescriptor, WorkspaceState};
use crate::core::llm::system_prompt::{
    SystemPromptSection, WorkspaceContext, WorkspaceStateSection,
};
use crate::core::permission::{PathRuleMode, PermissionGate, SessionGrants};
use crate::resolve_workspace_roots_paths;

pub(crate) fn runtime_tail_provider(ctx: &ChatContext) -> Arc<dyn EphemeralTailProvider> {
    Arc::new(RuntimeTailProvider {
        config: ctx.config.clone(),
        agent_definition_dir: ctx.scope_services.agent_definition_dir.clone(),
        agent_trail_dir: ctx.scope_services.agent_trail_dir.clone(),
        session_grants: ctx.session_runtime.session_grants.clone(),
        gate: Arc::clone(&ctx.global_services.gate),
        plan_runtime: Arc::clone(&ctx.session_runtime.plan_runtime),
    })
}

pub(crate) fn render_plan_runtime_reminder(
    plan_runtime: &crate::core::plan_runtime::PlanRuntime,
) -> Option<String> {
    match plan_runtime.mode() {
        crate::core::session::AgentMode::Plan => {
            Some((*crate::core::plan_runtime::reminders::PLANNER_REMINDER).to_string())
        }
        crate::core::session::AgentMode::Chat => plan_runtime.executing_plan_id().map(|plan_id| {
            crate::core::plan_runtime::reminders::render_executor_reminder(&plan_id)
        }),
    }
}

struct RuntimeTailProvider {
    config: crate::AppConfig,
    agent_definition_dir: PathBuf,
    agent_trail_dir: PathBuf,
    session_grants: SessionGrants,
    gate: Arc<dyn PermissionGate>,
    plan_runtime: Arc<crate::core::plan_runtime::PlanRuntime>,
}

impl EphemeralTailProvider for RuntimeTailProvider {
    fn render_ephemeral_tail(&self) -> String {
        let mut sections = Vec::new();
        if let Some(reminder) = render_plan_runtime_reminder(&self.plan_runtime) {
            sections.push(reminder);
        }

        let state = compute_workspace_state(
            &self.config,
            &self.agent_definition_dir,
            &self.agent_trail_dir,
            &self.session_grants,
            self.gate.as_ref(),
        );
        let context = WorkspaceContext {
            agent_workspace_dir: String::new(),
            agent_definition_dir: self.agent_definition_dir.to_string_lossy().to_string(),
            agent_plans_dir: crate::core::plan_runtime::file_store::plans_dir()
                .map(|path| crate::infra::platform::format_home_path(path.as_path()))
                .unwrap_or_else(|_| "~/.tomcat/plans".to_string()),
            agent_trail_dir: self.agent_trail_dir.to_string_lossy().to_string(),
            tool_lines: None,
        };
        let rendered_state = WorkspaceStateSection::new(state).render(&context);
        sections.push(format!(
            "<system_reminder kind=\"workspace_state\">\n{rendered_state}\n</system_reminder>"
        ));
        sections.join("\n\n")
    }
}

fn compute_workspace_state(
    cfg: &crate::AppConfig,
    agent_definition_dir: &Path,
    agent_trail_dir: &Path,
    session_grants: &SessionGrants,
    gate: &dyn PermissionGate,
) -> WorkspaceState {
    let workspace_roots = resolve_workspace_roots_paths(cfg).unwrap_or_default();
    let agent_plans_dir = crate::infra::config::resolve_plans_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string());

    let agent_trail_readonly_dirs: Vec<PathBuf> = vec![
        Some(agent_trail_dir.to_path_buf()),
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
    let session_set: HashSet<String> = session_grants
        .snapshot()
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let effective_roots = gate.effective_roots();
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
    for rule in gate.effective_path_rules() {
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

#[cfg(test)]
#[path = "tests/workspace_state_test.rs"]
mod tests;
