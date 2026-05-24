use std::sync::Arc;

use crate::core::tools::primitive::BashTaskRegistry;

pub(in super::super) async fn handle_task_list(
    registry: &Option<Arc<BashTaskRegistry>>,
) -> Result<String, String> {
    let Some(registry) = registry.as_ref() else {
        return Err("task_list 未启用：未注入 BashTaskRegistry".to_string());
    };
    let infos = registry.list();
    Ok(serde_json::to_string(&infos).unwrap_or_else(|_| "[]".to_string()))
}
