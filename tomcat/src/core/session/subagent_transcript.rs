use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use parking_lot::Mutex;
use serde_json::json;
use tracing::warn;

use crate::core::agent_loop::SubagentType;
use crate::core::llm::ChatMessage;
use crate::core::session::manager::{generate_entry_id, MessageAppendSink};
use crate::core::session::tool_display_sidecar::append_tool_display;
use crate::core::session::transcript::{
    append_entry_with_sync, write_header, CustomEntry, MessageEntry, SessionHeader, SyncLevel,
    TranscriptEntry,
};
use crate::infra::error::AppError;
use crate::infra::events::ToolDisplay;

pub(crate) fn open_subagent_transcript(
    agent_trail_dir: &str,
    child_session_id: &str,
    subagent_type: SubagentType,
    model: &str,
    parent_session_id: &str,
) -> Option<Arc<dyn MessageAppendSink>> {
    let path = subagent_transcript_path(agent_trail_dir, child_session_id)?;
    let sink = Arc::new(JsonlFileAppendSink::new(
        path,
        child_session_id,
        subagent_type,
        model,
        parent_session_id,
    ));
    if let Err(error) = sink.ensure_initialized() {
        warn!(
            path = %sink.path.display(),
            error = %error,
            child_session_id = %sink.child_session_id,
            "subagent transcript eager init failed; continuing without persistence"
        );
    }
    Some(sink)
}

pub(crate) fn subagent_transcript_path(
    agent_trail_dir: &str,
    child_session_id: &str,
) -> Option<PathBuf> {
    let agent_trail_dir = agent_trail_dir.trim();
    let child_session_id = child_session_id.trim();
    if agent_trail_dir.is_empty() || child_session_id.is_empty() {
        return None;
    }
    Some(
        Path::new(agent_trail_dir)
            .join("subagent-sessions")
            .join(format!("{child_session_id}.jsonl")),
    )
}

pub(crate) fn format_subagent_transcript_path(
    agent_trail_dir: &str,
    child_session_id: &str,
) -> Option<String> {
    let agent_trail_dir = agent_trail_dir.trim();
    let child_session_id = child_session_id.trim();
    if agent_trail_dir.is_empty() || child_session_id.is_empty() {
        return None;
    }
    Some(format!("subagent-sessions/{child_session_id}.jsonl"))
}

pub(crate) fn append_subagent_transcript_hint(
    summary: &mut String,
    agent_trail_dir: &str,
    child_session_id: &str,
) {
    let Some(path) = format_subagent_transcript_path(agent_trail_dir, child_session_id) else {
        return;
    };
    if summary.contains(&path) {
        return;
    }
    let transcript_exists = subagent_transcript_path(agent_trail_dir, child_session_id)
        .is_some_and(|absolute| absolute.is_file());
    let note = if transcript_exists {
        format!("[debug transcript] {path}")
    } else {
        "[no transcript] 子 Agent 未产生任何消息（首个 LLM 请求即失败）".to_string()
    };
    if summary.is_empty() {
        *summary = note;
    } else {
        summary.push(' ');
        summary.push_str(&note);
    }
}

/// Persist seed messages before a subagent starts its first LLM request.
///
/// AgentLoop persists messages it creates while running, but its caller owns the initial
/// system/user brief. Recording them here makes the injected workspace root, diff, and review
/// scope auditable without changing their roles or adding them twice to the live conversation.
pub(crate) fn persist_initial_messages(
    sink: Option<&Arc<dyn MessageAppendSink>>,
    messages: &[ChatMessage],
) {
    let Some(sink) = sink else {
        return;
    };
    for message in messages {
        let Ok(value) = serde_json::to_value(message) else {
            warn!("subagent seed message serialization failed; continuing without persistence");
            continue;
        };
        if let Err(error) = sink.append_message(value) {
            warn!(error = %error, "subagent seed message persistence failed");
        }
    }
}

pub(crate) struct JsonlFileAppendSink {
    path: PathBuf,
    child_session_id: String,
    parent_session_id: String,
    subagent_type: SubagentType,
    model: String,
    write_lock: Mutex<()>,
}

impl JsonlFileAppendSink {
    pub(crate) fn new(
        path: PathBuf,
        child_session_id: &str,
        subagent_type: SubagentType,
        model: &str,
        parent_session_id: &str,
    ) -> Self {
        Self {
            path,
            child_session_id: child_session_id.to_string(),
            parent_session_id: parent_session_id.to_string(),
            subagent_type,
            model: model.to_string(),
            write_lock: Mutex::new(()),
        }
    }

    fn append_message_internal(
        &self,
        mut message: serde_json::Value,
        forced_id: Option<&str>,
    ) -> Result<String, AppError> {
        let row_id = forced_id
            .map(ToOwned::to_owned)
            .unwrap_or_else(generate_entry_id);
        let _guard = self.write_lock.lock();

        if let Err(error) = self.ensure_initialized() {
            warn!(
                path = %self.path.display(),
                error = %error,
                child_session_id = %self.child_session_id,
                "subagent transcript init failed; continuing without persistence"
            );
            return Ok(row_id);
        }

        let timestamp = iso_ts_now();
        if let Some(object) = message.as_object_mut() {
            if let Some(value) = object.remove("tool_display") {
                let tool_call_id = object
                    .get("tool_call_id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        AppError::Config("tool_display message is missing tool_call_id".to_string())
                    })?;
                let display = serde_json::from_value::<ToolDisplay>(value).map_err(|error| {
                    AppError::Config(format!(
                        "invalid tool_display on subagent transcript message: {error}"
                    ))
                })?;
                append_tool_display(&self.path, tool_call_id, &timestamp, &display)?;
            }
        }

        let entry = TranscriptEntry::Message(MessageEntry {
            id: Some(row_id.clone()),
            parent_id: None,
            timestamp,
            message,
        });
        if let Err(error) = append_entry_with_sync(&self.path, &entry, SyncLevel::Flush) {
            warn!(
                path = %self.path.display(),
                error = %error,
                child_session_id = %self.child_session_id,
                "subagent transcript append failed; continuing without persistence"
            );
        }
        Ok(row_id)
    }

    fn append_custom_entry_internal(&self, extra: serde_json::Value) -> Result<(), AppError> {
        let _guard = self.write_lock.lock();

        self.ensure_initialized()?;

        let entry = TranscriptEntry::Custom(CustomEntry {
            id: Some(generate_entry_id()),
            parent_id: None,
            timestamp: iso_ts_now(),
            extra,
        });
        append_entry_with_sync(&self.path, &entry, SyncLevel::Flush)
    }

    fn ensure_initialized(&self) -> Result<(), AppError> {
        let needs_init = match std::fs::metadata(&self.path) {
            Ok(meta) => meta.len() == 0,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
            Err(err) => return Err(AppError::Io(err)),
        };
        if !needs_init {
            return Ok(());
        }

        let header = SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: self.child_session_id.clone(),
            timestamp: iso_ts_now(),
            cwd: None,
            project_root: None,
        };
        write_header(&self.path, &header)?;

        let meta = TranscriptEntry::Custom(CustomEntry {
            id: Some(generate_entry_id()),
            parent_id: None,
            timestamp: iso_ts_now(),
            extra: json!({
                "event": "subagent.transcript.meta",
                "child_session_id": self.child_session_id.clone(),
                "parent_session_id": self.parent_session_id.clone(),
                "subagent_type": self.subagent_type.as_str(),
                "model": self.model.clone(),
                "dispatched_at": iso_ts_now(),
            }),
        });
        append_entry_with_sync(&self.path, &meta, SyncLevel::Flush)
    }
}

impl MessageAppendSink for JsonlFileAppendSink {
    fn append_message(&self, value: serde_json::Value) -> Result<String, AppError> {
        self.append_message_internal(value, None)
    }

    fn append_custom_entry(&self, extra: serde_json::Value) -> Result<(), AppError> {
        self.append_custom_entry_internal(extra)
    }

    fn append_message_with_id(
        &self,
        value: serde_json::Value,
        forced_id: &str,
    ) -> Result<String, AppError> {
        self.append_message_internal(value, Some(forced_id))
    }
}

fn iso_ts_now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
