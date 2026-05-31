use crate::core::agent_loop::types::SubagentType;

pub(super) fn bash_background_unavailable(_tool_name: &str, subagent_type: SubagentType) -> String {
    match subagent_type {
        SubagentType::Reviewer | SubagentType::Verifier => {
            "`bash(run_in_background=true)` is currently unsupported in this subagent. Use foreground `bash` instead. Do not call `task_output`, `task_stop`, or `task_list`.".to_string()
        }
        SubagentType::User => {
            "Background bash is not enabled in this AgentLoop. Use foreground `bash` instead. Do not call `bash(run_in_background=true)`, `task_output`, `task_stop`, or `task_list`.".to_string()
        }
    }
}
