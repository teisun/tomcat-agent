use std::path::PathBuf;
use std::sync::Arc;

use crate::core::agent_loop::types::SubagentType;
use crate::core::tools::primitive::BashTaskRegistry;

const BACKGROUND_BASH_NEXT_STEP: &str = "If your current step depends on this task, poll task_output(task_id, block=true, wait_ms=...); otherwise keep working and a <background-task-finished> message will be injected automatically when it exits. Do not busy-poll.";

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
        .map(|ticket| serialize_background_ticket(&ticket))
        .map_err(|e| e.to_string())
}

fn serialize_background_ticket(ticket: &crate::core::tools::primitive::BashTaskTicket) -> String {
    let mut value = serde_json::to_value(ticket).unwrap_or(serde_json::Value::Null);
    if let serde_json::Value::Object(ref mut map) = value {
        map.insert(
            "next".to_string(),
            serde_json::Value::String(BACKGROUND_BASH_NEXT_STEP.to_string()),
        );
    }
    serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tools::primitive::BashTaskRegistry;

    #[tokio::test]
    async fn background_ticket_keeps_fields_and_adds_next_hint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = Some(Arc::new(BashTaskRegistry::new(
            dir.path().join("tool-results"),
        )));

        let text = handle_bash_background(&registry, SubagentType::User, "echo queued", None, None)
            .await
            .expect("background bash should queue");

        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert!(value.get("taskId").and_then(|v| v.as_str()).is_some());
        assert!(value.get("logPath").and_then(|v| v.as_str()).is_some());
        assert!(value
            .get("startedAtUnixMs")
            .and_then(|v| v.as_u64())
            .is_some());
        assert_eq!(
            value.get("next").and_then(|v| v.as_str()),
            Some(BACKGROUND_BASH_NEXT_STEP)
        );
    }
}
