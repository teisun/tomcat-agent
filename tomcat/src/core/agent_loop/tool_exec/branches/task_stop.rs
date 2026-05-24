use std::sync::Arc;

use crate::core::tools::primitive::BashTaskRegistry;

pub(in super::super) async fn handle_task_stop(
    registry: &Option<Arc<BashTaskRegistry>>,
    args: &serde_json::Value,
) -> Result<String, String> {
    let Some(registry) = registry.as_ref() else {
        return Err("task_stop 未启用：未注入 BashTaskRegistry".to_string());
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
