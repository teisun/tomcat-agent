use std::sync::Arc;

use async_trait::async_trait;
use chrono::DateTime;
use serde::Deserialize;
use tracing::warn;

use super::openai_files::{
    FilePurpose, FilesApiProviderContext, OpenAiFileMeta, OpenAiFilesClient,
};
use super::retry_delay::provider_retry_delay;
use crate::infra::config::LlmFilesConfig;
use crate::infra::error::{
    is_retryable_llm_error, llm_error_with_source, llm_http_status_error_with_summary, AppError,
    LlmErrorStage,
};

pub const ANTHROPIC_FILES_BETA: &str = "files-api-2025-04-14";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageRefSlot {
    FileIdField,
    UrlField,
}

#[async_trait]
pub trait FilesApiAdapter: Send + Sync + std::fmt::Debug {
    async fn upload(
        &self,
        purpose: FilePurpose,
        filename: &str,
        mime_type: &str,
        bytes: &[u8],
    ) -> Result<OpenAiFileMeta, AppError>;

    async fn delete(&self, file_id: &str) -> Result<(), AppError>;

    fn expires_after_seconds(&self) -> u64;

    fn reference_token(&self, file_id: &str) -> String {
        file_id.to_string()
    }

    fn image_ref_slot(&self) -> ImageRefSlot {
        ImageRefSlot::FileIdField
    }
}

#[derive(Debug, Clone)]
pub struct OpenAiFilesAdapter {
    client: OpenAiFilesClient,
}

impl OpenAiFilesAdapter {
    pub fn new(client: OpenAiFilesClient) -> Self {
        Self { client }
    }

    pub fn from_provider_context(ctx: FilesApiProviderContext, files_cfg: &LlmFilesConfig) -> Self {
        Self::new(OpenAiFilesClient::from_provider_context(ctx, files_cfg))
    }
}

#[async_trait]
impl FilesApiAdapter for OpenAiFilesClient {
    async fn upload(
        &self,
        purpose: FilePurpose,
        filename: &str,
        mime_type: &str,
        bytes: &[u8],
    ) -> Result<OpenAiFileMeta, AppError> {
        OpenAiFilesClient::upload(self, purpose, filename, mime_type, bytes).await
    }

    async fn delete(&self, file_id: &str) -> Result<(), AppError> {
        OpenAiFilesClient::delete(self, file_id).await
    }

    fn expires_after_seconds(&self) -> u64 {
        OpenAiFilesClient::expires_after_seconds(self)
    }
}

#[async_trait]
impl FilesApiAdapter for OpenAiFilesAdapter {
    async fn upload(
        &self,
        purpose: FilePurpose,
        filename: &str,
        mime_type: &str,
        bytes: &[u8],
    ) -> Result<OpenAiFileMeta, AppError> {
        self.client
            .upload(purpose, filename, mime_type, bytes)
            .await
    }

    async fn delete(&self, file_id: &str) -> Result<(), AppError> {
        self.client.delete(file_id).await
    }

    fn expires_after_seconds(&self) -> u64 {
        self.client.expires_after_seconds()
    }
}

#[derive(Debug, Clone)]
pub struct MoonshotFilesAdapter {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    retry_count: u32,
    expires_after_seconds: u64,
}

impl MoonshotFilesAdapter {
    pub fn from_provider_context(ctx: FilesApiProviderContext, files_cfg: &LlmFilesConfig) -> Self {
        Self {
            client: ctx.client,
            base_url: ctx.base_url.trim_end_matches('/').to_string(),
            api_key: ctx.api_key,
            retry_count: ctx.retry_count,
            expires_after_seconds: files_cfg.expires_after_seconds,
        }
    }

    fn files_base_url(&self) -> String {
        let root = self.base_url.trim_end_matches('/');
        if root.ends_with("/v1") {
            format!("{root}/files")
        } else {
            format!("{root}/v1/files")
        }
    }

    fn purpose_name(purpose: FilePurpose) -> &'static str {
        match purpose {
            FilePurpose::Vision => "image",
            FilePurpose::UserData => "file-extract",
        }
    }

    fn auth_header(&self) -> (&str, String) {
        ("Authorization", format!("Bearer {}", self.api_key))
    }

    async fn upload_once(
        &self,
        purpose: FilePurpose,
        filename: &str,
        mime_type: &str,
        bytes: &[u8],
    ) -> Result<OpenAiFileMeta, AppError> {
        if bytes.is_empty() {
            return Err(AppError::Llm(
                "Moonshot Files upload: 空文件不可上传".to_string(),
            ));
        }
        let part = reqwest::multipart::Part::bytes(bytes.to_vec())
            .file_name(filename.to_string())
            .mime_str(mime_type)
            .map_err(|e| AppError::Llm(format!("Moonshot Files upload: mime 无效: {e}")))?;
        let mut form = reqwest::multipart::Form::new()
            .text("purpose", Self::purpose_name(purpose).to_string())
            .part("file", part);
        if self.expires_after_seconds > 0 {
            form = form.text("expires_after[anchor]", "created_at").text(
                "expires_after[seconds]",
                self.expires_after_seconds.to_string(),
            );
        }
        let (header_key, header_value) = self.auth_header();
        let resp = self
            .client
            .post(self.files_base_url())
            .header(header_key, header_value)
            .multipart(form)
            .send()
            .await
            .map_err(|e| map_send_error("moonshot-files", "upload", e))?;
        let status = resp.status();
        let body = resp
            .bytes()
            .await
            .map_err(|e| map_body_read_error("moonshot-files", "upload", e))?;
        if !status.is_success() {
            return Err(classify_http_error(
                "moonshot-files",
                status,
                &String::from_utf8_lossy(&body),
                "upload",
            ));
        }
        let raw: OpenAiCompatibleFileObject = serde_json::from_slice(&body)
            .map_err(|e| map_parse_error("moonshot-files", "upload", e))?;
        Ok(raw.into_meta())
    }

    async fn delete_once(&self, file_id: &str) -> Result<(), AppError> {
        let (header_key, header_value) = self.auth_header();
        let resp = self
            .client
            .delete(format!("{}/{}", self.files_base_url(), file_id))
            .header(header_key, header_value)
            .send()
            .await
            .map_err(|e| map_send_error("moonshot-files", "delete", e))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        let body = resp
            .bytes()
            .await
            .map_err(|e| map_body_read_error("moonshot-files", "delete", e))?;
        if !status.is_success() {
            return Err(classify_http_error(
                "moonshot-files",
                status,
                &String::from_utf8_lossy(&body),
                "delete",
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl FilesApiAdapter for MoonshotFilesAdapter {
    async fn upload(
        &self,
        purpose: FilePurpose,
        filename: &str,
        mime_type: &str,
        bytes: &[u8],
    ) -> Result<OpenAiFileMeta, AppError> {
        let mut last_err = None;
        for attempt in 0..=self.retry_count {
            match self.upload_once(purpose, filename, mime_type, bytes).await {
                Ok(meta) => return Ok(meta),
                Err(err) if is_retryable_llm_error(&err) && attempt < self.retry_count => {
                    let delay = provider_retry_delay(attempt);
                    warn!(
                        "Moonshot Files upload 失败，{}ms 后重试 ({}/{}): {}",
                        delay.as_millis(),
                        attempt + 1,
                        self.retry_count,
                        err
                    );
                    tokio::time::sleep(delay).await;
                    last_err = Some(err);
                }
                Err(err) => return Err(err),
            }
        }
        Err(last_err.unwrap_or_else(|| AppError::Llm("Moonshot Files upload 重试耗尽".to_string())))
    }

    async fn delete(&self, file_id: &str) -> Result<(), AppError> {
        let mut last_err = None;
        for attempt in 0..=self.retry_count {
            match self.delete_once(file_id).await {
                Ok(()) => return Ok(()),
                Err(err) if is_retryable_llm_error(&err) && attempt < self.retry_count => {
                    let delay = provider_retry_delay(attempt);
                    tokio::time::sleep(delay).await;
                    last_err = Some(err);
                }
                Err(err) => return Err(err),
            }
        }
        Err(last_err.unwrap_or_else(|| AppError::Llm("Moonshot Files delete 重试耗尽".to_string())))
    }

    fn expires_after_seconds(&self) -> u64 {
        self.expires_after_seconds
    }

    fn reference_token(&self, file_id: &str) -> String {
        format!("ms://{file_id}")
    }

    fn image_ref_slot(&self) -> ImageRefSlot {
        ImageRefSlot::UrlField
    }
}

#[derive(Debug, Clone)]
pub struct AnthropicFilesAdapter {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    retry_count: u32,
}

impl AnthropicFilesAdapter {
    pub fn from_provider_context(
        ctx: FilesApiProviderContext,
        _files_cfg: &LlmFilesConfig,
    ) -> Self {
        Self {
            client: ctx.client,
            base_url: ctx.base_url.trim_end_matches('/').to_string(),
            api_key: ctx.api_key,
            retry_count: ctx.retry_count,
        }
    }

    fn files_base_url(&self) -> String {
        let root = self.base_url.trim_end_matches('/');
        if root.ends_with("/v1") {
            format!("{root}/files")
        } else {
            format!("{root}/v1/files")
        }
    }

    async fn upload_once(
        &self,
        filename: &str,
        mime_type: &str,
        bytes: &[u8],
    ) -> Result<OpenAiFileMeta, AppError> {
        if bytes.is_empty() {
            return Err(AppError::Llm(
                "Anthropic Files upload: 空文件不可上传".to_string(),
            ));
        }
        let part = reqwest::multipart::Part::bytes(bytes.to_vec())
            .file_name(filename.to_string())
            .mime_str(mime_type)
            .map_err(|e| AppError::Llm(format!("Anthropic Files upload: mime 无效: {e}")))?;
        let form = reqwest::multipart::Form::new().part("file", part);
        let resp = self
            .client
            .post(self.files_base_url())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", ANTHROPIC_FILES_BETA)
            .multipart(form)
            .send()
            .await
            .map_err(|e| map_send_error("anthropic-files", "upload", e))?;
        let status = resp.status();
        let body = resp
            .bytes()
            .await
            .map_err(|e| map_body_read_error("anthropic-files", "upload", e))?;
        if !status.is_success() {
            return Err(classify_http_error(
                "anthropic-files",
                status,
                &String::from_utf8_lossy(&body),
                "upload",
            ));
        }
        let raw: AnthropicFileObject = serde_json::from_slice(&body)
            .map_err(|e| map_parse_error("anthropic-files", "upload", e))?;
        Ok(raw.into_meta())
    }

    async fn delete_once(&self, file_id: &str) -> Result<(), AppError> {
        let resp = self
            .client
            .delete(format!("{}/{}", self.files_base_url(), file_id))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", ANTHROPIC_FILES_BETA)
            .send()
            .await
            .map_err(|e| map_send_error("anthropic-files", "delete", e))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        let body = resp
            .bytes()
            .await
            .map_err(|e| map_body_read_error("anthropic-files", "delete", e))?;
        if !status.is_success() {
            return Err(classify_http_error(
                "anthropic-files",
                status,
                &String::from_utf8_lossy(&body),
                "delete",
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl FilesApiAdapter for AnthropicFilesAdapter {
    async fn upload(
        &self,
        _purpose: FilePurpose,
        filename: &str,
        mime_type: &str,
        bytes: &[u8],
    ) -> Result<OpenAiFileMeta, AppError> {
        let mut last_err = None;
        for attempt in 0..=self.retry_count {
            match self.upload_once(filename, mime_type, bytes).await {
                Ok(meta) => return Ok(meta),
                Err(err) if is_retryable_llm_error(&err) && attempt < self.retry_count => {
                    let delay = provider_retry_delay(attempt);
                    warn!(
                        "Anthropic Files upload 失败，{}ms 后重试 ({}/{}): {}",
                        delay.as_millis(),
                        attempt + 1,
                        self.retry_count,
                        err
                    );
                    tokio::time::sleep(delay).await;
                    last_err = Some(err);
                }
                Err(err) => return Err(err),
            }
        }
        Err(last_err
            .unwrap_or_else(|| AppError::Llm("Anthropic Files upload 重试耗尽".to_string())))
    }

    async fn delete(&self, file_id: &str) -> Result<(), AppError> {
        let mut last_err = None;
        for attempt in 0..=self.retry_count {
            match self.delete_once(file_id).await {
                Ok(()) => return Ok(()),
                Err(err) if is_retryable_llm_error(&err) && attempt < self.retry_count => {
                    let delay = provider_retry_delay(attempt);
                    tokio::time::sleep(delay).await;
                    last_err = Some(err);
                }
                Err(err) => return Err(err),
            }
        }
        Err(last_err
            .unwrap_or_else(|| AppError::Llm("Anthropic Files delete 重试耗尽".to_string())))
    }

    fn expires_after_seconds(&self) -> u64 {
        0
    }
}

pub fn build_openai_compatible_files_adapter(
    provider: &str,
    ctx: FilesApiProviderContext,
    files_cfg: &LlmFilesConfig,
) -> Arc<dyn FilesApiAdapter> {
    if provider.trim().eq_ignore_ascii_case("moonshot") {
        Arc::new(MoonshotFilesAdapter::from_provider_context(ctx, files_cfg))
    } else {
        Arc::new(OpenAiFilesAdapter::from_provider_context(ctx, files_cfg))
    }
}

fn map_send_error(provider_name: &str, op: &str, err: reqwest::Error) -> AppError {
    if err.is_connect() {
        return llm_error_with_source(
            provider_name,
            LlmErrorStage::Connect,
            format!("{provider_name} {op} 请求连接失败"),
            err,
        );
    }
    if err.is_timeout() {
        return llm_error_with_source(
            provider_name,
            LlmErrorStage::ReadTimeout,
            format!("{provider_name} {op} 读/空闲超时"),
            err,
        );
    }
    llm_error_with_source(
        provider_name,
        LlmErrorStage::Send,
        format!("{provider_name} {op} 请求发送失败"),
        err,
    )
}

fn map_body_read_error(provider_name: &str, op: &str, err: reqwest::Error) -> AppError {
    if err.is_timeout() {
        return llm_error_with_source(
            provider_name,
            LlmErrorStage::ReadTimeout,
            format!("{provider_name} {op} 读响应超时"),
            err,
        );
    }
    llm_error_with_source(
        provider_name,
        LlmErrorStage::BodyRead,
        format!("{provider_name} {op} 读响应失败"),
        err,
    )
}

fn map_parse_error(provider_name: &str, op: &str, err: impl Into<anyhow::Error>) -> AppError {
    llm_error_with_source(
        provider_name,
        LlmErrorStage::Parse,
        format!("{provider_name} {op} 解析响应失败"),
        err,
    )
}

fn classify_http_error(
    provider_name: &str,
    status: reqwest::StatusCode,
    body: &str,
    op: &str,
) -> AppError {
    let lower = body.to_ascii_lowercase();
    if status == reqwest::StatusCode::UNAUTHORIZED
        || lower.contains("invalid_api_key")
        || lower.contains("incorrect_api_key")
    {
        return llm_http_status_error_with_summary(
            provider_name,
            status.as_u16(),
            format!(
                "{provider_name} {op} 失败：API Key 无效（HTTP {}）。",
                status.as_u16()
            ),
        );
    }
    if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE || lower.contains("file_too_large") {
        return llm_http_status_error_with_summary(
            provider_name,
            status.as_u16(),
            format!(
                "{provider_name} {op} 失败：文件超过上游上限（HTTP {}）。",
                status.as_u16()
            ),
        );
    }
    llm_http_status_error_with_summary(
        provider_name,
        status.as_u16(),
        format!(
            "{provider_name} {op} 失败（HTTP {}）：{body}",
            status.as_u16()
        ),
    )
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatibleFileObject {
    id: String,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    bytes: Option<u64>,
    #[serde(default)]
    created_at: Option<i64>,
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    expires_at: Option<i64>,
}

impl OpenAiCompatibleFileObject {
    fn into_meta(self) -> OpenAiFileMeta {
        OpenAiFileMeta {
            id: self.id,
            filename: self.filename.unwrap_or_else(|| "unknown".to_string()),
            bytes: self.bytes.unwrap_or(0),
            created_at: self.created_at.unwrap_or(0),
            purpose: self.purpose,
            expires_at: self.expires_at,
        }
    }
}

#[derive(Debug, Deserialize)]
struct AnthropicFileObject {
    id: String,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    size_bytes: Option<u64>,
    #[serde(default)]
    created_at: Option<String>,
}

impl AnthropicFileObject {
    fn into_meta(self) -> OpenAiFileMeta {
        let created_at = self
            .created_at
            .as_deref()
            .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
            .map(|dt| dt.timestamp())
            .unwrap_or(0);
        OpenAiFileMeta {
            id: self.id,
            filename: self.filename.unwrap_or_else(|| "unknown".to_string()),
            bytes: self.size_bytes.unwrap_or(0),
            created_at,
            purpose: None,
            expires_at: None,
        }
    }
}
