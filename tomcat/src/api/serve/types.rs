//! `tomcat serve` 的协议类型定义。
//!
//! 这里集中承载：
//! - UI -> agent 的命令帧
//! - 双向控制帧
//! - agent -> UI 的响应/事件信封
//! - schema 导出所需的 `schemars` 派生

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::llm::{ModelEntryInput, ModelKeyStatus, ModelView, ProviderKeyView};
use crate::infra::events::WireEvent as AgentWireEvent;

/// `prompt` / `follow_up` 附件的逻辑类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServeAttachmentKind {
    Image,
    File,
}

/// 多模态消息中的单个附件描述。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServeAttachment {
    pub kind: ServeAttachmentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServeContextRefKind {
    Selection,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServeContextReference {
    pub kind: ServeContextRefKind,
    pub path: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServeContentSegment {
    Text {
        text: String,
    },
    Reference {
        #[serde(flatten)]
        reference: ServeContextReference,
    },
}

/// 发送给 `prompt` / `follow_up` / `steer` 的附加参数。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServeMessageParams {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<ServeContentSegment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ServeAttachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_message_id: Option<String>,
}

impl ServeMessageParams {
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty() && self.attachments.is_empty() && self.user_message_id.is_none()
    }
}

/// 新会话的运行模式。
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

/// `list_sessions` 的可选枚举范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ListSessionsScope {
    Live,
    Disk,
}

/// `set_plan_mode` 的动作枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SetPlanModeAction {
    Enter,
    Exit,
    Build,
}

/// `new_session` 的可选参数。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ServeSessionMode>,
}

/// `get_messages` 的分页/裁剪参数。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GetMessagesParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_n_turns: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListModelsPayload {
    pub models: Vec<ModelView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpsertModelResponse {
    pub model: ModelView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoveModelResponse {
    pub model_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetProviderKeyResponse {
    pub env_name: String,
    pub key_present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListProviderKeysPayload {
    pub keys: Vec<ProviderKeyView>,
}

impl From<ModelKeyStatus> for SetProviderKeyResponse {
    fn from(value: ModelKeyStatus) -> Self {
        Self {
            env_name: value.env_name,
            key_present: value.key_present,
        }
    }
}

/// UI 通过 stdin 发送给 `tomcat serve` 的命令帧。
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
    ListCheckpoints {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    RestoreCheckpoint {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "checkpointId")]
        checkpoint_id: String,
        #[serde(rename = "revertFiles")]
        revert_files: bool,
        #[serde(default, rename = "dryRun", skip_serializing_if = "Option::is_none")]
        dry_run: Option<bool>,
    },
    #[serde(rename_all = "camelCase")]
    SetPlanMode {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        action: SetPlanModeAction,
        #[serde(default, rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
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
    SetThinkingLevel {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        model: String,
        level: String,
    },
    #[serde(rename_all = "camelCase")]
    ListModels {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    UpsertModel {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        model: ModelEntryInput,
    },
    #[serde(rename_all = "camelCase")]
    RemoveModel {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        model_id: String,
    },
    #[serde(rename_all = "camelCase")]
    SetProviderKey {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        env_name: String,
        value: String,
    },
    #[serde(rename_all = "camelCase")]
    ListProviderKeys {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<ListSessionsScope>,
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
            | Self::ListCheckpoints { id, .. }
            | Self::RestoreCheckpoint { id, .. }
            | Self::SetPlanMode { id, .. }
            | Self::SetModel { id, .. }
            | Self::SetThinkingLevel { id, .. }
            | Self::ListModels { id, .. }
            | Self::UpsertModel { id, .. }
            | Self::RemoveModel { id, .. }
            | Self::SetProviderKey { id, .. }
            | Self::ListProviderKeys { id, .. }
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
            | Self::ListCheckpoints { session_id, .. }
            | Self::RestoreCheckpoint { session_id, .. }
            | Self::SetPlanMode { session_id, .. }
            | Self::SetModel { session_id, .. }
            | Self::SetThinkingLevel { session_id, .. }
            | Self::GetMessages { session_id, .. }
            | Self::CloseSession { session_id, .. }
            | Self::Interrupt { session_id, .. }
            | Self::ControlRequest { session_id, .. }
            | Self::ControlResponse { session_id, .. }
            | Self::ControlCancel { session_id, .. } => session_id.as_deref(),
            Self::SwitchSession { session_id, .. } => Some(session_id.as_str()),
            Self::NewSession { .. }
            | Self::ListModels { .. }
            | Self::UpsertModel { .. }
            | Self::RemoveModel { .. }
            | Self::SetProviderKey { .. }
            | Self::ListProviderKeys { .. }
            | Self::ListSessions { .. } => None,
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
            Self::ListCheckpoints { .. } => "list_checkpoints",
            Self::RestoreCheckpoint { .. } => "restore_checkpoint",
            Self::SetPlanMode { .. } => "set_plan_mode",
            Self::SetModel { .. } => "set_model",
            Self::SetThinkingLevel { .. } => "set_thinking_level",
            Self::ListModels { .. } => "list_models",
            Self::UpsertModel { .. } => "upsert_model",
            Self::RemoveModel { .. } => "remove_model",
            Self::SetProviderKey { .. } => "set_provider_key",
            Self::ListProviderKeys { .. } => "list_provider_keys",
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

/// `plan.*` 自定义事件的 schema 入口。
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(tag = "type")]
pub enum ServePlanEvent {
    #[serde(rename = "plan.create")]
    PlanCreate {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        state: Option<String>,
    },
    #[serde(rename = "plan.build")]
    PlanBuild {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        state: Option<String>,
    },
    #[serde(rename = "plan.enter")]
    PlanEnter {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        state: Option<String>,
    },
    #[serde(rename = "plan.exit")]
    PlanExit {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        state: Option<String>,
    },
    #[serde(rename = "plan.update")]
    PlanUpdate {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        state: Option<String>,
    },
    #[serde(rename = "plan.review")]
    PlanReview {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        aborted: Option<bool>,
    },
    #[serde(rename = "plan.code_review")]
    PlanCodeReview {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        aborted: Option<bool>,
    },
    #[serde(rename = "plan.verify")]
    PlanVerify {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        verdict: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        aborted: Option<bool>,
    },
    #[serde(rename = "plan.review.warning")]
    PlanReviewWarning {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rounds: Option<u32>,
    },
    #[serde(rename = "plan.code_review.warning")]
    PlanCodeReviewWarning {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rounds: Option<u32>,
    },
    #[serde(rename = "plan.complete")]
    PlanComplete {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        state: Option<String>,
    },
    #[serde(rename = "plan.pending")]
    PlanPending {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        state: Option<String>,
    },
    #[serde(rename = "plan.todos")]
    PlanTodos {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        todos: Option<Vec<ServeTodoItem>>,
    },
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ServeEvent {
    Agent(AgentWireEvent),
    Plan(ServePlanEvent),
    Session(ServeSessionEvent),
    Turn(ServeTurnEvent),
    Tool(ServeToolEvent),
}

/// `session.*` 自定义事件的 schema 入口（`session.todos` / `session.title_updated`）。
///
/// 仅用于 `tomcat serve --print-schema` / fixture 导出，不影响运行时 event bus 发射路径
/// （运行时经 `write_transcript_custom` / `emit_payload` 以字符串常量发射）。
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(tag = "type")]
pub enum ServeSessionEvent {
    #[serde(rename = "session.todos")]
    SessionTodos {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        todos: Option<Vec<ServeTodoItem>>,
    },
    #[serde(rename = "session.title_updated")]
    SessionTitleUpdated {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
}

/// `turn.*` 自定义事件的 schema 入口（当前仅 `turn.summary_updated`）。
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(tag = "type")]
pub enum ServeTurnEvent {
    #[serde(rename = "turn.summary_updated")]
    TurnSummaryUpdated {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "turnIndex", skip_serializing_if = "Option::is_none")]
        turn_index: Option<usize>,
        #[serde(rename = "assistantMessageId", skip_serializing_if = "Option::is_none")]
        assistant_message_id: Option<String>,
        #[serde(rename = "toolCallIds", skip_serializing_if = "Option::is_none")]
        tool_call_ids: Option<Vec<String>>,
        #[serde(rename = "summaryTitle", skip_serializing_if = "Option::is_none")]
        summary_title: Option<String>,
    },
}

/// `tool.*` 自定义事件的 schema 入口（当前仅 `tool.summary_updated`）。
///
/// 单条工具卡片（bash）的标题在命令执行后由 utility 模型异步生成，通过该事件
/// 按 `toolCallId` 热更新到前端；仅 live 生效，历史重载回落客户端占位。
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(tag = "type")]
pub enum ServeToolEvent {
    #[serde(rename = "tool.summary_updated")]
    ToolSummaryUpdated {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "summaryTitle", skip_serializing_if = "Option::is_none")]
        summary_title: Option<String>,
    },
}

/// plan / session todo 项的 wire schema 形状，与 `shared_todo_ops::items_json` 运行时输出一致。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ServeTodoItem {
    pub id: String,
    pub content: String,
    pub status: String,
}

/// 普通命令的 ack / error 响应。
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
    /// 构造成功响应。
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

    /// 构造失败响应。
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

/// 审批、初始化与取消等双向控制帧。
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

/// writer 下行队列里的统一帧类型。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum OutFrame {
    Response(ResponseFrame),
    Control(ControlFrame),
    Event(#[schemars(with = "ServeEvent")] Value),
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
