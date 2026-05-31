use std::path::PathBuf;
use std::sync::Arc;

use crate::core::agent_loop::types::SubagentType;
use crate::core::tools::primitive::BashTaskRegistry;

pub(in super::super) async fn handle_bash_background(
    registry: &Option<Arc<BashTaskRegistry>>,
    subagent_type: SubagentType,
    command: &str,
    cwd: Option<&str>,
    argv: Option<Vec<String>>,
) -> Result<String, String> {
    let Some(registry) = registry.as_ref() else {
        return Err(super::background_unavailable::bash_background_unavailable(
            "bash",
            subagent_type,
        ));
    };
    let cwd_pb = cwd.map(PathBuf::from);
    registry
        .spawn(command.to_string(), argv, cwd_pb)
        .await
        .map(|t| serde_json::to_string(&t).unwrap_or_else(|_| "{}".to_string()))
        .map_err(|e| e.to_string())
}
