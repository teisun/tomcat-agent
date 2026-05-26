use crate::core::plan_runtime::mode::PlanMode;

fn user_prompt_label(mode: &PlanMode) -> &'static str {
    match mode {
        PlanMode::Chat => "Chat",
        PlanMode::Planning => "Plan:planning",
        PlanMode::Executing { .. } => "Plan:executing",
        PlanMode::Pending { .. } => "Plan:pending",
        PlanMode::Completed { .. } => "Plan:completed",
    }
}

fn agent_prompt_label(mode: &PlanMode) -> Option<&'static str> {
    match mode {
        PlanMode::Chat => None,
        PlanMode::Planning => Some("Plan:planning"),
        PlanMode::Executing { .. } => Some("Plan:executing"),
        PlanMode::Pending { .. } => Some("Plan:pending"),
        PlanMode::Completed { .. } => Some("Plan:completed"),
    }
}

pub(crate) fn user_prompt_for_mode(mode: &PlanMode) -> String {
    format!("u[{}]> ", user_prompt_label(mode))
}

pub(crate) fn agent_prompt_for_mode(agent_id: &str, mode: &PlanMode) -> String {
    match agent_prompt_label(mode) {
        Some(label) => format!("agent.{agent_id}[{label}]> "),
        None => format!("agent.{agent_id}> "),
    }
}
