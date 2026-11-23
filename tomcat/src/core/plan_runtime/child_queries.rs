use super::PlanRuntime;
use crate::core::{
    agent_registry::{rows::TaskState, AgentRegistry},
    session::{
        subagent_transcript::owned_child_path,
        transcript::{read_entries_tail, TranscriptEntry},
    },
};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
pub(super) struct ChildQueries {
    registry: Arc<AgentRegistry>,
    root: PathBuf,
    parent: String,
}
impl PlanRuntime {
    pub(crate) fn bind_child_queries(
        &self,
        registry: Arc<AgentRegistry>,
        root: PathBuf,
        parent: String,
    ) {
        let _ = self.child_queries.set(ChildQueries {
            registry,
            root,
            parent,
        });
    }
    pub(crate) fn query_child(&self, tool: &str, args: &Value) -> Result<String, String> {
        let q = self
            .child_queries
            .get()
            .ok_or("subagent tracking is unavailable")?;
        let rows = q.registry.child_rows(&q.parent);
        if tool == "subagent_list" {
            return Ok(json!({"agents":rows}).to_string());
        }
        let id = args["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("id is required")?;
        if tool == "subagent_stop" {
            let r = q.registry.stop_child(&q.parent, id)?;
            return Ok(
                json!({"stopping":r.state==TaskState::Stopping,"state":r.state}).to_string(),
            );
        }
        let row = rows.iter().find(|r| r.id == id);
        if let Some(r) = row {
            if r.state.active() || matches!(r.state, TaskState::Stopped | TaskState::Interrupted) {
                return Ok(json!({"id":id,"state":r.state,"activity":r.activity}).to_string());
            }
        }
        let path = owned_child_path(&q.root, &q.parent, id).map_err(|err| {
            if row.is_some() {
                "child transcript unavailable; the report cannot be retrieved".to_owned()
            } else {
                err
            }
        })?;
        let tail = read_entries_tail(&path, 1).map_err(|e| e.to_string())?;
        if let Some(TranscriptEntry::Custom(entry)) = tail.last() {
            if entry.extra["event"] == "subagent.finished" && entry.extra["row"]["id"] == id {
                let state = &entry.extra["row"]["state"];
                let mut result = json!({"id":id,"state":state});
                if matches!(state.as_str(), Some("completed" | "failed")) {
                    result["report"] = entry.extra["report"].clone();
                }
                return Ok(result.to_string());
            }
        }
        if row.is_some() {
            Err("child transcript unavailable; the report cannot be retrieved".into())
        } else {
            Ok(json!({"id":id,"state":"interrupted"}).to_string())
        }
    }
}
