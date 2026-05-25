use crate::core::plan_runtime::mode::PlanMode;

fn mode_label(mode: &PlanMode) -> Option<&'static str> {
    match mode {
        PlanMode::Chat => None,
        PlanMode::Planning => Some("Plan"),
        PlanMode::Executing { .. } => Some("Exec"),
        PlanMode::Pending { .. } => Some("Pending"),
        PlanMode::Completed { .. } => Some("Done"),
    }
}

pub(crate) fn user_prompt_for_mode(mode: &PlanMode) -> String {
    match mode_label(mode) {
        Some(label) => format!("u[{label}]> "),
        None => "u> ".to_string(),
    }
}

pub(crate) fn agent_prompt_for_mode(agent_id: &str, mode: &PlanMode) -> String {
    match mode_label(mode) {
        Some(label) => format!("agent.{agent_id}[{label}]> "),
        None => format!("agent.{agent_id}> "),
    }
}
