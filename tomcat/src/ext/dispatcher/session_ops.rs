use super::helpers::{agent_send_message_wire, transcript_entry_id};
use super::types::HostApiDispatcher;
use crate::ext::host_binding::HostResponse;
use crate::infra::error::AppError;
use std::sync::atomic::Ordering;

impl HostApiDispatcher {
    pub(super) async fn do_get_current_session(
        &self,
        _params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let session = match &self.session {
            None => return Ok(HostResponse::err("SessionManager not configured")),
            Some(s) => s,
        };
        let key = session.current_session_key();
        let entry = session.get_session(key)?;
        let data = match entry {
            Some(e) => serde_json::to_value(e).map_err(AppError::Serialize)?,
            None => serde_json::Value::Null,
        };
        Ok(HostResponse::ok(data))
    }

    pub(super) async fn do_get_messages(
        &self,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let session = match &self.session {
            None => return Ok(HostResponse::err("SessionManager not configured")),
            Some(s) => s,
        };
        let cap = params.get("cap").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let entries = session.get_entries(cap)?;
        let list: Vec<serde_json::Value> = entries
            .into_iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect();
        Ok(HostResponse::ok(serde_json::json!(list)))
    }

    pub(super) fn do_agent_send_message(
        &self,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let Some(session) = &self.session else {
            tracing::debug!(
                "[plugin sendMessage] no SessionManager, message={:?}",
                params.get("message")
            );
            return Ok(HostResponse::ok(serde_json::Value::Null));
        };
        if params
            .get("options")
            .and_then(|o| o.get("silent"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            tracing::debug!("[plugin sendMessage] silent=true, skip transcript append");
            return Ok(HostResponse::ok(serde_json::Value::Null));
        }
        let wire = agent_send_message_wire(params)?;
        session.try_append_message(wire)?;
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    pub(super) fn do_agent_send_user_message(
        &self,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let Some(session) = &self.session else {
            tracing::debug!(
                "[plugin sendUserMessage] no SessionManager, content={:?}",
                params.get("content")
            );
            return Ok(HostResponse::ok(serde_json::Value::Null));
        };
        if params
            .get("options")
            .and_then(|o| o.get("silent"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Ok(HostResponse::ok(serde_json::Value::Null));
        }
        let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let role = params
            .get("options")
            .and_then(|o| o.get("role"))
            .and_then(|v| v.as_str())
            .unwrap_or("user");
        session.try_append_message(serde_json::json!({ "role": role, "content": content }))?;
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    pub(super) fn do_context_is_idle() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "idle": true }))
    }

    pub(super) fn do_context_abort() -> HostResponse {
        tracing::debug!("[context] abort requested by plugin");
        HostResponse::ok(serde_json::Value::Null)
    }

    pub(super) fn do_context_get_cwd() -> HostResponse {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        HostResponse::ok(serde_json::json!({ "cwd": cwd }))
    }

    pub(super) fn do_context_get_model() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "model": serde_json::Value::Null }))
    }

    pub(super) fn do_context_ui_notify(&self, params: &serde_json::Value) -> HostResponse {
        if let Some(c) = &self.ui_notify_count {
            c.fetch_add(1, Ordering::SeqCst);
        }
        let msg = params.get("message").and_then(|v| v.as_str()).unwrap_or("");
        let kind = params
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("info");
        tracing::debug!("[context.ui.notify] [{}] {}", kind, msg);
        HostResponse::ok(serde_json::Value::Null)
    }

    pub(super) fn do_context_ui_select(params: &serde_json::Value) -> HostResponse {
        let options = params
            .get("options")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let title = params.get("title").and_then(|v| v.as_str()).unwrap_or("");
        tracing::debug!(
            "[context.ui.select] title={} option_count={}",
            title,
            options.len()
        );
        let (selected_index, selected, cancelled) = if let Some(first) = options.first() {
            (0_i64, first.clone(), false)
        } else {
            (-1_i64, serde_json::Value::Null, true)
        };
        HostResponse::ok(serde_json::json!({
            "selectedIndex": selected_index,
            "selected": selected,
            "cancelled": cancelled
        }))
    }

    pub(super) fn do_context_ui_confirm(params: &serde_json::Value) -> HostResponse {
        let title = params.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let message = params.get("message").and_then(|v| v.as_str()).unwrap_or("");
        tracing::debug!(
            "[context.ui.confirm] title={} message_len={}",
            title,
            message.len()
        );
        HostResponse::ok(serde_json::json!({ "confirmed": true }))
    }

    pub(super) fn do_context_ui_input(params: &serde_json::Value) -> HostResponse {
        let placeholder = params
            .get("placeholder")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        tracing::debug!("[context.ui.input] placeholder_len={}", placeholder.len());
        HostResponse::ok(serde_json::json!({ "value": "" }))
    }

    pub(super) fn do_context_ui_set_status(params: &serde_json::Value) -> HostResponse {
        let message = params.get("message").and_then(|v| v.as_str()).unwrap_or("");
        let details = params
            .get("details")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        tracing::debug!("[context.ui.setStatus] {} details={}", message, details);
        HostResponse::ok(serde_json::Value::Null)
    }

    pub(super) fn do_command_completed(&self, params: &serde_json::Value) -> HostResponse {
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        tracing::debug!("[context.commandCompleted] name={}", name);
        self.command_completed_count.fetch_add(1, Ordering::SeqCst);
        HostResponse::ok(serde_json::Value::Null)
    }

    pub(super) fn do_command_failed(&self, params: &serde_json::Value) -> HostResponse {
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let error = params.get("error").and_then(|v| v.as_str()).unwrap_or("");
        tracing::warn!("[context.commandFailed] name={} error={}", name, error);
        self.command_failed_count.fetch_add(1, Ordering::SeqCst);
        HostResponse::ok(serde_json::Value::Null)
    }

    pub(super) fn do_context_ui_custom(params: &serde_json::Value) -> HostResponse {
        let lines = params
            .get("lines")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|v| v.as_str().unwrap_or("").to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !lines.is_empty() {
            tracing::info!("[context.ui.custom] rendered {} lines", lines.len());
            for line in &lines {
                tracing::debug!("  | {}", line);
            }
        }
        HostResponse::ok(serde_json::Value::Null)
    }

    pub(super) fn do_context_ui_stub(op: &str, params: &serde_json::Value) -> HostResponse {
        tracing::debug!("[context.ui.{}] stub, params={}", op, params);
        HostResponse::ok(serde_json::Value::Null)
    }

    pub(super) fn do_context_ui_editor(params: &serde_json::Value) -> HostResponse {
        let prefill = params.get("prefill").and_then(|v| v.as_str()).unwrap_or("");
        tracing::debug!(
            "[context.ui.editor] title={:?} prefill_len={}",
            params.get("title").and_then(|v| v.as_str()).unwrap_or(""),
            prefill.len()
        );
        HostResponse::ok(serde_json::json!({ "text": prefill }))
    }

    pub(super) fn do_context_get_system_prompt() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "prompt": "" }))
    }

    pub(super) fn do_context_has_pending() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "pending": false }))
    }

    pub(super) fn do_context_shutdown() -> HostResponse {
        tracing::warn!("[context] shutdown requested by plugin");
        HostResponse::ok(serde_json::Value::Null)
    }

    pub(super) fn do_context_usage() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "tokens": null, "contextWindow": 0, "percent": null }))
    }

    pub(super) fn do_context_compact() -> HostResponse {
        tracing::debug!("[context] compact requested by plugin");
        HostResponse::ok(serde_json::Value::Null)
    }

    pub(super) fn do_session_get_branch(
        &self,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let session = match &self.session {
            None => return Ok(HostResponse::err("SessionManager not configured")),
            Some(s) => s,
        };
        let from_id = params.get("fromId").and_then(|v| v.as_str());
        let leaf_id = match from_id {
            Some(id) => id.to_string(),
            None => match session.get_leaf_entry()? {
                Some(e) => transcript_entry_id(&e).unwrap_or_default().to_string(),
                None => return Ok(HostResponse::ok(serde_json::json!([]))),
            },
        };
        let branch = session.get_branch(&leaf_id)?;
        let list: Vec<serde_json::Value> = branch
            .into_iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect();
        Ok(HostResponse::ok(serde_json::json!(list)))
    }

    pub(super) fn do_session_get_leaf_entry(&self) -> Result<HostResponse, AppError> {
        let session = match &self.session {
            None => return Ok(HostResponse::err("SessionManager not configured")),
            Some(s) => s,
        };
        let entry = session.get_leaf_entry()?;
        let data = match entry {
            Some(e) => serde_json::to_value(e).map_err(AppError::Serialize)?,
            None => serde_json::Value::Null,
        };
        Ok(HostResponse::ok(data))
    }

    pub(super) fn do_session_get_leaf_id(&self) -> Result<HostResponse, AppError> {
        let session = match &self.session {
            None => return Ok(HostResponse::err("SessionManager not configured")),
            Some(s) => s,
        };
        let id = session
            .get_leaf_entry()?
            .as_ref()
            .and_then(transcript_entry_id)
            .unwrap_or("")
            .to_string();
        Ok(HostResponse::ok(serde_json::json!({ "id": id })))
    }

    pub(super) fn do_session_get_entry(
        &self,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let session = match &self.session {
            None => return Ok(HostResponse::err("SessionManager not configured")),
            Some(s) => s,
        };
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("getEntry: missing id".to_string()))?;
        let entry = session.get_entry(id)?;
        let data = match entry {
            Some(e) => serde_json::to_value(e).map_err(AppError::Serialize)?,
            None => serde_json::Value::Null,
        };
        Ok(HostResponse::ok(data))
    }

    pub(super) fn do_session_get_header(&self) -> Result<HostResponse, AppError> {
        let session = match &self.session {
            None => return Ok(HostResponse::err("SessionManager not configured")),
            Some(s) => s,
        };
        let header = session.read_session_header()?;
        let data = match header {
            Some(h) => serde_json::to_value(h).map_err(AppError::Serialize)?,
            None => serde_json::Value::Null,
        };
        Ok(HostResponse::ok(data))
    }

    pub(super) fn do_session_get_entries(
        &self,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let session = match &self.session {
            None => return Ok(HostResponse::err("SessionManager not configured")),
            Some(s) => s,
        };
        let cap = params.get("cap").and_then(|v| v.as_u64()).unwrap_or(2000) as usize;
        let entries = session.get_entries(cap)?;
        let list: Vec<serde_json::Value> = entries
            .into_iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect();
        Ok(HostResponse::ok(serde_json::json!(list)))
    }

    pub(super) fn do_context_list_models() -> HostResponse {
        HostResponse::ok(serde_json::json!([]))
    }

    pub(super) async fn do_send_message(
        &self,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let session = match &self.session {
            None => return Ok(HostResponse::err("SessionManager not configured")),
            Some(s) => s,
        };
        let message = params
            .get("message")
            .cloned()
            .ok_or_else(|| AppError::Plugin("sendMessage: missing message".to_string()))?;
        session.try_append_message(message)?;
        Ok(HostResponse::ok(serde_json::Value::Null))
    }
}
