use std::sync::Arc;

use crate::core::agent_loop::types::SubagentType;
use crate::core::tools::primitive::BashTaskRegistry;

pub(in super::super) async fn handle_task_stop(
    registry: &Option<Arc<BashTaskRegistry>>,
    subagent_type: SubagentType,
    args: &serde_json::Value,
) -> Result<String, String> {
    let Some(registry) = registry.as_ref() else {
        return Err(super::background_unavailable::bash_background_unavailable(
            "task_stop",
            subagent_type,
        ));
    };
    let task_id = args
        .get("task_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "task_stop 缺少 task_id".to_string())?;
    registry
        .stop(task_id)
        .await
        .map(|_| format!("已停止: {}", task_id))
        .map_err(|e| e.to_string())
}
