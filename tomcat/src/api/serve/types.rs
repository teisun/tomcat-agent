use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::infra::events::WireEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServeAttachmentKind {
    Image,
    File,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServeAttachment {
    pub kind: ServeAttachmentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServeMessageParams {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ServeAttachment>,
}

impl ServeMessageParams {
    pub fn is_empty(&self) -> bool {
        self.attachments.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ServeSessionMode {
    Code,
    Claw,
}

impl ServeSessionMode {
    pub fn into_core_mode(self) -> crate::SessionMode {
        match self {
            Self::Code => crate::SessionMode::Code,
            Self::Claw => crate::SessionMode::Claw,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ServeSessionMode>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GetMessagesParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_n_turns: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServeCommand {
    #[serde(rename_all = "camelCase")]
    Prompt {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        text: String,
        #[serde(default, skip_serializing_if = "ServeMessageParams::is_empty")]
        params: ServeMessageParams,
    },
    #[serde(rename_all = "camelCase")]
    Steer {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        text: String,
        #[serde(default, skip_serializing_if = "ServeMessageParams::is_empty")]
        params: ServeMessageParams,
    },
    #[serde(rename_all = "camelCase")]
    FollowUp {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        text: String,
        #[serde(default, skip_serializing_if = "ServeMessageParams::is_empty")]
        params: ServeMessageParams,
    },
    #[serde(rename_all = "camelCase")]
    GetState {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    SetModel {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        model: String,
    },
    #[serde(rename_all = "camelCase")]
    NewSession {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default)]
        params: NewSessionParams,
    },
    #[serde(rename_all = "camelCase")]
    SwitchSession {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    #[serde(rename_all = "camelCase")]
    GetMessages {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(default)]
        params: GetMessagesParams,
    },
    #[serde(rename_all = "camelCase")]
    CloseSession {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ListSessions {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Interrupt {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ControlRequest {
        request_id: String,
        subtype: String,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        payload: Value,
    },
    #[serde(rename_all = "camelCase")]
    ControlResponse {
        request_id: String,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        payload: Value,
    },
    #[serde(rename_all = "camelCase")]
    ControlCancel {
        request_id: String,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        payload: Value,
    },
}

impl ServeCommand {
    pub fn command_id(&self) -> Option<&str> {
        match self {
            Self::Prompt { id, .. }
            | Self::Steer { id, .. }
            | Self::FollowUp { id, .. }
            | Self::GetState { id, .. }
            | Self::SetModel { id, .. }
            | Self::NewSession { id, .. }
            | Self::SwitchSession { id, .. }
            | Self::GetMessages { id, .. }
            | Self::CloseSession { id, .. }
            | Self::ListSessions { id, .. }
            | Self::Interrupt { id, .. } => id.as_deref(),
            Self::ControlRequest { .. }
            | Self::ControlResponse { .. }
            | Self::ControlCancel { .. } => None,
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::Prompt { session_id, .. }
            | Self::Steer { session_id, .. }
            | Self::FollowUp { session_id, .. }
            | Self::GetState { session_id, .. }
            | Self::SetModel { session_id, .. }
            | Self::GetMessages { session_id, .. }
            | Self::CloseSession { session_id, .. }
            | Self::Interrupt { session_id, .. }
            | Self::ControlRequest { session_id, .. }
            | Self::ControlResponse { session_id, .. }
            | Self::ControlCancel { session_id, .. } => session_id.as_deref(),
            Self::SwitchSession { session_id, .. } => Some(session_id.as_str()),
            Self::NewSession { .. } | Self::ListSessions { .. } => None,
        }
    }

    pub fn is_initialize(&self) -> bool {
        matches!(
            self,
            Self::ControlRequest {
                subtype,
                request_id: _,
                session_id: _,
                payload: _,
            } if subtype == "initialize"
        )
    }

    pub fn requires_initialized(&self) -> bool {
        !self.is_initialize()
    }

    pub fn wire_type(&self) -> &'static str {
        match self {
            Self::Prompt { .. } => "prompt",
            Self::Steer { .. } => "steer",
            Self::FollowUp { .. } => "follow_up",
            Self::GetState { .. } => "get_state",
            Self::SetModel { .. } => "set_model",
            Self::NewSession { .. } => "new_session",
            Self::SwitchSession { .. } => "switch_session",
            Self::GetMessages { .. } => "get_messages",
            Self::CloseSession { .. } => "close_session",
            Self::ListSessions { .. } => "list_sessions",
            Self::Interrupt { .. } => "interrupt",
            Self::ControlRequest { .. } => "control_request",
            Self::ControlResponse { .. } => "control_response",
            Self::ControlCancel { .. } => "control_cancel",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResponseFrame {
    #[serde(rename = "type")]
    pub frame_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub success: bool,
    #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

impl ResponseFrame {
    pub fn ok(id: Option<String>, session_id: Option<String>, payload: Option<Value>) -> Self {
        Self {
            frame_type: "response".to_string(),
            id,
            success: true,
            session_id,
            error: None,
            payload,
        }
    }

    pub fn error(id: Option<String>, session_id: Option<String>, error: impl Into<String>) -> Self {
        Self {
            frame_type: "response".to_string(),
            id,
            success: false,
            session_id,
            error: Some(error.into()),
            payload: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlFrame {
    #[serde(rename_all = "camelCase")]
    ControlRequest {
        request_id: String,
        subtype: String,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        payload: Value,
    },
    #[serde(rename_all = "camelCase")]
    ControlResponse {
        request_id: String,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        payload: Value,
    },
    #[serde(rename_all = "camelCase")]
    ControlCancel {
        request_id: String,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        payload: Value,
    },
}

impl ControlFrame {
    pub fn request(
        request_id: impl Into<String>,
        subtype: impl Into<String>,
        session_id: Option<String>,
        payload: Value,
    ) -> Self {
        Self::ControlRequest {
            request_id: request_id.into(),
            subtype: subtype.into(),
            session_id,
            payload,
        }
    }

    pub fn response(
        request_id: impl Into<String>,
        session_id: Option<String>,
        payload: Value,
    ) -> Self {
        Self::ControlResponse {
            request_id: request_id.into(),
            session_id,
            payload,
        }
    }

    pub fn cancel(
        request_id: impl Into<String>,
        session_id: Option<String>,
        payload: Value,
    ) -> Self {
        Self::ControlCancel {
            request_id: request_id.into(),
            session_id,
            payload,
        }
    }

    pub fn request_id(&self) -> &str {
        match self {
            Self::ControlRequest { request_id, .. }
            | Self::ControlResponse { request_id, .. }
            | Self::ControlCancel { request_id, .. } => request_id,
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::ControlRequest { session_id, .. }
            | Self::ControlResponse { session_id, .. }
            | Self::ControlCancel { session_id, .. } => session_id.as_deref(),
        }
    }

    pub fn wire_type(&self) -> &'static str {
        match self {
            Self::ControlRequest { .. } => "control_request",
            Self::ControlResponse { .. } => "control_response",
            Self::ControlCancel { .. } => "control_cancel",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum OutFrame {
    Response(ResponseFrame),
    Control(ControlFrame),
    Event(#[schemars(with = "WireEvent")] Value),
}

impl OutFrame {
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::Response(frame) => frame.session_id.as_deref(),
            Self::Control(frame) => frame.session_id(),
            Self::Event(value) => value.get("sessionId").and_then(Value::as_str),
        }
    }

    pub fn wire_type(&self) -> Option<&str> {
        match self {
            Self::Response(frame) => Some(frame.frame_type.as_str()),
            Self::Control(frame) => Some(frame.wire_type()),
            Self::Event(value) => value.get("type").and_then(Value::as_str),
        }
    }

    #[allow(dead_code)]
    pub fn is_lossless(&self) -> bool {
        // TODO(next): either wire this into writer backpressure classification or delete it.
        !matches!(self.wire_type(), Some("message_update"))
    }

    pub fn is_message_delta(&self) -> bool {
        matches!(self.wire_type(), Some("message_update"))
    }
}
