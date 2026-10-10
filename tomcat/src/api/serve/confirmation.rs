//! 专用用户确认的 serve 控制桥。
//!
//! 它与 `ask_question` 共用 control frame，但没有把安全确认伪装成 LLM 工具调用。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use tokio::sync::oneshot;

use crate::core::tools::contract::confirmation::{ConfirmDecision, UserConfirmationProvider};
use crate::core::tools::primitive::PrimitiveOperation;
use crate::AppError;

use super::types::{ControlFrame, OutFrame};
use super::writer::WriterHandle;

static NEXT_CONFIRMATION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfirmationRequest {
    request_id: String,
    operation: String,
    preview: String,
    plugin_id: String,
    suggested_root: Option<String>,
}

struct PendingConfirmation {
    session_id: String,
    response: oneshot::Sender<ConfirmDecision>,
}

#[derive(Clone)]
pub struct ServeConfirmationBridge {
    writer: WriterHandle,
    pending: Arc<DashMap<String, PendingConfirmation>>,
}

struct ConfirmationWaitGuard {
    bridge: ServeConfirmationBridge,
    request_id: String,
}

impl Drop for ConfirmationWaitGuard {
    fn drop(&mut self) {
        if let Some((_, pending)) = self.bridge.pending.remove(&self.request_id) {
            self.bridge
                .send_cancel(&self.request_id, &pending.session_id, "cancelled");
        }
    }
}

impl ServeConfirmationBridge {
    pub fn new(writer: WriterHandle) -> Self {
        Self {
            writer,
            pending: Arc::new(DashMap::new()),
        }
    }

    pub fn has_pending_session(&self, session_id: &str) -> bool {
        self.pending
            .iter()
            .any(|pending| pending.session_id == session_id)
    }

    pub fn provider_for_session(
        &self,
        session_id: impl Into<String>,
    ) -> Arc<dyn UserConfirmationProvider> {
        Arc::new(ServeConfirmationProvider {
            bridge: self.clone(),
            session_id: session_id.into(),
        })
    }

    async fn request(
        &self,
        session_id: &str,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
        suggested_root: Option<PathBuf>,
    ) -> Result<ConfirmDecision, AppError> {
        let request_id = format!(
            "confirm-{session_id}-{}",
            NEXT_CONFIRMATION_ID.fetch_add(1, Ordering::Relaxed)
        );
        let (sender, receiver) = oneshot::channel();
        self.pending.insert(
            request_id.clone(),
            PendingConfirmation {
                session_id: session_id.to_string(),
                response: sender,
            },
        );
        let _wait_guard = ConfirmationWaitGuard {
            bridge: self.clone(),
            request_id: request_id.clone(),
        };
        let payload = serde_json::to_value(ConfirmationRequest {
            request_id: request_id.clone(),
            operation: format!("{operation:?}"),
            preview: preview.to_string(),
            plugin_id: plugin_id.to_string(),
            suggested_root: suggested_root
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        })
        .map_err(|error| {
            AppError::Config(crate::infra::i18n::tr(
                "serve.serializeFailed",
                &[("name", "confirmation"), ("detail", &error.to_string())],
            ))
        })?;
        self.writer.send(OutFrame::Control(ControlFrame::request(
            request_id.clone(),
            "confirmation",
            Some(session_id.to_string()),
            payload,
        )))?;
        receiver.await.map_err(|_| {
            AppError::Permission(crate::infra::i18n::tr(
                "serve.confirmationDisconnected",
                &[],
            ))
        })
    }

    pub fn handle_control_response(&self, frame: &ControlFrame) -> Result<bool, AppError> {
        let ControlFrame::ControlResponse {
            request_id,
            session_id,
            payload,
        } = frame
        else {
            return Ok(false);
        };
        let Some(entry) = self.pending.get(request_id) else {
            return Ok(false);
        };
        if session_id.as_deref() != Some(entry.session_id.as_str()) {
            tracing::warn!(request_id, response_session_id = ?session_id, pending_session_id = %entry.session_id, "dropping confirmation response routed to the wrong session");
            return Ok(false);
        }
        let decision = match payload.get("decision").and_then(serde_json::Value::as_str) {
            Some("allow_once") => ConfirmDecision::AllowOnce,
            Some("allow_and_persist_root") => {
                let Some(root) = payload.get("root").and_then(serde_json::Value::as_str) else {
                    return Ok(false);
                };
                ConfirmDecision::AllowAndPersistRoot {
                    root: PathBuf::from(root),
                }
            }
            Some("deny") => ConfirmDecision::Deny,
            _ => return Ok(false),
        };
        drop(entry);
        if let Some((_, pending)) = self.pending.remove(request_id) {
            let _ = pending.response.send(decision);
        }
        Ok(true)
    }

    pub fn handle_control_cancel(&self, frame: &ControlFrame) -> Result<bool, AppError> {
        let ControlFrame::ControlCancel {
            request_id,
            session_id,
            ..
        } = frame
        else {
            return Ok(false);
        };
        let Some(entry) = self.pending.get(request_id) else {
            return Ok(false);
        };
        if session_id.as_deref() != Some(entry.session_id.as_str()) {
            return Ok(false);
        }
        drop(entry);
        if let Some((_, pending)) = self.pending.remove(request_id) {
            let _ = pending.response.send(ConfirmDecision::Deny);
        }
        Ok(true)
    }

    fn send_cancel(&self, request_id: &str, session_id: &str, reason: &str) {
        let _ = self.writer.send(OutFrame::Control(ControlFrame::cancel(
            request_id.to_string(),
            Some(session_id.to_string()),
            serde_json::json!({ "reason": reason }),
        )));
    }

    pub fn cancel_live_session(&self, session_id: &str, reason: &str) -> usize {
        let request_ids = self
            .pending
            .iter()
            .filter(|entry| entry.value().session_id == session_id)
            .map(|entry| entry.key().clone())
            .collect::<Vec<_>>();
        for request_id in &request_ids {
            if let Some((_, pending)) = self.pending.remove(request_id) {
                self.send_cancel(request_id, session_id, reason);
                let _ = pending.response.send(ConfirmDecision::Deny);
            }
        }
        request_ids.len()
    }
}

struct ServeConfirmationProvider {
    bridge: ServeConfirmationBridge,
    session_id: String,
}

#[async_trait]
impl UserConfirmationProvider for ServeConfirmationProvider {
    async fn confirm(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
    ) -> Result<bool, AppError> {
        Ok(!matches!(
            self.bridge
                .request(&self.session_id, operation, preview, plugin_id, None)
                .await?,
            ConfirmDecision::Deny
        ))
    }

    async fn confirm_decision(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
        _target: Option<PathBuf>,
        suggested_root: Option<PathBuf>,
    ) -> Result<ConfirmDecision, AppError> {
        let decision = self
            .bridge
            .request(
                &self.session_id,
                operation,
                preview,
                plugin_id,
                suggested_root.clone(),
            )
            .await?;
        match decision {
            ConfirmDecision::AllowAndPersistRoot { ref root }
                if suggested_root.as_deref() == Some(root.as_path()) =>
            {
                Ok(decision)
            }
            ConfirmDecision::AllowAndPersistRoot { .. } => Ok(ConfirmDecision::Deny),
            _ => Ok(decision),
        }
    }
}
