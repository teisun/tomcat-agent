use crate::core::plan_runtime::state::PlanState;

fn user_prompt_label(mode: &PlanState) -> &'static str {
    match mode {
        PlanState::Chat => "Chat",
        PlanState::Planning => "Plan:planning",
        PlanState::Executing { .. } => "Plan:executing",
        PlanState::Pending { .. } => "Plan:pending",
        PlanState::Completed { .. } => "Chat",
    }
}

fn agent_prompt_label(mode: &PlanState) -> Option<&'static str> {
    match mode {
        PlanState::Chat => None,
        PlanState::Planning => Some("Plan:planning"),
        PlanState::Executing { .. } => Some("Plan:executing"),
        PlanState::Pending { .. } => Some("Plan:pending"),
        PlanState::Completed { .. } => None,
    }
}

pub(crate) fn user_prompt_for_mode(mode: &PlanState) -> String {
    format!("u[{}]> ", user_prompt_label(mode))
}

pub(crate) fn user_prompt_for_mode_with_model(mode: &PlanState, model: &str) -> String {
    let model = model.trim();
    if model.is_empty() {
        return user_prompt_for_mode(mode);
    }
    format!("u[{}|{}]> ", user_prompt_label(mode), model)
}

pub(crate) fn agent_prompt_for_mode(agent_id: &str, mode: &PlanState) -> String {
    match agent_prompt_label(mode) {
        Some(label) => format!("agent.{agent_id}[{label}]> "),
        None => format!("agent.{agent_id}> "),
    }
}
