//! Parent-facing projection. A running row lives on its own handle; only the
//! final row moves to the parent. Reports are never retained by this registry.
use super::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Running,
    Stopping,
    Completed,
    Failed,
    Stopped,
    Interrupted,
}
impl TaskState {
    pub fn active(self) -> bool {
        matches!(self, Self::Running | Self::Stopping)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChildRow {
    pub id: String,
    #[serde(rename = "type")]
    pub role: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub state: TaskState,
    pub activity: String,
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    pub revision: u64,
}
impl ChildRow {
    pub(super) fn new(
        id: &str,
        role: SubagentType,
        description: &str,
        tool_call_id: Option<&str>,
    ) -> Self {
        Self {
            id: id.into(),
            role: role.as_str().into(),
            description: description.into(),
            tool_call_id: tool_call_id.map(str::to_owned),
            state: TaskState::Running,
            activity: String::new(),
            started_at: chrono::Utc::now().timestamp_millis(),
            ended_at: None,
            revision: 1,
        }
    }
    pub fn wire(&self) -> serde_json::Value {
        let mut row = serde_json::to_value(self).expect("serializable child row");
        row["kind"] = "subagent".into();
        row
    }
}
impl AgentRegistry {
    pub fn child_rows(&self, parent: &str) -> Vec<ChildRow> {
        let handles = self.handles.read();
        let Some(root) = handles.get(parent) else {
            return vec![];
        };
        let mut rows = root.finished.lock().clone();
        rows.extend(
            root.children
                .lock()
                .iter()
                .filter_map(|id| handles.get(id)?.row.lock().clone()),
        );
        rows.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));
        rows
    }
    pub fn stop_child(&self, parent: &str, id: &str) -> Result<ChildRow, String> {
        let handles = self.handles.read();
        let root = handles.get(parent).ok_or("unknown_child")?;
        let Some(child) = handles
            .get(id)
            .filter(|c| c.parent_session_id.as_deref() == Some(parent))
        else {
            return root
                .finished
                .lock()
                .iter()
                .find(|r| r.id == id)
                .cloned()
                .ok_or_else(|| "unknown_child".into());
        };
        let mut slot = child.row.lock();
        let r = slot.as_mut().ok_or("unknown_child")?;
        let changed = r.state == TaskState::Running;
        if changed {
            r.state = TaskState::Stopping;
            r.revision += 1;
            child.cancel();
        }
        let row = r.clone();
        drop(slot);
        drop(handles);
        if changed {
            self.emit_row(parent, &row);
        }
        Ok(row)
    }
    pub(crate) fn record_activity(&self, parent: &str, id: &str, activity: &str) {
        let handles = self.handles.read();
        let Some(child) = handles
            .get(id)
            .filter(|c| c.parent_session_id.as_deref() == Some(parent))
        else {
            return;
        };
        let mut slot = child.row.lock();
        let Some(r) = slot.as_mut() else { return };
        if r.state != TaskState::Running || r.activity == activity {
            return;
        }
        r.activity = activity.into();
        r.revision += 1;
        let row = r.clone();
        drop(slot);
        drop(handles);
        self.emit_row(parent, &row);
    }
    pub(super) fn emit_row(&self, parent: &str, r: &ChildRow) {
        if let Some(bus) = &self.event_bus {
            let _ = ScopedEventEmitter::new(bus.clone(), parent).emit_payload(
                "background.updated",
                serde_json::json!({"type":"background.updated","row":r.wire()}),
            );
        }
    }
    pub(crate) fn bind_terminal_sink(
        &self,
        id: &str,
        sink: Option<Arc<dyn crate::core::session::MessageAppendSink>>,
    ) {
        if let Some(c) = self.handles.read().get(id) {
            *c.terminal_sink.lock() = sink;
        }
    }
    pub(crate) fn set_terminal_report(&self, id: &str, report: &str) {
        if let Some(c) = self.handles.read().get(id) {
            *c.terminal_report.lock() = Some(report.chars().take(16_032).collect());
        }
    }
    pub(super) fn finish_row(
        &self,
        child: &AgentHandle,
        label: SubagentOutcomeLabel,
    ) -> Option<ChildRow> {
        let mut slot = child.row.lock();
        let r = slot.as_mut()?;
        r.state = if r.state == TaskState::Stopping || child.is_aborted() {
            TaskState::Stopped
        } else {
            match label {
                SubagentOutcomeLabel::Completed => TaskState::Completed,
                SubagentOutcomeLabel::Failed => TaskState::Failed,
                SubagentOutcomeLabel::Interrupted => TaskState::Stopped,
            }
        };
        r.activity.clear();
        r.ended_at = Some(chrono::Utc::now().timestamp_millis());
        r.revision += 1;
        if let Some(sink) = child.terminal_sink.lock().take() {
            let report = child
                .terminal_report
                .lock()
                .take()
                .filter(|_| matches!(r.state, TaskState::Completed | TaskState::Failed));
            if let Err(e) = sink.append_custom_entry(
                serde_json::json!({"event":"subagent.finished","row":r,"report":report}),
            ) {
                tracing::warn!(error=%e,"could not persist child result");
            }
        }
        Some(r.clone())
    }
}
