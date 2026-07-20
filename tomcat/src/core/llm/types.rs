//! # LLM 请求/响应类型
//!
//! 与 OpenAI API 兼容，供宿主与插件共用；字段命名与 pi-mono/OpenAI 对齐（snake_case）。
//!
//! ## 多模态 parts
//!
//! [`ChatMessageContentPart`] 是 `#[serde(tag = "type", rename_all = "snake_case")]`
//! 三态枚举：`InputText` / `InputImage` / `InputFile`，对齐 OpenAI Responses 的
//! `input_text` / `input_image` / `input_file` content part 形状。
//!
//! - **A 通道（inline base64）**：调用方传 `(mime_type, &Path)` 让 helper
//!   `image_b64` / `file_b64` 自己**打开文件 + metadata 二次校验 + 读字节 + base64
//!   编码**（PR-RJ-0 重构：避免 read 工具与 LLM 客户端各写一遍 IO）；wire 翻译
//!   时再拼 `data:{mime};base64,{b64}` data URL，封装在 [`OpenAiResponsesProvider`]
//!   内，类型层不暴露 wire 字符串。
//! - **B 通道（已知 file_id 透传 / 上传 helper）**：调用方可直接使用
//!   `image_file_id` / `file_file_id` 构造，也可调用异步上传 helper
//!   `image_upload` / `file_upload` 完成「字节上传 -> file_id part」一步到位。
//!
//! 限制：
//! - `IMAGE_MAX_BYTES = 4_718_592` (4.5 MB)，与 [`pi_agent_rust`] 一致
//! - `FILE_MAX_BYTES = 25 * 1024 * 1024` (25 MB)，按 OpenAI Responses 单次请求体硬上限近似
//! - image MIME 仅允许 `image/{png,jpeg,gif,webp}` 白名单（与 [`pi_agent_rust`] 对齐）

use std::path::Path;

use base64::Engine;
use serde::{Deserialize, Serialize};

use super::thinking_policy::ThinkingLevel;
use crate::core::llm::files_api::FilesApiAdapter;
use crate::core::llm::openai_files::FilePurpose;
use crate::infra::error::AppError;

/// inline 图片字节上限（解码后），与 [`pi_agent_rust/src/tools.rs`] 对齐。
pub const IMAGE_MAX_BYTES: usize = 4_718_592;

/// inline 文件字节上限（解码后）；OpenAI Responses 单次请求体硬上限 ~25 MB，
/// base64 膨胀 33%，所以 25 MB 字节已是 inline 路径的上沿。
pub const FILE_MAX_BYTES: usize = 25 * 1024 * 1024;

/// `count_tokens` 启发式：单张 inline 图片折合的字符数（≈ 1200 token），
/// 与 [`pi_agent_rust/src/compaction.rs`] `IMAGE_CHAR_ESTIMATE` 同值。
const IMAGE_CHAR_ESTIMATE: usize = 3600;

/// `count_tokens` 启发式：单份 inline 文件折合的字符数（≈ 2700 token），PDF 通常远大于单图。
const FILE_CHAR_ESTIMATE: usize = 8000;

/// image MIME 白名单（与 OpenAI vision 模型实际接受集合对齐）。
const ALLOWED_IMAGE_MIMES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// 单条对话消息，与 OpenAI chat completions messages 兼容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatMessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// 消息内容：纯文本或 parts 数组（便于扩展多模态）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChatMessageContent {
    Text(String),
    Parts(Vec<ChatMessageContentPart>),
}

/// 上下文引用类型：选区快照 or 文件路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextRefKind {
    Selection,
    File,
}

/// 结构化上下文引用，既用于 transcript 落盘，也用于发送前投影成 LLM 可读文本。
///
/// 外部 JSON 形态保持扁平，便于 transcript 直接落盘与回放：
///
/// ```json
/// {
///   "type": "input_reference",
///   "ref_kind": "selection",
///   "path": "src/app.ts",
///   "label": "app.ts:10-18",
///   "line_start": 10,
///   "line_end": 18,
///   "text": "const answer = 42;"
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ContextReference {
    pub ref_kind: ContextRefKind,
    pub path: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl ContextReference {
    pub fn selection(
        path: impl Into<String>,
        label: impl Into<String>,
        line_start: Option<u32>,
        line_end: Option<u32>,
        text: Option<String>,
    ) -> Self {
        Self {
            ref_kind: ContextRefKind::Selection,
            path: path.into(),
            label: label.into(),
            line_start,
            line_end,
            text,
        }
    }

    pub fn file(path: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            ref_kind: ContextRefKind::File,
            path: path.into(),
            label: label.into(),
            line_start: None,
            line_end: None,
            text: None,
        }
    }

    pub fn to_prompt_text(&self) -> String {
        match self.ref_kind {
            ContextRefKind::Selection => {
                let lines_attr = match (self.line_start, self.line_end) {
                    (Some(start), Some(end)) if start == end => format!(" lines=\"{start}\""),
                    (Some(start), Some(end)) => format!(" lines=\"{start}-{end}\""),
                    (Some(start), None) => format!(" lines=\"{start}\""),
                    _ => String::new(),
                };
                let text = self.text.as_deref().unwrap_or_default();
                let escaped_path = self
                    .path
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
                    .replace('\"', "&quot;");
                format!(
                    "<selection file=\"{}\"{}>\n{}\n</selection>",
                    escaped_path, lines_attr, text
                )
            }
            ContextRefKind::File => format!("[file reference] {}", self.path),
        }
    }
}

/// 单条 content part：文本 / 引用 / 图片 / 文件 四态枚举，wire 由 provider 适配层翻译。
///
/// 设计原则：把「inline base64」与「已知 file_id 引用」拆成 sum type，让非法状态
/// （如内联文件缺 filename、两条通道同时出现、两条通道都缺）无法通过类型层表达。
///
/// 外部 JSON 形态仍保持扁平：
///
/// ```json
/// {"type": "input_text",  "text": "..."}
/// {"type": "input_reference", "ref_kind": "file", "path": "src/app.ts", "label": "app.ts"}
/// {"type": "input_image", "mime_type": "image/png", "image_b64": "...", "detail": "high"}
/// {"type": "input_image", "file_id": "file-abc"}
/// {"type": "input_file",  "filename": "x.pdf", "mime_type": "application/pdf", "file_b64": "..."}
/// {"type": "input_file",  "file_id": "file-abc"}
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatMessageContentPart {
    /// 文本片段。
    InputText { text: String },
    /// 结构化上下文引用：选区快照或文件路径。
    InputReference {
        #[serde(flatten)]
        reference: ContextReference,
    },
    /// 图片：inline base64 或已知 file_id（二选一）。
    InputImage {
        #[serde(flatten)]
        source: ImageSource,
        /// vision detail：`auto` / `low` / `high`，可选，默认 auto。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// 文件（PDF / markdown 等）：inline base64 或已知 file_id（二选一）。
    InputFile {
        #[serde(flatten)]
        source: FileSource,
    },
}

/// 图片来源：inline base64 或已知 file_id。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ImageSource {
    Inline(ImageInlineSource),
    Uploaded(ImageUploadedSource),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageInlineSource {
    /// e.g. "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    pub mime_type: String,
    /// 标准 base64（不带 `data:` 前缀）；wire 拼装由 provider 层做。
    #[serde(rename = "image_b64")]
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageUploadedSource {
    /// OpenAI Files API 引用通道；本期 schema 保留 + 公开 helper 接收已知 id；
    /// 「读字节 → 上传 → 拿 id」由 T2-P0-015 提供。
    pub file_id: String,
}

/// 文件来源：inline base64 或已知 file_id。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FileSource {
    Inline(FileInlineSource),
    Uploaded(FileUploadedSource),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileInlineSource {
    pub filename: String,
    /// e.g. "application/pdf" / "text/markdown"
    pub mime_type: String,
    #[serde(rename = "file_b64")]
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileUploadedSource {
    pub file_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

impl ChatMessageContentPart {
    /// 文本片段。
    pub fn text(s: impl Into<String>) -> Self {
        Self::InputText { text: s.into() }
    }

    pub fn reference(reference: ContextReference) -> Self {
        Self::InputReference { reference }
    }

    /// inline 图片 helper（PR-RJ-0 重构）：从磁盘路径直接构造 `InputImage`。
    ///
    /// 调用方提供 `(mime_type, &Path)`，helper 内部完成：
    /// 1. MIME 白名单校验（`image/{png,jpeg,gif,webp}`）
    /// 2. `metadata().len()` **预检**（廉价，无 base64 33% 膨胀开销）
    /// 3. `std::fs::read(path)` 读字节
    /// 4. base64 编码并装入 `InputImage` variant
    ///
    /// 设计契约：
    /// - **不**接受 `data: String` 入参——避免 `read` 工具与 LLM 客户端各写一遍
    ///   `decode_b64_len + size check`，把唯一可信数据源固定到「文件路径」。
    /// - metadata 与 `read` 工具的 25 MiB metadata 预检**互不冲突**：read 工具在
    ///   路由前先做一道 metadata；本 helper 是 LLM 类型层的最后一道，确保即便
    ///   绕过 read 工具直接构造 part 也能拒绝超大字节。
    pub fn image_b64(
        mime_type: impl Into<String>,
        path: impl AsRef<Path>,
    ) -> Result<Self, AppError> {
        let mime = mime_type.into();
        let mime_lower = mime.to_ascii_lowercase();
        if !ALLOWED_IMAGE_MIMES.contains(&mime_lower.as_str()) {
            return Err(AppError::Llm(format!(
                "image_b64: 不支持的 mime_type {:?}, 仅允许 {:?}",
                mime, ALLOWED_IMAGE_MIMES
            )));
        }
        let path_ref = path.as_ref();
        let meta = std::fs::metadata(path_ref).map_err(|e| {
            AppError::Llm(format!(
                "image_b64: 无法 stat 路径 {}: {}",
                path_ref.display(),
                e
            ))
        })?;
        if meta.len() as usize > IMAGE_MAX_BYTES {
            return Err(AppError::Llm(format!(
                "image_b64: 图片 {} 字节超过 IMAGE_MAX_BYTES = {} 字节",
                meta.len(),
                IMAGE_MAX_BYTES
            )));
        }
        let bytes = std::fs::read(path_ref).map_err(|e| {
            AppError::Llm(format!(
                "image_b64: 读取 {} 失败: {}",
                path_ref.display(),
                e
            ))
        })?;
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(Self::InputImage {
            source: ImageSource::Inline(ImageInlineSource {
                mime_type: mime,
                data,
            }),
            detail: None,
        })
    }

    /// inline 图片 helper：直接接受 base64 文本，复用 MIME 白名单与解码后字节上限校验。
    pub fn image_base64_data(
        mime_type: impl Into<String>,
        data_base64: impl Into<String>,
    ) -> Result<Self, AppError> {
        let mime = mime_type.into();
        let mime_lower = mime.to_ascii_lowercase();
        if !ALLOWED_IMAGE_MIMES.contains(&mime_lower.as_str()) {
            return Err(AppError::Llm(format!(
                "image_base64_data: 不支持的 mime_type {:?}, 仅允许 {:?}",
                mime, ALLOWED_IMAGE_MIMES
            )));
        }
        let data = data_base64.into();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .map_err(|e| AppError::Llm(format!("image_base64_data: base64 解码失败: {e}")))?;
        if decoded.len() > IMAGE_MAX_BYTES {
            return Err(AppError::Llm(format!(
                "image_base64_data: 图片 {} 字节超过 IMAGE_MAX_BYTES = {} 字节",
                decoded.len(),
                IMAGE_MAX_BYTES
            )));
        }
        Ok(Self::InputImage {
            source: ImageSource::Inline(ImageInlineSource {
                mime_type: mime,
                data,
            }),
            detail: None,
        })
    }

    /// inline 文件 helper（PR-RJ-0 重构）：从磁盘路径直接构造 `InputFile`。
    ///
    /// 与 [`Self::image_b64`] 相同设计契约：metadata 预检 → 读字节 → base64 → 装 variant。
    /// 不做 MIME 白名单校验（PDF / 文本 / 二进制都可走 inline 文件通道）。
    pub fn file_b64(
        filename: impl Into<String>,
        mime_type: impl Into<String>,
        path: impl AsRef<Path>,
    ) -> Result<Self, AppError> {
        let path_ref = path.as_ref();
        let meta = std::fs::metadata(path_ref).map_err(|e| {
            AppError::Llm(format!(
                "file_b64: 无法 stat 路径 {}: {}",
                path_ref.display(),
                e
            ))
        })?;
        if meta.len() as usize > FILE_MAX_BYTES {
            return Err(AppError::Llm(format!(
                "file_b64: 文件 {} 字节超过 FILE_MAX_BYTES = {} 字节",
                meta.len(),
                FILE_MAX_BYTES
            )));
        }
        let bytes = std::fs::read(path_ref).map_err(|e| {
            AppError::Llm(format!("file_b64: 读取 {} 失败: {}", path_ref.display(), e))
        })?;
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(Self::InputFile {
            source: FileSource::Inline(FileInlineSource {
                filename: filename.into(),
                mime_type: mime_type.into(),
                data,
            }),
        })
    }

    /// inline 文件 helper：直接接受 base64 文本，复用解码后字节上限校验。
    pub fn file_base64_data(
        filename: impl Into<String>,
        mime_type: impl Into<String>,
        data_base64: impl Into<String>,
    ) -> Result<Self, AppError> {
        let data = data_base64.into();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .map_err(|e| AppError::Llm(format!("file_base64_data: base64 解码失败: {e}")))?;
        if decoded.len() > FILE_MAX_BYTES {
            return Err(AppError::Llm(format!(
                "file_base64_data: 文件 {} 字节超过 FILE_MAX_BYTES = {} 字节",
                decoded.len(),
                FILE_MAX_BYTES
            )));
        }
        Ok(Self::InputFile {
            source: FileSource::Inline(FileInlineSource {
                filename: filename.into(),
                mime_type: mime_type.into(),
                data,
            }),
        })
    }

    /// 已知 file_id 引用图片（B 通道），不做字节大小校验（字节已在 OpenAI 侧）。
    pub fn image_file_id(file_id: impl Into<String>) -> Result<Self, AppError> {
        let id = file_id.into();
        if id.trim().is_empty() {
            return Err(AppError::Llm("image_file_id: file_id 不能为空".to_string()));
        }
        Ok(Self::InputImage {
            source: ImageSource::Uploaded(ImageUploadedSource { file_id: id }),
            detail: None,
        })
    }

    /// 已知 file_id 引用文件（B 通道），可附带 filename 提示。
    pub fn file_file_id(
        file_id: impl Into<String>,
        filename: Option<String>,
    ) -> Result<Self, AppError> {
        let id = file_id.into();
        if id.trim().is_empty() {
            return Err(AppError::Llm("file_file_id: file_id 不能为空".to_string()));
        }
        Ok(Self::InputFile {
            source: FileSource::Uploaded(FileUploadedSource {
                file_id: id,
                filename,
            }),
        })
    }

    /// 上传图片到 OpenAI Files（B 通道），并返回 `file_id` part。
    ///
    /// 仅在当前 provider 声明支持 Files API 时应调用；否则请回退 inline helper。
    pub async fn image_upload(
        adapter: &(impl FilesApiAdapter + ?Sized),
        mime_type: impl Into<String>,
        bytes: &[u8],
        filename: impl Into<String>,
    ) -> Result<Self, AppError> {
        let filename = filename.into();
        let mime = mime_type.into();
        let mime_lower = mime.to_ascii_lowercase();
        if !ALLOWED_IMAGE_MIMES.contains(&mime_lower.as_str()) {
            return Err(AppError::Llm(format!(
                "image_upload: 不支持的 mime_type {:?}, 仅允许 {:?}",
                mime, ALLOWED_IMAGE_MIMES
            )));
        }
        if bytes.is_empty() {
            return Err(AppError::Llm("image_upload: 文件内容为空".to_string()));
        }
        let upload = adapter
            .upload(FilePurpose::Vision, &filename, &mime, bytes)
            .await?;
        Self::image_file_id(upload.id)
    }

    /// 上传通用文件到 OpenAI Files（B 通道），并返回 `file_id` part。
    pub async fn file_upload(
        adapter: &(impl FilesApiAdapter + ?Sized),
        filename: impl Into<String>,
        mime_type: impl Into<String>,
        bytes: &[u8],
    ) -> Result<Self, AppError> {
        let filename = filename.into();
        let mime = mime_type.into();
        if bytes.is_empty() {
            return Err(AppError::Llm("file_upload: 文件内容为空".to_string()));
        }
        let upload = adapter
            .upload(FilePurpose::UserData, &filename, &mime, bytes)
            .await?;
        Self::file_file_id(upload.id, Some(filename))
    }

    /// `count_tokens` 启发式：按变体折算字符数；inline 字节不进入字符统计。
    pub(crate) fn estimated_chars(&self) -> usize {
        match self {
            Self::InputText { text } => text.chars().count(),
            Self::InputReference { reference } => reference.to_prompt_text().chars().count(),
            Self::InputImage { .. } => IMAGE_CHAR_ESTIMATE,
            Self::InputFile { .. } => FILE_CHAR_ESTIMATE,
        }
    }
}

/// 仅供测试 / 已知 base64 字符串场景：解码并返回字节长度。
///
/// PR-RJ-0 重构后生产路径改走 [`ChatMessageContentPart::image_b64`] /
/// [`ChatMessageContentPart::file_b64`]（直接读盘 + base64），本函数保留
/// 给单元测试断言「base64 编/解码长度对齐」的边角场景。
#[allow(dead_code)]
fn decode_b64_len(data: &str) -> Result<usize, base64::DecodeError> {
    base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map(|v| v.len())
}

/// Internal semantic tag for messages that share the same LLM wire role.
/// `#[serde(skip)]` — never serialized; defaults to `Normal` on deserialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MessageKind {
    #[default]
    Normal,
    /// Steering instruction injected mid-turn; LLM sees `role: user`.
    Steering,
    /// Compaction summary replacing older messages; LLM sees `role: user`.
    CompactionSummary,
}

/// Assistant turn 中 opaque continuity blob 的格式标签。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningFormat {
    OpenaiResponsesReasoningItems,
    DeepseekReasoningContent,
    AnthropicThinkingBlocks,
}

/// 同一条 continuity 材料在下一轮 replay 时的强弱要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayRequirement {
    #[default]
    Never,
    SameProfileOptional,
    SameProfileRequired,
}

/// provider 私有的附加引用，仅供同类 wire 优化分支使用。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ProviderRefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_response_id: Option<String>,
    /// Stable replay-profile identity for route-sensitive providers (for example different
    /// OpenAI-compatible relays sharing the same wire protocol).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_profile_id: Option<String>,
}

/// 可供下一轮继续推理的 opaque continuity 材料。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ReasoningContinuation {
    pub source_provider: String,
    pub source_api: String,
    pub source_model: String,
    pub format: ReasoningFormat,
    pub opaque_payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_refs: Option<ProviderRefs>,
}

/// transcript assistant turn 的 replay 元数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ContinuityMetadata {
    #[serde(default)]
    pub had_tool_call: bool,
    #[serde(default)]
    pub replay_requirement: ReplayRequirement,
}

/// 单条对话消息（与 OpenAI API 兼容，wire 格式为 snake_case）。
///
/// `finish_reason/error_message/error_code` 会随 transcript assistant message 一起持久化；
/// `msg_id/kind/timestamp` 仍是纯本地 bookkeeping，不出进程边界。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatMessage {
    pub role: ChatMessageRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<ChatMessageContent>,
    /// Provider-specific structured metadata such as URL citations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Responses / transcript 终局元数据；仅本地持久化与恢复使用，不参与 wire 语义。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// 可读 thinking 摘要 / 文本；用于展示、审计与跨 provider downgrade。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_text: Option<String>,
    /// 机器可读的 continuity blob；同类 provider/wire 可高保真 replay。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_continuation: Option<ReasoningContinuation>,
    /// replay 所需的 turn 级元数据；旧 transcript 缺失时按 None 兼容。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuity: Option<ContinuityMetadata>,
    /// turn/tool 折叠标题；仅本地持久化与 transcript/webview 恢复使用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_title: Option<String>,

    /// Transcript `MessageEntry.id` — set during hydration or after `append_message`.
    #[serde(skip)]
    pub msg_id: Option<String>,
    /// Semantic tag distinguishing steering / compaction-summary from normal messages.
    #[serde(skip)]
    pub kind: MessageKind,
    /// ISO-8601 timestamp from the transcript, used for day-based filtering.
    #[serde(skip)]
    pub timestamp: Option<String>,
}

impl ChatMessage {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: ChatMessageRole::User,
            content: Some(ChatMessageContent::Text(text.into())),
            annotations: None,
            name: None,
            tool_calls: None,
            tool_call_id: None,
            finish_reason: None,
            error_message: None,
            error_code: None,
            thinking_text: None,
            reasoning_continuation: None,
            continuity: None,
            summary_title: None,
            msg_id: None,
            kind: MessageKind::Normal,
            timestamp: None,
        }
    }

    /// 多模态 user 消息：parts 数组直接驱动 `Responses /v1/responses` 的
    /// `content` 字段。空 parts 仍允许，wire 层会兜底成单个空 `input_text`。
    pub fn user_with_parts(parts: Vec<ChatMessageContentPart>) -> Self {
        Self {
            role: ChatMessageRole::User,
            content: Some(ChatMessageContent::Parts(parts)),
            annotations: None,
            name: None,
            tool_calls: None,
            tool_call_id: None,
            finish_reason: None,
            error_message: None,
            error_code: None,
            thinking_text: None,
            reasoning_continuation: None,
            continuity: None,
            summary_title: None,
            msg_id: None,
            kind: MessageKind::Normal,
            timestamp: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: ChatMessageRole::Assistant,
            content: Some(ChatMessageContent::Text(text.into())),
            annotations: None,
            name: None,
            tool_calls: None,
            tool_call_id: None,
            finish_reason: None,
            error_message: None,
            error_code: None,
            thinking_text: None,
            reasoning_continuation: None,
            continuity: None,
            summary_title: None,
            msg_id: None,
            kind: MessageKind::Normal,
            timestamp: None,
        }
    }

    pub fn assistant_with_tool_calls(
        content: Option<&str>,
        tool_calls: Vec<serde_json::Value>,
    ) -> Self {
        Self {
            role: ChatMessageRole::Assistant,
            content: content.map(|s| ChatMessageContent::Text(s.to_string())),
            annotations: None,
            name: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            finish_reason: None,
            error_message: None,
            error_code: None,
            thinking_text: None,
            reasoning_continuation: None,
            continuity: None,
            summary_title: None,
            msg_id: None,
            kind: MessageKind::Normal,
            timestamp: None,
        }
    }

    pub fn tool(tool_call_id: &str, content: &str) -> Self {
        Self {
            role: ChatMessageRole::Tool,
            content: Some(ChatMessageContent::Text(content.to_string())),
            annotations: None,
            name: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.to_string()),
            finish_reason: None,
            error_message: None,
            error_code: None,
            thinking_text: None,
            reasoning_continuation: None,
            continuity: None,
            summary_title: None,
            msg_id: None,
            kind: MessageKind::Normal,
            timestamp: None,
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: ChatMessageRole::System,
            content: Some(ChatMessageContent::Text(text.into())),
            annotations: None,
            name: None,
            tool_calls: None,
            tool_call_id: None,
            finish_reason: None,
            error_message: None,
            error_code: None,
            thinking_text: None,
            reasoning_continuation: None,
            continuity: None,
            summary_title: None,
            msg_id: None,
            kind: MessageKind::Normal,
            timestamp: None,
        }
    }

    pub fn steering(text: impl Into<String>) -> Self {
        Self {
            role: ChatMessageRole::User,
            content: Some(ChatMessageContent::Text(text.into())),
            annotations: None,
            name: None,
            tool_calls: None,
            tool_call_id: None,
            finish_reason: None,
            error_message: None,
            error_code: None,
            thinking_text: None,
            reasoning_continuation: None,
            continuity: None,
            summary_title: None,
            msg_id: None,
            kind: MessageKind::Steering,
            timestamp: None,
        }
    }

    pub fn compaction_summary(text: impl Into<String>) -> Self {
        Self {
            role: ChatMessageRole::User,
            content: Some(ChatMessageContent::Text(text.into())),
            annotations: None,
            name: None,
            tool_calls: None,
            tool_call_id: None,
            finish_reason: None,
            error_message: None,
            error_code: None,
            thinking_text: None,
            reasoning_continuation: None,
            continuity: None,
            summary_title: None,
            msg_id: None,
            kind: MessageKind::CompactionSummary,
            timestamp: None,
        }
    }

    /// 为 assistant/tool 回合结果附加终局元数据；用于 transcript 持久化与 reload。
    pub fn with_completion_metadata(
        mut self,
        finish_reason: Option<String>,
        error_message: Option<String>,
        error_code: Option<String>,
    ) -> Self {
        self.finish_reason = finish_reason;
        self.error_message = error_message;
        self.error_code = error_code;
        self
    }

    /// 为 assistant turn 附加 continuity 主账本字段；仅影响 transcript 持久化与 replay。
    pub fn with_reasoning_state(
        mut self,
        thinking_text: Option<String>,
        reasoning_continuation: Option<ReasoningContinuation>,
        continuity: Option<ContinuityMetadata>,
    ) -> Self {
        self.thinking_text = thinking_text;
        self.reasoning_continuation = reasoning_continuation;
        self.continuity = continuity;
        self
    }

    /// 为 assistant/tool 回合附加 transcript/webview 使用的折叠摘要标题。
    pub fn with_summary_title(mut self, summary_title: Option<String>) -> Self {
        self.summary_title = summary_title;
        self
    }

    /// 请求发往上游前剥离本地 transcript 元数据，避免污染 API wire payload。
    pub fn without_completion_metadata(&self) -> Self {
        let mut cloned = self.clone();
        cloned.annotations = None;
        cloned.finish_reason = None;
        cloned.error_message = None;
        cloned.error_code = None;
        cloned.thinking_text = None;
        cloned.reasoning_continuation = None;
        cloned.continuity = None;
        cloned.summary_title = None;
        cloned
    }

    /// Replace the text content in-place (used by L0/L1 compaction on tool results).
    pub fn set_text_content(&mut self, text: String) {
        self.content = Some(ChatMessageContent::Text(text));
    }

    /// Helper to extract text content (for backward compat).
    pub fn text_content(&self) -> Option<&str> {
        match &self.content {
            Some(ChatMessageContent::Text(s)) => Some(s),
            _ => None,
        }
    }

    /// Extract user-authored text from plain text or structured parts.
    ///
    /// For multipart content we only keep `input_text` chunks and ignore references/files/images,
    /// because session titles should be derived from what the user typed rather than projected
    /// context labels or attachment metadata.
    pub fn first_text(&self) -> Option<String> {
        match &self.content {
            Some(ChatMessageContent::Text(s)) => Some(s.clone()),
            Some(ChatMessageContent::Parts(parts)) => {
                let mut text = String::new();
                let mut saw_input_text = false;
                for part in parts {
                    if let ChatMessageContentPart::InputText { text: chunk } = part {
                        saw_input_text = true;
                        text.push_str(chunk);
                    }
                }
                saw_input_text.then_some(text)
            }
            None => None,
        }
    }
}

/// 会话级模型覆盖；若为 None，使用全局 LlmConfig.default_model。
/// 后续 SessionManager 可用时由上层从 SessionEntry.model_override 填入。
/// 与 OpenAI API 请求体兼容（snake_case）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// 会话级模型覆盖（不发给 API，仅用于选模型）。
    #[serde(skip)]
    pub model_override: Option<String>,
    #[serde(skip)]
    pub thinking_level: Option<ThinkingLevel>,
    /// OpenAI function calling: tool definitions sent to the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<serde_json::Value>>,
}

/// 单次调用的 token 使用量（与 OpenAI API 一致，snake_case）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u32>,
}

/// Thinking/Reasoning 增量来源：原始推理链路（raw）或模型给出的摘要（summary）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingSource {
    Summary,
    Raw,
}

/// 非流式聊天响应，与 OpenAI 格式一致（snake_case）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatResponse {
    pub id: Option<String>,
    pub choices: Vec<ChatResponseChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatResponseChoice {
    pub index: u32,
    pub message: ChatMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

/// 流式事件类型，与 pi-mono 流式 API 对齐。
///
/// `Thinking` 与 `ContentDelta` 物理同源（都是模型流），但 **语义分通道**：
/// - `ContentDelta`：assistant 正文，进 Markdown 渲染 / transcript；
/// - `Thinking`：思考/推理增量（OpenAI `reasoning_content`、Responses
///   `response.reasoning_summary_text.delta`、Anthropic `thinking_delta` 等
///   归一映射），由上层决定是否折叠展示与是否落盘。
///
/// 详细决策见 `docs/architecture/llm-stream-events-cli-pipeline.md` §4.1 R2 / §5.1。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    ContentDelta {
        delta: String,
    },
    /// 思考/推理增量；`source` 区分 summary 与 raw，`signature` 仅 Anthropic 类协议会带
    /// （用于多轮重发校验）。
    Thinking {
        delta: String,
        source: ThinkingSource,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// provider 在流结束前上报的 reasoning continuity 快照；仅供 transcript/replay 主链消费。
    ReasoningSnapshot {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking_text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_continuation: Option<ReasoningContinuation>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        continuity: Option<ContinuityMetadata>,
    },
    /// Tool call 增量（OpenAI streaming 格式）。
    ToolCallDelta {
        index: u32,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: Option<String>,
    },
    FinishReason {
        reason: String,
    },
    /// 结构化 LLM 终局错误，供上层事件总线 / CLI / transcript 账本消费。
    LlmError {
        reason: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    /// 非错误终局提示（当前主要用于 `max_output_tokens` 截断轻提示）。
    LlmNotice {
        finish_reason: String,
        message: String,
    },
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: Option<u32>,
    },
}
