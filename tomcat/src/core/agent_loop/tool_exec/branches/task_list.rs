use std::sync::Arc;

use crate::core::agent_loop::types::SubagentType;
use crate::core::tools::primitive::BashTaskRegistry;

pub(in super::super) async fn handle_task_list(
    registry: &Option<Arc<BashTaskRegistry>>,
    subagent_type: SubagentType,
) -> Result<String, String> {
    let Some(registry) = registry.as_ref() else {
        return Err(super::background_unavailable::bash_background_unavailable(
            "task_list",
            subagent_type,
        ));
    };
    let infos = registry.list();
    Ok(serde_json::to_string(&infos).unwrap_or_else(|_| "[]".to_string()))
}
