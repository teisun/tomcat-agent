//! Standard ACP permission requests, backed by the existing confirmation gate.
//! This bridge never decides authorization, changes auto_confirm, or accepts a
//! root supplied by the client's answer. Native ownership validates every reply.

use crate::{
    api::serve::{
        stdin::QueuedCommand,
        types::{ControlFrame, ServeCommand},
    },
    AppError,
};
use agent_client_protocol::{schema::v1 as acp, Client, ConnectionTo};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub(super) struct PermissionBridge {
    client: Mutex<Option<ConnectionTo<Client>>>,
    pending: Mutex<HashMap<String, CancellationToken>>,
    input: mpsc::UnboundedSender<QueuedCommand>,
}

impl PermissionBridge {
    pub fn new(input: mpsc::UnboundedSender<QueuedCommand>) -> Self {
        Self {
            client: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            input,
        }
    }

    pub fn attach(&self, client: ConnectionTo<Client>) {
        *self.client.lock() = Some(client);
    }

    pub fn close(&self) {
        self.client.lock().take();
        for (_, cancellation) in self.pending.lock().drain() {
            cancellation.cancel();
        }
    }

    pub fn forward(self: &Arc<Self>, frame: ControlFrame) -> Result<Option<String>, AppError> {
        match frame {
            ControlFrame::ControlRequest {
                request_id,
                session_id: Some(session_id),
                subtype,
                payload,
            } if subtype == "confirmation" => {
                let client = self
                    .client
                    .lock()
                    .clone()
                    .ok_or_else(|| AppError::Config("ACP client is not initialized".into()))?;
                let cancellation = CancellationToken::new();
                self.pending
                    .lock()
                    .insert(request_id.clone(), cancellation.clone());
                let root = payload["suggestedRoot"].as_str().map(str::to_owned);
                let request =
                    permission_request(&request_id, &session_id, &payload, root.as_deref());
                let bridge = self.clone();
                let peer = client.clone();
                client.spawn(async move {
                    if cancellation.is_cancelled() { return Ok(()); }
                    let reply = tokio::select! {
                        _ = cancellation.cancelled() => { return Ok(()); }
                        response = peer.send_request(request).block_task() => response,
                    };
                    // Taking the pending entry is the local cancellation boundary.
                    // A late SDK answer cannot revive a native request already cancelled.
                    if bridge.pending.lock().remove(&request_id).is_none() { return Ok(()); }
                    let decision = match reply {
                        Ok(response) => permission_decision(response.outcome, root.as_deref()),
                        Err(error) => {
                            tracing::warn!(%error, %request_id, "ACP permission request failed; denying");
                            json!({"decision":"deny"})
                        }
                    };
                    let _ = bridge.input.send(ServeCommand::ControlResponse {
                        request_id, session_id: Some(session_id), payload: decision,
                    }.into());
                    Ok(())
                }).map_err(|e| AppError::Config(e.to_string()))?;
                Ok(None)
            }
            ControlFrame::ControlCancel { request_id, .. } => {
                if let Some(cancellation) = self.pending.lock().remove(&request_id) {
                    cancellation.cancel();
                }
                Ok(None)
            }
            ControlFrame::ControlRequest {
                request_id,
                session_id: Some(session_id),
                subtype,
                ..
            } => {
                // No invented answer or assistant text for unsupported interactions.
                tracing::warn!(%request_id, %subtype, "unsupported ACP interaction cancelled");
                self.input.send(ServeCommand::ControlCancel {
                    request_id, session_id: Some(session_id),
                    payload: json!({"reason":"unsupported_acp_interaction", "outcome":"interrupted"}),
                }.into()).map_err(|_| AppError::Config("connection_closed".into()))?;
                Ok(None)
            }
            ControlFrame::ControlResponse { .. } => Ok(None),
            _ => Err(AppError::Config(
                "ACP control request has no session".into(),
            )),
        }
    }
}

fn permission_request(
    request_id: &str,
    session_id: &str,
    payload: &Value,
    root: Option<&str>,
) -> acp::RequestPermissionRequest {
    let mut options = vec![acp::PermissionOption::new(
        "allow_once",
        "Allow once",
        acp::PermissionOptionKind::AllowOnce,
    )];
    if let Some(root) = root {
        options.push(acp::PermissionOption::new(
            "allow_and_persist_root",
            format!("Allow and save root: {root}"),
            acp::PermissionOptionKind::AllowAlways,
        ));
    }
    options.push(acp::PermissionOption::new(
        "deny",
        "Deny",
        acp::PermissionOptionKind::RejectOnce,
    ));
    let details = acp::ToolCallUpdateFields::new()
        .title(
            payload["operation"]
                .as_str()
                .unwrap_or("Permission required"),
        )
        .status(acp::ToolCallStatus::Pending)
        .raw_input(payload.clone())
        .content(vec![acp::ToolCallContent::from(
            payload["preview"].as_str().unwrap_or("").to_owned(),
        )]);
    acp::RequestPermissionRequest::new(
        session_id.to_owned(),
        acp::ToolCallUpdate::new(request_id.to_owned(), details),
        options,
    )
}

fn permission_decision(outcome: acp::RequestPermissionOutcome, root: Option<&str>) -> Value {
    match outcome {
        acp::RequestPermissionOutcome::Selected(selected) => {
            match selected.option_id.to_string().as_str() {
                "allow_once" => json!({"decision":"allow_once"}),
                "allow_and_persist_root" if root.is_some() => {
                    json!({"decision":"allow_and_persist_root", "root":root})
                }
                _ => json!({"decision":"deny"}),
            }
        }
        _ => json!({"decision":"deny"}),
    }
}

#[cfg(test)]
#[path = "tests/permissions_test.rs"]
mod tests;
