//! `tomcat serve` 的协议类型定义。
//!
//! 这里集中承载：
//! - UI -> agent 的命令帧
//! - 双向控制帧
//! - agent -> UI 的响应/事件信封
//! - schema 导出所需的 `schemars` 派生

use std::collections::BTreeMap;

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
///
/// **这里只有引用，没有字节。** 字节在粘贴那一刻就已经通过 `ingest_attachment` 交给后端了，
/// 发送时只说「用那份哈希对应的字节」。这让打字与发送两条路径的载荷都与图片大小无关。
///
/// `blob_sha` 与 `file_id` 二选一：前者是本机 ingest 过的字节，后者是已上传到 provider 的文件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServeAttachment {
    pub kind: ServeAttachmentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// 已经过 `ingest_attachment` 校验并落盘的字节的 sha256。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_sha: Option<String>,
    /// 发给模型用的那份字节的 sha256。仅 SVG 会与 `blob_sha` 不同（webview 转出的 PNG）。
    /// 省略则表示模型直接用 `blob_sha`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_sha: Option<String>,
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
    #[serde(default)]
    pub detached: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ServeSessionMode>,
}

/// 历史消息里的图片附件以什么形态回给调用方。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentMode {
    /// 原样回 transcript 里的 base64。**默认值**，保证 CLI 与既有调用方行为不变。
    #[default]
    Inline,
    /// UI 只回内容哈希引用；字节始终保留在 `attachments/blobs/`。
    ///
    /// webview 走这一条：字节由 Chromium 通过资源 URI 自己去拉，完全不进 JS 内存。
    Reference,
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
    #[serde(default)]
    pub attachment_mode: AttachmentMode,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
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
    /// 在不追加用户消息的前提下，基于当前 transcript 继续请求模型。
    #[serde(rename_all = "camelCase")]
    Resume {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    /// 复制指定失败输入为新的活 user message，然后重新开一轮。
    #[serde(rename_all = "camelCase")]
    Retry {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "messageId")]
        message_id: String,
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
    /// 主动压缩当前会话上下文，保留摘要以继续会话。
    #[serde(rename_all = "camelCase")]
    Compact {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
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
    SetContextWindow {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        model: String,
        #[serde(rename = "contextWindow")]
        context_window: u32,
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
    ListConnectors {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ListConnectorTools {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    AddConnector {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
        command: String,
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oauth: Option<Value>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<String>,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    RemoveConnector {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    SetConnectorTrust {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
        trusted: bool,
    },
    #[serde(rename_all = "camelCase")]
    TestConnector {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    LoginConnector {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    CancelLoginConnector {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    LogoutConnector {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    ReloadConnector {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    SetConnectorToolFilter {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
        #[serde(default)]
        include: Vec<String>,
        #[serde(default)]
        exclude: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
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
    /// 回滚一个尚未进入 live registry 的 detached 会话。幂等；绝不激活目标。
    #[serde(rename_all = "camelCase")]
    DiscardDetachedSession {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(rename = "sessionId")]
        session_id: String,
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
    /// 把一份附件字节交给后端收好，换回内容哈希。
    ///
    /// **这是全协议唯一携带图片字节的命令。** 它在粘贴/拖入/选择文件时逐张调用一次，
    /// 之后所有环节（打字、快照、发送、历史渲染）只传哈希。
    /// `r-test-payload-contract` 用 schema 静态扫描守住这条不变量。
    #[serde(rename_all = "camelCase")]
    IngestAttachment {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        attachment: IngestAttachmentInput,
    },
    /// 为 detached 目标会话批量保留现有 blob/provider rendition 租约，不携带字节。
    #[serde(rename_all = "camelCase")]
    RetainAttachmentLeases {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(rename = "sessionId")]
        session_id: String,
        params: RetainAttachmentLeasesParams,
    },
    /// 补交一张历史图的缩略图（见 [`CacheThumbnailInput`]）。
    #[serde(rename_all = "camelCase")]
    CacheAttachmentThumbnail {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        thumbnail: CacheThumbnailInput,
    },
}

/// `retain_attachment_leases` 的单个附件引用。只有内容哈希会过 wire；原始字节仍只允许
/// `ingest_attachment` 携带。`provider_sha` 省略时 provider 与原始 blob 共用同一份字节。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetainAttachmentLeaseRef {
    pub blob_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_sha: Option<String>,
}

/// `retain_attachment_leases` 的入参。后端会把所有 blob/provider SHA 合并去重后原子保留。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetainAttachmentLeasesParams {
    pub attachments: Vec<RetainAttachmentLeaseRef>,
}

/// `retain_attachment_leases` 的响应。按字典序返回实际保留的去重 SHA 集合。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetainAttachmentLeasesResponse {
    pub retained_shas: Vec<String>,
}

/// `ingest_attachment` 的入参。
///
/// 三份字节各有明确分工，都由 webview 侧的 Chromium 产出：
/// - `data_base64`：原始字节，用于显示与（非 SVG 时）发给模型
/// - `thumb_base64`：192px 缩略图，附件条与 filmstrip 只加载它
/// - `provider_base64`：仅 SVG 需要 —— 模型的视觉接口不认 SVG，webview 用 canvas 转成 PNG
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IngestAttachmentInput {
    pub kind: ServeAttachmentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    pub mime_type: String,
    pub data_base64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_base64: Option<String>,
    /// `provider_base64` 的 MIME（SVG 转出的 PNG 即 `image/png`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_mime_type: Option<String>,
}

/// `cache_attachment_thumbnail` 的入参。
///
/// 历史消息里的图片没有经过 `ingest_attachment`（它们的字节来自 transcript），
/// 所以缩略图要在 webview 第一次渲染那个会话时补生成一次，然后由后端存下来复用。
/// 之后再打开同一个会话就直接拿 192px 的缩略图，不必再解一遍原图。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CacheThumbnailInput {
    /// 缩略图所派生自的那份字节的 sha256。
    pub source_sha: String,
    pub thumb_base64: String,
}

/// `ingest_attachment` 的响应。
///
/// 从这里开始，这份附件在系统里的身份就是这几个哈希，不再是字节。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IngestAttachmentResponse {
    /// 原始字节的 sha256。
    pub blob_sha: String,
    /// 缩略图是否已就绪。
    ///
    /// 缩略图按**源字节的哈希**存在 `thumbs/<blob_sha>`，而不是按它自己的哈希存。
    /// 理由：没有任何人会问「给我哈希是 Y 的那张缩略图」，大家问的都是
    /// 「给我 blob X 的小图」。按自身哈希存会让它变成一份普通 blob —— 于是要占租约、
    /// 要参与 GC，而它本质上只是可随时重建的派生数据。所以这里只回一个布尔值，
    /// URI 也就固定是 `thumbs/<blobSha>`，历史图与新粘贴的图走同一套路径。
    pub has_thumb: bool,
    /// 发给模型用的那份字节的 sha256；仅 SVG 会与 `blob_sha` 不同。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_sha: Option<String>,
    /// 原始字节数（十进制），扩展层用它做草稿总量预算。
    pub bytes: u64,
    pub mime_type: String,
    pub filename: String,
}

impl ServeCommand {
    pub fn command_id(&self) -> Option<&str> {
        match self {
            Self::Prompt { id, .. }
            | Self::Steer { id, .. }
            | Self::FollowUp { id, .. }
            | Self::Resume { id, .. }
            | Self::Retry { id, .. }
            | Self::GetState { id, .. }
            | Self::ListCheckpoints { id, .. }
            | Self::RestoreCheckpoint { id, .. }
            | Self::Compact { id, .. }
            | Self::SetPlanMode { id, .. }
            | Self::SetModel { id, .. }
            | Self::SetThinkingLevel { id, .. }
            | Self::SetContextWindow { id, .. }
            | Self::ListModels { id, .. }
            | Self::UpsertModel { id, .. }
            | Self::RemoveModel { id, .. }
            | Self::SetProviderKey { id, .. }
            | Self::ListProviderKeys { id, .. }
            | Self::ListConnectors { id, .. }
            | Self::ListConnectorTools { id, .. }
            | Self::AddConnector { id, .. }
            | Self::RemoveConnector { id, .. }
            | Self::SetConnectorTrust { id, .. }
            | Self::TestConnector { id, .. }
            | Self::LoginConnector { id, .. }
            | Self::CancelLoginConnector { id, .. }
            | Self::LogoutConnector { id, .. }
            | Self::ReloadConnector { id, .. }
            | Self::SetConnectorToolFilter { id, .. }
            | Self::NewSession { id, .. }
            | Self::SwitchSession { id, .. }
            | Self::GetMessages { id, .. }
            | Self::CloseSession { id, .. }
            | Self::DiscardDetachedSession { id, .. }
            | Self::ListSessions { id, .. }
            | Self::Interrupt { id, .. }
            | Self::IngestAttachment { id, .. }
            | Self::RetainAttachmentLeases { id, .. }
            | Self::CacheAttachmentThumbnail { id, .. } => id.as_deref(),
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
            | Self::Resume { session_id, .. }
            | Self::Retry { session_id, .. }
            | Self::GetState { session_id, .. }
            | Self::ListCheckpoints { session_id, .. }
            | Self::RestoreCheckpoint { session_id, .. }
            | Self::Compact { session_id, .. }
            | Self::SetPlanMode { session_id, .. }
            | Self::SetModel { session_id, .. }
            | Self::SetThinkingLevel { session_id, .. }
            | Self::SetContextWindow { session_id, .. }
            | Self::GetMessages { session_id, .. }
            | Self::CloseSession { session_id, .. }
            | Self::Interrupt { session_id, .. }
            | Self::IngestAttachment { session_id, .. }
            | Self::CacheAttachmentThumbnail { session_id, .. }
            | Self::ControlRequest { session_id, .. }
            | Self::ControlResponse { session_id, .. }
            | Self::ControlCancel { session_id, .. } => session_id.as_deref(),
            Self::SwitchSession { session_id, .. }
            | Self::RetainAttachmentLeases { session_id, .. }
            | Self::DiscardDetachedSession { session_id, .. } => Some(session_id.as_str()),
            Self::NewSession { .. }
            | Self::ListModels { .. }
            | Self::UpsertModel { .. }
            | Self::RemoveModel { .. }
            | Self::SetProviderKey { .. }
            | Self::ListProviderKeys { .. }
            | Self::ListConnectors { .. }
            | Self::ListConnectorTools { .. }
            | Self::AddConnector { .. }
            | Self::RemoveConnector { .. }
            | Self::SetConnectorTrust { .. }
            | Self::TestConnector { .. }
            | Self::LoginConnector { .. }
            | Self::CancelLoginConnector { .. }
            | Self::LogoutConnector { .. }
            | Self::ReloadConnector { .. }
            | Self::SetConnectorToolFilter { .. }
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
            Self::Resume { .. } => "resume",
            Self::Retry { .. } => "retry",
            Self::GetState { .. } => "get_state",
            Self::ListCheckpoints { .. } => "list_checkpoints",
            Self::RestoreCheckpoint { .. } => "restore_checkpoint",
            Self::Compact { .. } => "compact",
            Self::SetPlanMode { .. } => "set_plan_mode",
            Self::SetModel { .. } => "set_model",
            Self::SetThinkingLevel { .. } => "set_thinking_level",
            Self::SetContextWindow { .. } => "set_context_window",
            Self::ListModels { .. } => "list_models",
            Self::UpsertModel { .. } => "upsert_model",
            Self::RemoveModel { .. } => "remove_model",
            Self::SetProviderKey { .. } => "set_provider_key",
            Self::ListProviderKeys { .. } => "list_provider_keys",
            Self::ListConnectors { .. } => "list_connectors",
            Self::ListConnectorTools { .. } => "list_connector_tools",
            Self::AddConnector { .. } => "add_connector",
            Self::RemoveConnector { .. } => "remove_connector",
            Self::SetConnectorTrust { .. } => "set_connector_trust",
            Self::TestConnector { .. } => "test_connector",
            Self::LoginConnector { .. } => "login_connector",
            Self::CancelLoginConnector { .. } => "cancel_login_connector",
            Self::LogoutConnector { .. } => "logout_connector",
            Self::ReloadConnector { .. } => "reload_connector",
            Self::SetConnectorToolFilter { .. } => "set_connector_tool_filter",
            Self::NewSession { .. } => "new_session",
            Self::SwitchSession { .. } => "switch_session",
            Self::GetMessages { .. } => "get_messages",
            Self::CloseSession { .. } => "close_session",
            Self::DiscardDetachedSession { .. } => "discard_detached_session",
            Self::ListSessions { .. } => "list_sessions",
            Self::Interrupt { .. } => "interrupt",
            Self::IngestAttachment { .. } => "ingest_attachment",
            Self::RetainAttachmentLeases { .. } => "retain_attachment_leases",
            Self::CacheAttachmentThumbnail { .. } => "cache_attachment_thumbnail",
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
    #[serde(rename = "session.agent_mode.changed")]
    SessionAgentModeChanged {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "agentMode", skip_serializing_if = "Option::is_none")]
        agent_mode: Option<String>,
    },
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
    #[serde(rename = "plan.review.started")]
    PlanReviewStarted {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
        #[serde(rename = "childSessionId", skip_serializing_if = "Option::is_none")]
        child_session_id: Option<String>,
        #[serde(rename = "transcriptPath", skip_serializing_if = "Option::is_none")]
        transcript_path: Option<String>,
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
    #[serde(rename = "plan.code_review.started")]
    PlanCodeReviewStarted {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        round: Option<u32>,
        #[serde(rename = "reviewAttemptId", skip_serializing_if = "Option::is_none")]
        review_attempt_id: Option<String>,
        #[serde(rename = "toolCallId", skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        #[serde(rename = "childSessionId", skip_serializing_if = "Option::is_none")]
        child_session_id: Option<String>,
        #[serde(rename = "transcriptPath", skip_serializing_if = "Option::is_none")]
        transcript_path: Option<String>,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        verdict: Option<String>,
        #[serde(rename = "changesSummary", skip_serializing_if = "Option::is_none")]
        changes_summary: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        findings: Option<Vec<ServeFinding>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rounds: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        round: Option<u32>,
        #[serde(rename = "reviewAttemptId", skip_serializing_if = "Option::is_none")]
        review_attempt_id: Option<String>,
        #[serde(rename = "toolCallId", skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        #[serde(rename = "childSessionId", skip_serializing_if = "Option::is_none")]
        child_session_id: Option<String>,
    },
    #[serde(rename = "plan.explorer.started")]
    PlanExplorerStarted {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "taskId", skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
        #[serde(rename = "childSessionId", skip_serializing_if = "Option::is_none")]
        child_session_id: Option<String>,
        #[serde(rename = "transcriptPath", skip_serializing_if = "Option::is_none")]
        transcript_path: Option<String>,
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
    /// 轮次预算用尽后的 review 收口事件：正常路径无条件放行进 acceptance 并携带残余；
    /// 基础设施连续故障时仍可能以 handoff outcome 交还用户。
    #[serde(rename = "plan.code_review.exhausted")]
    PlanCodeReviewExhausted {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rounds: Option<u32>,
        #[serde(rename = "unresolvedFindings", skip_serializing_if = "Option::is_none")]
        unresolved_findings: Option<Vec<String>>,
        #[serde(rename = "residualFindings", skip_serializing_if = "Option::is_none")]
        residual_findings: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        outcome: Option<String>,
        #[serde(rename = "codeReviewPass", skip_serializing_if = "Option::is_none")]
        code_review_pass: Option<bool>,
    },
    /// 预算耗尽后的代码改动没有再次派发 reviewer；仅作为审计事件保留。
    #[serde(rename = "plan.code_review.unreviewed_edit")]
    PlanCodeReviewUnreviewedEdit {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "planId", skip_serializing_if = "Option::is_none")]
        plan_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rounds: Option<u32>,
        #[serde(
            rename = "maxCodeReviewRounds",
            skip_serializing_if = "Option::is_none"
        )]
        max_code_review_rounds: Option<u32>,
        #[serde(rename = "changedCodeFiles", skip_serializing_if = "Option::is_none")]
        changed_code_files: Option<Vec<String>>,
        #[serde(rename = "newestEditMtimeMs", skip_serializing_if = "Option::is_none")]
        newest_edit_mtime_ms: Option<u128>,
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
        #[serde(rename = "cacheObservation", skip_serializing_if = "Option::is_none")]
        cache_observation: Option<ServeCacheObservation>,
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

/// `tool.*` 自定义事件的 schema 入口（`tool.summary_updated` / `background_task_finished`）。
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
    #[serde(rename = "background_task_finished")]
    BackgroundTaskFinished {
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "exitCode")]
        exit_code: i32,
        #[serde(rename = "logPath", skip_serializing_if = "Option::is_none")]
        log_path: Option<String>,
        #[serde(rename = "command", skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ServeFinding {
    pub severity: String,
    pub area: String,
    pub note: String,
}

/// Cache metrics collected during the active build. A missing `cacheHitRatio` means the
/// provider did not report cache-read usage, not that every request missed.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServeCacheObservation {
    pub prompt_tokens_total: u64,
    pub cache_read_tokens_total: u64,
    pub cache_observed_prompt_tokens_total: u64,
    pub cache_observed_request_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_hit_ratio: Option<f64>,
    pub consecutive_miss_max: u32,
    pub tail_changed_count: u32,
    pub tail_change_miss_tokens: u64,
}

/// plan / session todo 项的 wire schema 形状，与 `shared_todo_ops::items_json` 运行时输出一致。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ServeTodoItem {
    pub id: String,
    pub content: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
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
