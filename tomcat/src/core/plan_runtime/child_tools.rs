//! The effective schema list is also the execution capability list. Filter
//! inherited definitions rather than rebuilding them (they may be dynamic).
use super::{
    explorer::EXPLORER_ALLOWED_TOOLS, plan_reviewer::plan_reviewer_allowed_tools_with_policy,
    review::resolve_internal_tools,
};
use crate::core::agent_loop::SubagentType;
use crate::core::tools::contract::catalog::builtin_tool_by_name;
use serde_json::Value;

pub(crate) enum ChildToolPolicy<'a> {
    Allow(&'a [&'a str]),
    #[allow(dead_code)] // Worker consumes this policy when it becomes dispatchable.
    InheritExcept(&'a [&'a str]),
}

impl ChildToolPolicy<'_> {
    pub(crate) fn resolve(self, parent: &[Value]) -> Vec<Value> {
        let mut definitions = match self {
            Self::Allow(names) => resolve_internal_tools(names),
            Self::InheritExcept(deny) => parent
                .iter()
                .filter(|v| tool_name(v).is_some_and(|name| !deny.contains(&name)))
                .cloned()
                .collect(),
        };
        definitions.retain(|v| {
            tool_name(v).is_some_and(|name| {
                !matches!(
                    name,
                    "dispatch_agent" | "subagent_list" | "subagent_output" | "subagent_stop"
                ) && !builtin_tool_by_name(name)
                    .is_some_and(|entry| entry.requires_user_interaction)
            })
        });
        definitions
    }
}

pub(crate) fn for_role(role: SubagentType, expose_skills: bool) -> Option<Vec<Value>> {
    match role {
        SubagentType::Explorer => Some(ChildToolPolicy::Allow(EXPLORER_ALLOWED_TOOLS).resolve(&[])),
        SubagentType::PlanReviewer => Some(
            ChildToolPolicy::Allow(&plan_reviewer_allowed_tools_with_policy(expose_skills))
                .resolve(&[]),
        ),
        _ => None,
    }
}

fn tool_name(v: &Value) -> Option<&str> {
    v.get("function")?.get("name")?.as_str()
}

pub(crate) fn denial(role: SubagentType, definitions: &[Value], name: &str) -> Option<String> {
    let label = match role {
        SubagentType::Explorer => "explorer sub-Agent is read-only and",
        SubagentType::PlanReviewer => "reviewer sub-Agent",
        _ => return None,
    };
    if definitions.iter().filter_map(tool_name).any(|n| n == name) {
        return None;
    }
    let allowed = definitions
        .iter()
        .filter_map(tool_name)
        .collect::<Vec<_>>()
        .join("/");
    Some(format!(
        "{label} may not call `{name}` (allowed: {allowed})"
    ))
}
