//! OpenAI Files 上传管理（T2-P0-015）。
//!
//! 本模块提供：
//! 1) 纯 HTTP 客户端 [`OpenAiFilesClient`]（upload/get/delete/list）
//! 2) 会话级 runtime [`OpenAiFilesRuntime`]（双索引 cache + cleanup 注册与回收）
//! 3) 编排辅助：体积分流阈值 [`UploadDecision`]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::core::llm::provider::LlmProvider;
use crate::core::llm::retry_delay::provider_retry_delay;
use crate::infra::config::LlmFilesConfig;
use crate::infra::error::{
    is_retryable_llm_error, llm_error_with_source, llm_http_status_error_with_summary, AppError,
    LlmErrorStage,
};

static NEXT_FILES_CLIENT_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);
const PROVIDER_NAME: &str = "openai-files";

/// `< 1MiB`：默认走 inline（A 通道）。
pub const INLINE_SMALL_BYTES: u64 = 1024 * 1024;
/// `>= 10MiB`：策略层要求必须走 upload（B 通道）。
pub const MUST_UPLOAD_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadDecision {
    InlinePreferred,
    UploadPreferred,
    UploadRequired,
}

/// 与架构文档 §3.3 一致的默认分流阈值。
pub fn upload_decision_by_size(size_bytes: u64) -> UploadDecision {
    if size_bytes < INLINE_SMALL_BYTES {
        UploadDecision::InlinePreferred
    } else if size_bytes < MUST_UPLOAD_BYTES {
        UploadDecision::UploadPreferred
    } else {
        UploadDecision::UploadRequired
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilePurpose {
    Vision,
    UserData,
}

impl FilePurpose {
    fn as_str(self) -> &'static str {
        match self {
            Self::Vision => "vision",
            Self::UserData => "user_data",
        }
    }
}

/// Provider 暴露给 Files 子系统的上下文。
///
/// `reqwest::Client` clone 后共享同一连接池，满足「与 provider 同生命周期/同连接池」要求。
#[derive(Debug, Clone)]
pub struct OpenAiFilesProviderContext {
    pub client: reqwest::Client,
    pub base_url: String,
    pub api_key: String,
    pub retry_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiFileMeta {
    pub id: String,
    pub filename: String,
    pub bytes: u64,
    pub created_at: i64,
    pub purpose: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct OpenAiFilesClient {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    retry_count: u32,
    expires_after_seconds: u64,
    #[cfg_attr(not(test), allow(dead_code))]
    instance_id: u64,
}

impl OpenAiFilesClient {
    pub fn from_provider_context(
        ctx: OpenAiFilesProviderContext,
        files_cfg: &LlmFilesConfig,
    ) -> Self {
        Self {
            client: ctx.client,
            base_url: ctx.base_url.trim_end_matches('/').to_string(),
            api_key: ctx.api_key,
            retry_count: ctx.retry_count,
            expires_after_seconds: files_cfg.expires_after_seconds,
            instance_id: NEXT_FILES_CLIENT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(
        client: reqwest::Client,
        base_url: String,
        api_key: String,
        retry_count: u32,
        expires_after_seconds: u64,
    ) -> Self {
        Self {
            client,
            base_url,
            api_key,
            retry_count,
            expires_after_seconds,
            instance_id: NEXT_FILES_CLIENT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed),
        }
    }

    pub fn expires_after_seconds(&self) -> u64 {
        self.expires_after_seconds
    }

    #[cfg(test)]
    pub(crate) fn instance_id(&self) -> u64 {
        self.instance_id
    }

    fn files_base_url(&self) -> String {
        let root = self.base_url.trim_end_matches('/');
        if root.ends_with("/v1") {
            format!("{root}/files")
        } else {
            format!("{root}/v1/files")
        }
    }

    fn auth_header(&self) -> (&str, String) {
        ("Authorization", format!("Bearer {}", self.api_key))
    }

    fn map_send_error(op: &str, err: reqwest::Error) -> AppError {
        if err.is_connect() {
            return llm_error_with_source(
                PROVIDER_NAME,
                LlmErrorStage::Connect,
                format!("OpenAI Files {op} 请求连接失败"),
                err,
            );
        }
        if err.is_timeout() {
            return llm_error_with_source(
                PROVIDER_NAME,
                LlmErrorStage::ReadTimeout,
                format!("OpenAI Files {op} 读/空闲超时"),
                err,
            );
        }
        llm_error_with_source(
            PROVIDER_NAME,
            LlmErrorStage::Send,
            format!("OpenAI Files {op} 请求发送失败"),
            err,
        )
    }

    fn map_body_read_error(op: &str, err: reqwest::Error) -> AppError {
        if err.is_timeout() {
            return llm_error_with_source(
                PROVIDER_NAME,
                LlmErrorStage::ReadTimeout,
                format!("OpenAI Files {op} 读响应超时"),
                err,
            );
        }
        llm_error_with_source(
            PROVIDER_NAME,
            LlmErrorStage::BodyRead,
            format!("OpenAI Files {op} 读响应失败"),
            err,
        )
    }

    fn map_parse_error(op: &str, err: impl Into<anyhow::Error>) -> AppError {
        llm_error_with_source(
            PROVIDER_NAME,
            LlmErrorStage::Parse,
            format!("OpenAI Files {op} 解析响应失败"),
            err,
        )
    }

    fn classify_http_error(&self, status: reqwest::StatusCode, body: &str, op: &str) -> AppError {
        let lower = body.to_ascii_lowercase();
        let inline_hint =
            "建议改走 inline 通道（image_b64/file_b64）或切换支持 OpenAI Files API 的 provider";
        if status == reqwest::StatusCode::UNAUTHORIZED || lower.contains("invalid_api_key") {
            return llm_http_status_error_with_summary(
                PROVIDER_NAME,
                status.as_u16(),
                format!(
                    "OpenAI Files {op} 失败：API Key 无效（HTTP {}）。{}",
                    status.as_u16(),
                    inline_hint
                ),
            );
        }
        if status == reqwest::StatusCode::FORBIDDEN
            || lower.contains("organization_restricted")
            || lower.contains("project")
        {
            return llm_http_status_error_with_summary(
                PROVIDER_NAME,
                status.as_u16(),
                format!(
                    "OpenAI Files {op} 失败：Project/组织未启用 Files（HTTP {}）。{}",
                    status.as_u16(),
                    inline_hint
                ),
            );
        }
        if status == reqwest::StatusCode::BAD_REQUEST && lower.contains("purpose") {
            return llm_http_status_error_with_summary(
                PROVIDER_NAME,
                status.as_u16(),
                format!(
                    "OpenAI Files {op} 失败：purpose 不被接受（HTTP {}）。{}",
                    status.as_u16(),
                    inline_hint
                ),
            );
        }
        if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE || lower.contains("file_too_large") {
            return llm_http_status_error_with_summary(
                PROVIDER_NAME,
                status.as_u16(),
                format!(
                    "OpenAI Files {op} 失败：文件超过 OpenAI 上限（HTTP {}）。",
                    status.as_u16()
                ),
            );
        }
        llm_http_status_error_with_summary(
            PROVIDER_NAME,
            status.as_u16(),
            format!(
                "OpenAI Files {op} 失败（HTTP {}）：{}",
                status.as_u16(),
                body
            ),
        )
    }

    pub fn is_retriable(err: &AppError) -> bool {
        is_retryable_llm_error(err)
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
                "OpenAI Files upload: 空文件不可上传".to_string(),
            ));
        }

        let part = reqwest::multipart::Part::bytes(bytes.to_vec())
            .file_name(filename.to_string())
            .mime_str(mime_type)
            .map_err(|e| AppError::Llm(format!("OpenAI Files upload: mime 无效: {e}")))?;
        let mut form = reqwest::multipart::Form::new()
            .text("purpose", purpose.as_str().to_string())
            .part("file", part);
        if self.expires_after_seconds > 0 {
            form = form.text("expires_after[anchor]", "created_at").text(
                "expires_after[seconds]",
                self.expires_after_seconds.to_string(),
            );
        }

        let url = self.files_base_url();
        let (header_key, header_value) = self.auth_header();
        let resp = self
            .client
            .post(&url)
            .header(header_key, header_value)
            .multipart(form)
            .send()
            .await
            .map_err(|e| Self::map_send_error("upload", e))?;
        let status = resp.status();
        let body = resp
            .bytes()
            .await
            .map_err(|e| Self::map_body_read_error("upload", e))?;
        if !status.is_success() {
            return Err(self.classify_http_error(
                status,
                &String::from_utf8_lossy(&body),
                "upload",
            ));
        }
        let raw: OpenAiFileObject =
            serde_json::from_slice(&body).map_err(|e| Self::map_parse_error("upload", e))?;
        Ok(raw.into_meta())
    }

    pub async fn upload(
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
                Err(e) => {
                    if Self::is_retriable(&e) && attempt < self.retry_count {
                        let delay = provider_retry_delay(attempt);
                        warn!(
                            "OpenAI Files upload 失败，{}ms 后重试 ({}/{}): {}",
                            delay.as_millis(),
                            attempt + 1,
                            self.retry_count,
                            e
                        );
                        tokio::time::sleep(delay).await;
                        last_err = Some(e);
                    } else {
                        last_err = Some(e);
                        break;
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| AppError::Llm("OpenAI Files upload 重试耗尽".to_string())))
    }

    async fn get_once(&self, file_id: &str) -> Result<Option<OpenAiFileMeta>, AppError> {
        let url = format!("{}/{}", self.files_base_url(), file_id);
        let (header_key, header_value) = self.auth_header();
        let resp = self
            .client
            .get(&url)
            .header(header_key, header_value)
            .send()
            .await
            .map_err(|e| Self::map_send_error("get", e))?;
        let status = resp.status();
        let body = resp
            .bytes()
            .await
            .map_err(|e| Self::map_body_read_error("get", e))?;
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(self.classify_http_error(status, &String::from_utf8_lossy(&body), "get"));
        }
        let raw: OpenAiFileObject =
            serde_json::from_slice(&body).map_err(|e| Self::map_parse_error("get", e))?;
        Ok(Some(raw.into_meta()))
    }

    pub async fn get(&self, file_id: &str) -> Result<Option<OpenAiFileMeta>, AppError> {
        let mut last_err = None;
        for attempt in 0..=self.retry_count {
            match self.get_once(file_id).await {
                Ok(meta) => return Ok(meta),
                Err(e) => {
                    if Self::is_retriable(&e) && attempt < self.retry_count {
                        let delay = provider_retry_delay(attempt);
                        tokio::time::sleep(delay).await;
                        last_err = Some(e);
                    } else {
                        last_err = Some(e);
                        break;
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| AppError::Llm("OpenAI Files get 重试耗尽".to_string())))
    }

    async fn delete_once(&self, file_id: &str) -> Result<(), AppError> {
        let url = format!("{}/{}", self.files_base_url(), file_id);
        let (header_key, header_value) = self.auth_header();
        let resp = self
            .client
            .delete(&url)
            .header(header_key, header_value)
            .send()
            .await
            .map_err(|e| Self::map_send_error("delete", e))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            // 幂等成功：404 视为已删除。
            return Ok(());
        }
        let body = resp
            .bytes()
            .await
            .map_err(|e| Self::map_body_read_error("delete", e))?;
        if !status.is_success() {
            return Err(self.classify_http_error(
                status,
                &String::from_utf8_lossy(&body),
                "delete",
            ));
        }
        Ok(())
    }

    pub async fn delete(&self, file_id: &str) -> Result<(), AppError> {
        let mut last_err = None;
        for attempt in 0..=self.retry_count {
            match self.delete_once(file_id).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if Self::is_retriable(&e) && attempt < self.retry_count {
                        let delay = provider_retry_delay(attempt);
                        tokio::time::sleep(delay).await;
                        last_err = Some(e);
                    } else {
                        last_err = Some(e);
                        break;
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| AppError::Llm("OpenAI Files delete 重试耗尽".to_string())))
    }

    async fn list_once(&self) -> Result<Vec<OpenAiFileMeta>, AppError> {
        let url = self.files_base_url();
        let (header_key, header_value) = self.auth_header();
        let resp = self
            .client
            .get(&url)
            .header(header_key, header_value)
            .send()
            .await
            .map_err(|e| Self::map_send_error("list", e))?;
        let status = resp.status();
        let body = resp
            .bytes()
            .await
            .map_err(|e| Self::map_body_read_error("list", e))?;
        if !status.is_success() {
            return Err(self.classify_http_error(status, &String::from_utf8_lossy(&body), "list"));
        }
        let raw: OpenAiFilesListResponse =
            serde_json::from_slice(&body).map_err(|e| Self::map_parse_error("list", e))?;
        Ok(raw
            .data
            .into_iter()
            .map(OpenAiFileObject::into_meta)
            .collect())
    }

    pub async fn list(
        &self,
        prefix: Option<&str>,
        since: Option<i64>,
    ) -> Result<Vec<OpenAiFileMeta>, AppError> {
        let mut last_err = None;
        for attempt in 0..=self.retry_count {
            match self.list_once().await {
                Ok(mut files) => {
                    if let Some(p) = prefix {
                        files.retain(|f| f.filename.starts_with(p));
                    }
                    if let Some(since_ts) = since {
                        files.retain(|f| f.created_at >= since_ts);
                    }
                    return Ok(files);
                }
                Err(e) => {
                    if Self::is_retriable(&e) && attempt < self.retry_count {
                        let delay = provider_retry_delay(attempt);
                        tokio::time::sleep(delay).await;
                        last_err = Some(e);
                    } else {
                        last_err = Some(e);
                        break;
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| AppError::Llm("OpenAI Files list 重试耗尽".to_string())))
    }
}

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub mtime_ms: i64,
    pub size: u64,
    pub sha256: [u8; 32],
    pub file_id: String,
    pub purpose: FilePurpose,
    pub mime: String,
    pub uploaded_at: SystemTime,
    pub expires_at: Option<SystemTime>,
    pub bytes: Option<u64>,
    pub created_at: Option<i64>,
}

#[derive(Debug, Clone)]
struct HashCacheEntry {
    file_id: String,
    expires_at: Option<SystemTime>,
    bytes: Option<u64>,
    created_at: Option<i64>,
}

type HashCacheKey = ([u8; 32], String, FilePurpose);

#[derive(Debug, Default)]
pub struct FileIdCache {
    pub by_path: DashMap<PathBuf, CacheEntry>,
    by_hash: DashMap<HashCacheKey, HashCacheEntry>,
    inflight: DashMap<HashCacheKey, std::sync::Arc<tokio::sync::Mutex<()>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CleanupRecord {
    pub file_id: String,
    pub bytes: Option<u64>,
    pub created_at: Option<i64>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupSummary {
    pub total: usize,
    pub deleted: usize,
    pub failed: usize,
}

#[derive(Debug)]
pub struct OpenAiFilesRuntime {
    client: OpenAiFilesClient,
    pub cache: FileIdCache,
    session_files: DashMap<String, CleanupRecord>,
    delete_queue: DashMap<String, CleanupRecord>,
    registry_path: PathBuf,
    persist_lock: Mutex<()>,
}

impl OpenAiFilesRuntime {
    pub fn new(client: OpenAiFilesClient, registry_path: PathBuf) -> Self {
        let this = Self {
            client,
            cache: FileIdCache::default(),
            session_files: DashMap::new(),
            delete_queue: DashMap::new(),
            registry_path,
            persist_lock: Mutex::new(()),
        };
        this.load_registry_from_disk();
        this
    }

    pub fn client(&self) -> &OpenAiFilesClient {
        &self.client
    }

    pub fn registry_path_for_session(sessions_dir: &Path, session_key: &str) -> PathBuf {
        let mut safe = String::with_capacity(session_key.len());
        for ch in session_key.chars() {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                safe.push(ch);
            } else {
                safe.push('_');
            }
        }
        sessions_dir.join(format!("openai-files-{safe}.json"))
    }

    fn is_expired(expires_at: Option<SystemTime>, now: SystemTime) -> bool {
        match expires_at {
            Some(deadline) => deadline <= now,
            None => false,
        }
    }

    fn epoch_to_system_time(epoch: i64) -> Option<SystemTime> {
        if epoch < 0 {
            return None;
        }
        Some(UNIX_EPOCH + Duration::from_secs(epoch as u64))
    }

    fn read_bytes_and_sha(path: &Path) -> Result<(Vec<u8>, [u8; 32]), AppError> {
        let bytes = std::fs::read(path).map_err(|e| {
            AppError::Llm(format!(
                "OpenAI Files cache: 读取 {} 失败: {e}",
                path.display()
            ))
        })?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let digest = hasher.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&digest[..]);
        Ok((bytes, hash))
    }

    fn persist_registry_to_disk(&self) {
        let _guard = self.persist_lock.lock();
        let mut merged: HashMap<String, CleanupRecord> = HashMap::new();
        for item in self.session_files.iter() {
            merged.insert(item.key().clone(), item.value().clone());
        }
        for item in self.delete_queue.iter() {
            merged.insert(item.key().clone(), item.value().clone());
        }
        if merged.is_empty() {
            let _ = std::fs::remove_file(&self.registry_path);
            return;
        }
        if let Some(parent) = self.registry_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let payload = PersistedCleanupRegistry {
            files: merged.into_values().collect(),
        };
        match serde_json::to_vec_pretty(&payload) {
            Ok(buf) => {
                if let Err(e) = std::fs::write(&self.registry_path, buf) {
                    warn!(
                        error = %e,
                        path = %self.registry_path.display(),
                        "persist openai files cleanup registry failed"
                    );
                }
            }
            Err(e) => warn!(error = %e, "serialize openai files cleanup registry failed"),
        }
    }

    fn load_registry_from_disk(&self) {
        let Ok(data) = std::fs::read(&self.registry_path) else {
            return;
        };
        let Ok(payload) = serde_json::from_slice::<PersistedCleanupRegistry>(&data) else {
            return;
        };
        for record in payload.files {
            self.session_files.insert(record.file_id.clone(), record);
        }
    }

    fn track_session_file(&self, meta: &OpenAiFileMeta, reason: &str) {
        self.session_files.insert(
            meta.id.clone(),
            CleanupRecord {
                file_id: meta.id.clone(),
                bytes: Some(meta.bytes),
                created_at: Some(meta.created_at),
                reason: reason.to_string(),
            },
        );
        self.persist_registry_to_disk();
    }

    pub fn enqueue_delete(
        &self,
        file_id: String,
        bytes: Option<u64>,
        created_at: Option<i64>,
        reason: &str,
    ) {
        self.delete_queue.insert(
            file_id.clone(),
            CleanupRecord {
                file_id,
                bytes,
                created_at,
                reason: reason.to_string(),
            },
        );
        self.persist_registry_to_disk();
    }

    pub fn pending_cleanup_count(&self) -> usize {
        self.session_files.len() + self.delete_queue.len()
    }

    pub async fn resolve_or_upload_path(
        &self,
        path: &Path,
        mime: &str,
        filename: &str,
        purpose: FilePurpose,
    ) -> Result<OpenAiFileMeta, AppError> {
        let cache_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let meta = std::fs::metadata(path).map_err(|e| {
            AppError::Llm(format!(
                "OpenAI Files cache: 无法 stat 路径 {}: {e}",
                path.display()
            ))
        })?;
        let now = SystemTime::now();
        let mtime_ms = crate::core::tools::pipeline::read_state::metadata_mtime_ms(&meta);
        let size = meta.len();

        if let Some(entry) = self.cache.by_path.get(&cache_path) {
            if entry.mtime_ms == mtime_ms
                && entry.size == size
                && entry.mime == mime
                && entry.purpose == purpose
                && !Self::is_expired(entry.expires_at, now)
            {
                let out = OpenAiFileMeta {
                    id: entry.file_id.clone(),
                    filename: filename.to_string(),
                    bytes: entry.bytes.unwrap_or(size),
                    created_at: entry.created_at.unwrap_or(0),
                    purpose: Some(purpose.as_str().to_string()),
                    expires_at: entry.expires_at.and_then(system_time_to_epoch),
                };
                self.track_session_file(&out, "cache_hit_path");
                return Ok(out);
            }
        }

        let previous_entry = self.cache.by_path.get(&cache_path).map(|v| v.clone());
        let (bytes, sha256) = Self::read_bytes_and_sha(path)?;
        if let Some(prev) = previous_entry {
            if prev.sha256 == sha256
                && prev.mime == mime
                && prev.purpose == purpose
                && !Self::is_expired(prev.expires_at, now)
            {
                let refreshed = CacheEntry {
                    mtime_ms,
                    size,
                    ..prev.clone()
                };
                self.cache
                    .by_path
                    .insert(cache_path.clone(), refreshed.clone());
                let out = OpenAiFileMeta {
                    id: refreshed.file_id,
                    filename: filename.to_string(),
                    bytes: refreshed.bytes.unwrap_or(size),
                    created_at: refreshed.created_at.unwrap_or(0),
                    purpose: Some(purpose.as_str().to_string()),
                    expires_at: refreshed.expires_at.and_then(system_time_to_epoch),
                };
                self.track_session_file(&out, "cache_hit_sha_same_path");
                return Ok(out);
            }
            if prev.sha256 != sha256 && !prev.file_id.is_empty() {
                self.enqueue_delete(prev.file_id, prev.bytes, prev.created_at, "cache_evicted");
            }
        }

        let hash_key = (sha256, mime.to_string(), purpose);
        if let Some(entry) = self.cache.by_hash.get(&hash_key).map(|v| v.clone()) {
            if !Self::is_expired(entry.expires_at, now) {
                let refreshed = CacheEntry {
                    mtime_ms,
                    size,
                    sha256,
                    file_id: entry.file_id.clone(),
                    purpose,
                    mime: mime.to_string(),
                    uploaded_at: now,
                    expires_at: entry.expires_at,
                    bytes: entry.bytes,
                    created_at: entry.created_at,
                };
                self.cache.by_path.insert(cache_path.clone(), refreshed);
                let out = OpenAiFileMeta {
                    id: entry.file_id.clone(),
                    filename: filename.to_string(),
                    bytes: entry.bytes.unwrap_or(size),
                    created_at: entry.created_at.unwrap_or(0),
                    purpose: Some(purpose.as_str().to_string()),
                    expires_at: entry.expires_at.and_then(system_time_to_epoch),
                };
                self.track_session_file(&out, "cache_hit_hash");
                return Ok(out);
            }
            self.cache.by_hash.remove(&hash_key);
        }

        let inflight = self
            .cache
            .inflight
            .entry(hash_key.clone())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _guard = inflight.lock().await;

        let result = if let Some(entry) = self.cache.by_hash.get(&hash_key).map(|v| v.clone()) {
            if !Self::is_expired(entry.expires_at, now) {
                let refreshed = CacheEntry {
                    mtime_ms,
                    size,
                    sha256,
                    file_id: entry.file_id.clone(),
                    purpose,
                    mime: mime.to_string(),
                    uploaded_at: now,
                    expires_at: entry.expires_at,
                    bytes: entry.bytes,
                    created_at: entry.created_at,
                };
                self.cache.by_path.insert(cache_path.clone(), refreshed);
                let out = OpenAiFileMeta {
                    id: entry.file_id.clone(),
                    filename: filename.to_string(),
                    bytes: entry.bytes.unwrap_or(size),
                    created_at: entry.created_at.unwrap_or(0),
                    purpose: Some(purpose.as_str().to_string()),
                    expires_at: entry.expires_at.and_then(system_time_to_epoch),
                };
                self.track_session_file(&out, "cache_hit_hash_after_wait");
                Ok(out)
            } else {
                self.cache.by_hash.remove(&hash_key);
                let uploaded = self.client.upload(purpose, filename, mime, &bytes).await?;
                let expires_at = uploaded
                    .expires_at
                    .and_then(Self::epoch_to_system_time)
                    .or_else(|| {
                        if self.client.expires_after_seconds() == 0 {
                            None
                        } else {
                            Some(
                                SystemTime::now()
                                    + Duration::from_secs(self.client.expires_after_seconds()),
                            )
                        }
                    });
                self.cache.by_hash.insert(
                    hash_key.clone(),
                    HashCacheEntry {
                        file_id: uploaded.id.clone(),
                        expires_at,
                        bytes: Some(uploaded.bytes),
                        created_at: Some(uploaded.created_at),
                    },
                );
                self.cache.by_path.insert(
                    cache_path.clone(),
                    CacheEntry {
                        mtime_ms,
                        size,
                        sha256,
                        file_id: uploaded.id.clone(),
                        purpose,
                        mime: mime.to_string(),
                        uploaded_at: SystemTime::now(),
                        expires_at,
                        bytes: Some(uploaded.bytes),
                        created_at: Some(uploaded.created_at),
                    },
                );
                self.track_session_file(&uploaded, "upload");
                Ok(uploaded)
            }
        } else {
            let uploaded = self.client.upload(purpose, filename, mime, &bytes).await?;
            let expires_at = uploaded
                .expires_at
                .and_then(Self::epoch_to_system_time)
                .or_else(|| {
                    if self.client.expires_after_seconds() == 0 {
                        None
                    } else {
                        Some(
                            SystemTime::now()
                                + Duration::from_secs(self.client.expires_after_seconds()),
                        )
                    }
                });
            self.cache.by_hash.insert(
                hash_key.clone(),
                HashCacheEntry {
                    file_id: uploaded.id.clone(),
                    expires_at,
                    bytes: Some(uploaded.bytes),
                    created_at: Some(uploaded.created_at),
                },
            );
            self.cache.by_path.insert(
                cache_path,
                CacheEntry {
                    mtime_ms,
                    size,
                    sha256,
                    file_id: uploaded.id.clone(),
                    purpose,
                    mime: mime.to_string(),
                    uploaded_at: SystemTime::now(),
                    expires_at,
                    bytes: Some(uploaded.bytes),
                    created_at: Some(uploaded.created_at),
                },
            );
            self.track_session_file(&uploaded, "upload");
            Ok(uploaded)
        };
        self.cache.inflight.remove(&hash_key);
        result
    }

    /// 会话退出（或命令结束）时的 best-effort cleanup。
    pub async fn cleanup_registered_files(&self, reason: &str) -> CleanupSummary {
        let mut merged: HashMap<String, CleanupRecord> = HashMap::new();
        for item in self.session_files.iter() {
            merged.insert(item.key().clone(), item.value().clone());
        }
        for item in self.delete_queue.iter() {
            merged.insert(item.key().clone(), item.value().clone());
        }
        let total = merged.len();
        if total == 0 {
            return CleanupSummary {
                total: 0,
                deleted: 0,
                failed: 0,
            };
        }

        let mut deleted = 0usize;
        let mut failed = 0usize;
        for (file_id, mut record) in merged {
            record.reason = reason.to_string();
            match self.client.delete(&file_id).await {
                Ok(()) => {
                    deleted += 1;
                    self.session_files.remove(&file_id);
                    self.delete_queue.remove(&file_id);
                }
                Err(e) => {
                    failed += 1;
                    warn!(
                        file_id = %record.file_id,
                        error = %e,
                        "cleanup openai file failed"
                    );
                }
            }
        }
        self.persist_registry_to_disk();
        CleanupSummary {
            total,
            deleted,
            failed,
        }
    }
}

/// 根据当前 provider 能力构造会话级 runtime（不支持时返回 `None`）。
pub fn build_runtime_for_provider(
    provider: &dyn LlmProvider,
    files_cfg: &LlmFilesConfig,
    sessions_dir: &Path,
    session_key: &str,
) -> Option<OpenAiFilesRuntime> {
    if !provider.supports_openai_files_api() {
        return None;
    }
    let client = provider.openai_files_client(files_cfg)?;
    let registry_path = OpenAiFilesRuntime::registry_path_for_session(sessions_dir, session_key);
    Some(OpenAiFilesRuntime::new(client, registry_path))
}

fn system_time_to_epoch(ts: SystemTime) -> Option<i64> {
    ts.duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

#[derive(Debug, Deserialize)]
struct OpenAiFileObject {
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

impl OpenAiFileObject {
    fn into_meta(self) -> OpenAiFileMeta {
        OpenAiFileMeta {
            filename: self.filename.unwrap_or_else(|| "unknown".to_string()),
            bytes: self.bytes.unwrap_or(0),
            created_at: self.created_at.unwrap_or(0),
            purpose: self.purpose,
            expires_at: self.expires_at,
            id: self.id,
        }
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiFilesListResponse {
    data: Vec<OpenAiFileObject>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedCleanupRegistry {
    files: Vec<CleanupRecord>,
}

#[cfg(test)]
#[path = "tests/openai_files_test.rs"]
mod tests;
