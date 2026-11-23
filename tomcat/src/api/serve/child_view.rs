//! Read-only streams share the parent's bus, but never its slot, controller or input queue.
use super::{event_pump::MessageReplay, types::OutFrame, ServeState, SessionSlot};
use crate::{core::session::TranscriptEntry, AppError};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

#[derive(Default)]
pub(super) struct ChildStream {
    replay: MessageReplay,
    viewers: HashMap<uuid::Uuid, Weak<ServeState>>,
}

/// Allocate replay only while a child exists, and release all subscriptions on its terminal row.
pub(super) fn row_changed(slot: &SessionSlot, row: &Value) {
    if row["kind"] != "subagent" {
        return;
    }
    let Some(id) = row["id"].as_str() else { return };
    let mut streams = slot.child_streams.lock();
    match row["state"].as_str() {
        Some("running") if row["revision"] == 1 => {
            streams.entry(id.into()).or_default();
        }
        Some("completed" | "failed" | "stopped" | "interrupted") => {
            streams.remove(id);
        }
        _ => {}
    }
}
pub(super) fn forward(slot: &SessionSlot, id: &str, payload: &Value) {
    let mut streams = slot.child_streams.lock();
    let Some(stream) = streams.get_mut(id) else {
        return;
    };
    // Replay only consumes the public content / thinking deltas.
    stream.replay.record(payload);
    if stream.viewers.is_empty() {
        return;
    }
    let Some(event) = public_event(payload) else {
        return;
    };
    stream.viewers.retain(|_, weak| {
        weak.upgrade().is_some_and(|c| {
            !c.is_closed()
                && c.active_session_id().as_deref() == Some(&slot.session_id)
                && (c.controls(slot) || c.observes(slot))
                && c.writer.send(OutFrame::Event(event.clone())).is_ok()
        })
    });
}
impl ServeState {
    pub(super) fn close_child_view(&self) {
        let old = self.child_view.lock().take();
        if let Some((parent, child)) = old {
            if let Some(slot) = self.registry.get(&parent) {
                if let Some(s) = slot.child_streams.lock().get_mut(&child) {
                    s.viewers.remove(&self.connection_id);
                }
            }
        }
    }
    pub(super) fn watch_child(
        self: &Arc<Self>,
        slot: &SessionSlot,
        id: &str,
    ) -> Result<(), AppError> {
        self.close_child_view();
        let mut view = self.child_view.lock();
        let mut streams = slot.child_streams.lock();
        if let Some(s) = streams.get_mut(id) {
            s.replay.send(id, &self.writer)?;
            s.viewers.insert(self.connection_id, Arc::downgrade(self));
            *view = Some((slot.session_id.clone(), id.into()));
        }
        Ok(())
    }
}
/// Enrich only children referenced by start entries on this history page; never scan the directory.
pub(super) fn page_rows(
    root: &std::path::Path,
    parent: &str,
    entries: &[TranscriptEntry],
    registry: &crate::core::agent_registry::AgentRegistry,
) -> Vec<Value> {
    use crate::core::session::{
        subagent_transcript::owned_child_path, transcript::read_entries_tail,
    };
    let live = registry.child_rows(parent);
    let mut seen = std::collections::HashSet::new();
    entries.iter().filter_map(|e| {
        let TranscriptEntry::Custom(c) = e else { return None; };
        let v = &c.extra;
        let role=match v["event"].as_str()?{"plan.explorer.started"=>"explorer","plan.review.started" | "plan.review"=>"plan_reviewer",_=>return None};
        let id=v["child_session_id"].as_str().or(v["childSessionId"].as_str())?;
        if !id.starts_with(&format!("{parent}-child-")) || !seen.insert(id) {return None;}
        if let Some(row)=live.iter().find(|r|r.id==id){return Some(row.wire());}
        let terminal=owned_child_path(root,parent,id).ok().and_then(|p|read_entries_tail(&p,1).ok()).and_then(|mut tail| match tail.pop(){Some(TranscriptEntry::Custom(end)) if end.extra["event"]=="subagent.finished"=>serde_json::from_value::<crate::core::agent_registry::rows::ChildRow>(end.extra["row"].clone()).ok(),_=>None});
        if let Some(row)=terminal.filter(|r|r.id==id && !r.state.active()){return Some(row.wire());}
        Some(serde_json::json!({"kind":"subagent","id":id,"type":role,"description":v["task_id"].as_str().or(v["taskId"].as_str()).unwrap_or(role),"toolCallId":v.get("tool_call_id").or(v.get("toolCallId")),"state":"interrupted","activity":"","revision":0,"startedAt":chrono::DateTime::parse_from_rfc3339(&c.timestamp).ok().map(|d|d.timestamp_millis())}))
    }).collect()
}
fn public_message(v: &mut Value) -> bool {
    if v["role"] == "system" {
        return false;
    }
    if let Some(obj) = v.as_object_mut() {
        obj.retain(|k, _| {
            matches!(
                k.as_str(),
                "role"
                    | "content"
                    | "tool_calls"
                    | "tool_call_id"
                    | "is_error"
                    | "name"
                    | "thinking_text"
                    | "tool_display"
                    | "summary_title"
                    | "finish_reason"
                    | "error_message"
                    | "kind"
            )
        });
    }
    true
}
pub(super) fn public_entry(entry: &mut TranscriptEntry) -> bool {
    match entry {
        TranscriptEntry::Message(m) => public_message(&mut m.message),
        TranscriptEntry::ThinkingTrace(t) => {
            t.signature = None;
            true
        }
        TranscriptEntry::BranchSummary(_)
        | TranscriptEntry::BranchSummaryText(_)
        | TranscriptEntry::Error(_) => true,
        _ => false,
    }
}
fn public_event(payload: &Value) -> Option<Value> {
    let mut p = payload.clone();
    match p["type"].as_str()? {
        "message_start" | "message_update" | "message_end" | "turn_end" => {
            if !public_message(&mut p["message"]) {
                return None;
            }
            if let Some(obj) = p
                .get_mut("assistantMessageEvent")
                .and_then(Value::as_object_mut)
            {
                obj.retain(|k, _| matches!(k.as_str(), "kind" | "delta"));
            }
            if let Some(results) = p.get_mut("toolResults").and_then(Value::as_array_mut) {
                results.retain_mut(public_message);
            }
        }
        "agent_end" => {
            p["messages"] = serde_json::json!([]);
        }
        "agent_start"
        | "turn_start"
        | "tool_execution_start"
        | "tool_call_streaming"
        | "tool_execution_update"
        | "tool_execution_end"
        | "auto_compaction_start"
        | "auto_compaction_end"
        | "foreground_compaction_start"
        | "foreground_compaction_end"
        | "session.todos" => {}
        _ => return None,
    }
    Some(p)
}
