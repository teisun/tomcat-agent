use crate::core::plan_runtime::{file_store::PlanFileState, ActivePlan};
use crate::core::session::AgentMode;
use crate::infra::i18n::tr;

fn user_prompt_label(mode: AgentMode, active_plan: Option<&ActivePlan>) -> String {
    let mode = match mode {
        AgentMode::Chat => tr("term.mode.chat", &[]),
        AgentMode::Plan => tr("term.mode.plan", &[]),
    };
    match active_plan.map(|plan| plan.state) {
        Some(PlanFileState::Executing) => {
            format!("{mode}·plan:{}", tr("term.planState.executing", &[]))
        }
        Some(PlanFileState::Pending) => {
            format!("{mode}·plan:{}", tr("term.planState.pending", &[]))
        }
        _ => mode,
    }
}

fn agent_prompt_label(mode: AgentMode, active_plan: Option<&ActivePlan>) -> Option<String> {
    if mode == AgentMode::Chat
        && !matches!(
            active_plan.map(|plan| plan.state),
            Some(PlanFileState::Executing | PlanFileState::Pending)
        )
    {
        None
    } else {
        Some(user_prompt_label(mode, active_plan))
    }
}

pub(crate) fn user_prompt_for_mode(mode: AgentMode, active_plan: Option<&ActivePlan>) -> String {
    format!("u[{}]> ", user_prompt_label(mode, active_plan))
}

pub(crate) fn user_prompt_for_mode_with_model(
    mode: AgentMode,
    active_plan: Option<&ActivePlan>,
    model: &str,
) -> String {
    let model = model.trim();
    if model.is_empty() {
        return user_prompt_for_mode(mode, active_plan);
    }
    format!("u[{}|{}]> ", user_prompt_label(mode, active_plan), model)
}

pub(crate) fn agent_prompt_for_mode(
    agent_id: &str,
    mode: AgentMode,
    active_plan: Option<&ActivePlan>,
) -> String {
    match agent_prompt_label(mode, active_plan) {
        Some(label) => format!("agent.{agent_id}[{label}]> "),
        None => format!("agent.{agent_id}> "),
    }
}
