use super::helpers::{agent_send_message_wire, transcript_entry_id};
use super::types::HostApiDispatcher;
use crate::ext::host_binding::HostResponse;
use crate::infra::error::AppError;
use std::sync::atomic::Ordering;

impl HostApiDispatcher {
    pub(super) async fn do_get_current_session(
        &self,
        instance_id: &str,
        _params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let Some(session) = self.session_for_instance(instance_id) else {
            return Ok(HostResponse::err("SessionManager not configured"));
        };
        let Some(session_id) = self.session_id_for_instance(instance_id) else {
            return Ok(HostResponse::ok(serde_json::Value::Null));
        };
        let entry = session.get_session_by_id(&session_id)?;
        let data = match entry {
            Some(e) => serde_json::to_value(e).map_err(AppError::Serialize)?,
            None => serde_json::Value::Null,
        };
        Ok(HostResponse::ok(data))
    }

    pub(super) async fn do_get_messages(
        &self,
        instance_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let Some(session) = self.session_for_instance(instance_id) else {
            return Ok(HostResponse::err("SessionManager not configured"));
        };
        let session_id = self
            .session_id_for_instance(instance_id)
            .ok_or_else(|| AppError::Config(format!("实例未绑定 session: {instance_id}")))?;
        let cap = params.get("cap").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let entries = session.get_entries_for_session(&session_id, cap)?;
        let list: Vec<serde_json::Value> = entries
            .into_iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect();
        Ok(HostResponse::ok(serde_json::json!(list)))
    }

    pub(super) fn do_agent_send_message(
        &self,
        instance_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let Some(session) = self.session_for_instance(instance_id) else {
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
        let session_id = self
            .session_id_for_instance(instance_id)
            .ok_or_else(|| AppError::Config(format!("实例未绑定 session: {instance_id}")))?;
        session.try_append_message_to_session(&session_id, wire)?;
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    pub(super) fn do_agent_send_user_message(
        &self,
        instance_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let Some(session) = self.session_for_instance(instance_id) else {
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
        let session_id = self
            .session_id_for_instance(instance_id)
            .ok_or_else(|| AppError::Config(format!("实例未绑定 session: {instance_id}")))?;
        session.try_append_message_to_session(
            &session_id,
            serde_json::json!({ "role": role, "content": content }),
        )?;
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    pub(super) fn do_context_is_idle() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "idle": true }))
    }

    pub(super) fn do_context_abort() -> HostResponse {
        tracing::debug!("[context] abort requested by plugin");
        HostResponse::ok(serde_json::Value::Null)
    }

    pub(super) fn do_context_get_cwd(&self, instance_id: &str) -> HostResponse {
        let cwd = if let (Some(session), Some(session_id)) = (
            self.session_for_instance(instance_id),
            self.session_id_for_instance(instance_id),
        ) {
            session
                .get_session_by_id(&session_id)
                .ok()
                .flatten()
                .and_then(|entry| entry.cwd)
        } else {
            None
        }
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|path| path.display().to_string())
        })
        .unwrap_or_default();
        HostResponse::ok(serde_json::json!({ "cwd": cwd }))
    }

    pub(super) fn do_context_get_model(&self, instance_id: &str) -> HostResponse {
        let model = if let (Some(session), Some(session_id)) = (
            self.session_for_instance(instance_id),
            self.session_id_for_instance(instance_id),
        ) {
            session
                .get_session_by_id(&session_id)
                .ok()
                .flatten()
                .and_then(|entry| entry.model_override)
        } else {
            None
        }
        .map(serde_json::Value::String)
        .unwrap_or(serde_json::Value::Null);
        HostResponse::ok(serde_json::json!({ "model": model }))
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
        let call_id = params.get("callId").and_then(|v| v.as_str());
        let result = params
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        tracing::debug!("[context.commandCompleted] name={}", name);
        self.command_completed_count.fetch_add(1, Ordering::SeqCst);
        if let Some(call_id) = call_id {
            if let Some((_, waiter)) = self.command_waiters.remove(call_id) {
                let _ = waiter.send(Ok(result));
            }
        }
        HostResponse::ok(serde_json::Value::Null)
    }

    pub(super) fn do_command_failed(&self, params: &serde_json::Value) -> HostResponse {
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let call_id = params.get("callId").and_then(|v| v.as_str());
        let error = params.get("error").and_then(|v| v.as_str()).unwrap_or("");
        tracing::warn!("[context.commandFailed] name={} error={}", name, error);
        self.command_failed_count.fetch_add(1, Ordering::SeqCst);
        if let Some(call_id) = call_id {
            if let Some((_, waiter)) = self.command_waiters.remove(call_id) {
                let _ = waiter.send(Err(error.to_string()));
            }
        }
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
        instance_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let Some(session) = self.session_for_instance(instance_id) else {
            return Ok(HostResponse::err("SessionManager not configured"));
        };
        let session_id = self
            .session_id_for_instance(instance_id)
            .ok_or_else(|| AppError::Config(format!("实例未绑定 session: {instance_id}")))?;
        let from_id = params.get("fromId").and_then(|v| v.as_str());
        let leaf_id = match from_id {
            Some(id) => id.to_string(),
            None => match session.get_leaf_entry_for_session(&session_id)? {
                Some(e) => transcript_entry_id(&e).unwrap_or_default().to_string(),
                None => return Ok(HostResponse::ok(serde_json::json!([]))),
            },
        };
        let branch = session.get_branch_for_session(&session_id, &leaf_id)?;
        let list: Vec<serde_json::Value> = branch
            .into_iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect();
        Ok(HostResponse::ok(serde_json::json!(list)))
    }

    pub(super) fn do_session_get_leaf_entry(
        &self,
        instance_id: &str,
    ) -> Result<HostResponse, AppError> {
        let Some(session) = self.session_for_instance(instance_id) else {
            return Ok(HostResponse::err("SessionManager not configured"));
        };
        let session_id = self
            .session_id_for_instance(instance_id)
            .ok_or_else(|| AppError::Config(format!("实例未绑定 session: {instance_id}")))?;
        let entry = session.get_leaf_entry_for_session(&session_id)?;
        let data = match entry {
            Some(e) => serde_json::to_value(e).map_err(AppError::Serialize)?,
            None => serde_json::Value::Null,
        };
        Ok(HostResponse::ok(data))
    }

    pub(super) fn do_session_get_leaf_id(
        &self,
        instance_id: &str,
    ) -> Result<HostResponse, AppError> {
        let Some(session) = self.session_for_instance(instance_id) else {
            return Ok(HostResponse::err("SessionManager not configured"));
        };
        let session_id = self
            .session_id_for_instance(instance_id)
            .ok_or_else(|| AppError::Config(format!("实例未绑定 session: {instance_id}")))?;
        let id = session
            .get_leaf_entry_for_session(&session_id)?
            .as_ref()
            .and_then(transcript_entry_id)
            .unwrap_or("")
            .to_string();
        Ok(HostResponse::ok(serde_json::json!({ "id": id })))
    }

    pub(super) fn do_session_get_entry(
        &self,
        instance_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let Some(session) = self.session_for_instance(instance_id) else {
            return Ok(HostResponse::err("SessionManager not configured"));
        };
        let session_id = self
            .session_id_for_instance(instance_id)
            .ok_or_else(|| AppError::Config(format!("实例未绑定 session: {instance_id}")))?;
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("getEntry: missing id".to_string()))?;
        let entry = session.get_entry_for_session(&session_id, id)?;
        let data = match entry {
            Some(e) => serde_json::to_value(e).map_err(AppError::Serialize)?,
            None => serde_json::Value::Null,
        };
        Ok(HostResponse::ok(data))
    }

    pub(super) fn do_session_get_header(
        &self,
        instance_id: &str,
    ) -> Result<HostResponse, AppError> {
        let Some(session) = self.session_for_instance(instance_id) else {
            return Ok(HostResponse::err("SessionManager not configured"));
        };
        let session_id = self
            .session_id_for_instance(instance_id)
            .ok_or_else(|| AppError::Config(format!("实例未绑定 session: {instance_id}")))?;
        let header = session.read_session_header_for_session(&session_id)?;
        let data = match header {
            Some(h) => serde_json::to_value(h).map_err(AppError::Serialize)?,
            None => serde_json::Value::Null,
        };
        Ok(HostResponse::ok(data))
    }

    pub(super) fn do_session_get_entries(
        &self,
        instance_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let Some(session) = self.session_for_instance(instance_id) else {
            return Ok(HostResponse::err("SessionManager not configured"));
        };
        let session_id = self
            .session_id_for_instance(instance_id)
            .ok_or_else(|| AppError::Config(format!("实例未绑定 session: {instance_id}")))?;
        let cap = params.get("cap").and_then(|v| v.as_u64()).unwrap_or(2000) as usize;
        let entries = session.get_entries_for_session(&session_id, cap)?;
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
        instance_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let Some(session) = self.session_for_instance(instance_id) else {
            return Ok(HostResponse::err("SessionManager not configured"));
        };
        let session_id = self
            .session_id_for_instance(instance_id)
            .ok_or_else(|| AppError::Config(format!("实例未绑定 session: {instance_id}")))?;
        let message = params
            .get("message")
            .cloned()
            .ok_or_else(|| AppError::Plugin("sendMessage: missing message".to_string()))?;
        session.try_append_message_to_session(&session_id, message)?;
        Ok(HostResponse::ok(serde_json::Value::Null))
    }
}
