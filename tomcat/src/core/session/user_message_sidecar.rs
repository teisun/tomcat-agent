//! # 当前 session 的真实用户输入侧车
//!
//! transcript 是唯一事实源；本模块只在 compaction 需要生成摘要时，从 transcript 派生一个
//! 可供模型按需读取的 JSONL 文件。它绝不在用户发送消息的热路径上写入，也不维护第二份
//! `superseded` 状态。

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use std::sync::{Arc, OnceLock};

use dashmap::DashMap;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

use crate::core::llm::MessageKind;
use crate::core::session::transcript::{read_entries_tail, MessageEntry, TranscriptEntry};
use crate::infra::error::AppError;
use crate::infra::platform::write_file_atomic_with;

const SIDECAR_TYPE: &str = "tomcat_user_messages";
const SIDECAR_MESSAGE_TYPE: &str = "message";
const SIDECAR_SCHEMA_VERSION: u32 = 2;
const STABLE_REBUILD_ATTEMPTS: usize = 2;
/// `write_file_atomic_with` 使用固定临时文件名；同一进程中同一路径并发重建会争抢该文件。
/// 侧车每个路径一把锁，锁等待发生在 `spawn_blocking` 线程而不影响 Tokio worker。
static REBUILD_LOCKS: OnceLock<DashMap<PathBuf, Arc<Mutex<()>>>> = OnceLock::new();

fn rebuild_lock(path: &Path) -> Arc<Mutex<()>> {
    REBUILD_LOCKS
        .get_or_init(DashMap::new)
        .entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// 用户输入侧车路径与 transcript 同目录、同 session id。
pub(crate) fn user_message_sidecar_path(transcript_path: &Path) -> PathBuf {
    let stem = transcript_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("session");
    transcript_path.with_file_name(format!("{stem}.user_messages.jsonl"))
}

/// 确保 sidecar 对应当前 transcript。
///
/// 命中 header 中的 fingerprint 时不改写文件；否则流式重建。若 transcript 在扫描期间发生
/// 变化，会最多重试一次，以免写出声称“最新”却漏掉并发追加消息的副本。
pub(crate) fn ensure_user_message_sidecar(transcript_path: &Path) -> Result<PathBuf, AppError> {
    if transcript_path.as_os_str().is_empty() {
        return Err(AppError::Config("sidecar 缺少 transcript 路径".to_string()));
    }

    let sidecar_path = user_message_sidecar_path(transcript_path);
    let lock = rebuild_lock(&sidecar_path);
    let _guard = lock.lock();
    for attempt in 0..STABLE_REBUILD_ATTEMPTS {
        let expected = transcript_fingerprint(transcript_path)?;
        if read_sidecar_fingerprint(&sidecar_path)?.as_ref() == Some(&expected) {
            return Ok(sidecar_path);
        }

        rebuild_sidecar(transcript_path, &sidecar_path, &expected)?;
        if transcript_fingerprint(transcript_path)? == expected {
            return Ok(sidecar_path);
        }

        warn!(
            transcript = %transcript_path.display(),
            attempt = attempt + 1,
            "transcript changed while rebuilding user-message sidecar; retrying"
        );
    }

    Err(AppError::Config(
        "transcript 在 sidecar 重建期间持续变化，无法得到一致快照".to_string(),
    ))
}

/// 在 blocking 线程池中执行 sidecar 检查/重建，避免 JSONL 扫描占用 Tokio worker 或发送热路径。
///
/// sidecar 只是“按需读取”的增强资料；任意 I/O 或任务错误都降级为 `None`，调用者仍须正常
/// 完成摘要生成，只是不向机器区写入一个可能不存在的路径。
pub(crate) async fn ensure_user_message_sidecar_current(transcript_path: &Path) -> Option<PathBuf> {
    if transcript_path.as_os_str().is_empty() {
        return None;
    }
    let transcript_path = transcript_path.to_path_buf();
    let transcript_display = transcript_path.display().to_string();
    match tokio::task::spawn_blocking(move || ensure_user_message_sidecar(&transcript_path)).await {
        Ok(Ok(sidecar_path)) => Some(sidecar_path),
        Ok(Err(error)) => {
            warn!(transcript = %transcript_display, error = %error, "重建 user-message sidecar 失败，摘要将省略路径提示");
            None
        }
        Err(error) => {
            warn!(transcript = %transcript_display, error = %error, "user-message sidecar blocking 任务失败，摘要将省略路径提示");
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptFingerprint {
    transcript_size: u64,
    /// JSON integers are limited to `u64`; Unix milliseconds are safely within that range.
    transcript_mtime_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_entry_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SidecarContentIntegrity {
    message_count: u64,
    #[serde(default)]
    messages_sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SidecarHeader {
    #[serde(rename = "type")]
    entry_type: String,
    schema_version: u32,
    #[serde(flatten)]
    fingerprint: TranscriptFingerprint,
    #[serde(default)]
    integrity: SidecarContentIntegrity,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SidecarMessage<'a> {
    #[serde(rename = "type")]
    entry_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a String>,
    timestamp: &'a str,
    message: &'a Value,
}

fn transcript_fingerprint(transcript_path: &Path) -> Result<TranscriptFingerprint, AppError> {
    let metadata = fs::metadata(transcript_path).map_err(AppError::Io)?;
    let modified = u64::try_from(
        metadata
            .modified()
            .map_err(AppError::Io)?
            .duration_since(UNIX_EPOCH)
            .map_err(|error| {
                AppError::Config(format!("transcript mtime 早于 Unix epoch: {error}"))
            })?
            .as_millis(),
    )
    .map_err(|error| {
        AppError::Config(format!("transcript mtime milliseconds exceed u64: {error}"))
    })?;
    let last_entry_id = read_entries_tail(transcript_path, 1)?
        .last()
        .and_then(transcript_entry_id)
        .map(str::to_owned);

    Ok(TranscriptFingerprint {
        transcript_size: metadata.len(),
        transcript_mtime_ms: modified,
        last_entry_id,
    })
}

fn transcript_entry_id(entry: &TranscriptEntry) -> Option<&str> {
    match entry {
        TranscriptEntry::Message(entry) => entry.id.as_deref(),
        TranscriptEntry::Error(entry) => entry.id.as_deref(),
        TranscriptEntry::ModelChange(entry) => entry.id.as_deref(),
        TranscriptEntry::ThinkingLevelChange(entry) => entry.id.as_deref(),
        TranscriptEntry::ThinkingTrace(entry) => entry.id.as_deref(),
        TranscriptEntry::BranchSummary(entry) => entry.id.as_deref(),
        TranscriptEntry::ToolResultsCompacted(entry) => entry.id.as_deref(),
        TranscriptEntry::Label(entry) => entry.id.as_deref(),
        TranscriptEntry::SessionInfo(entry) => entry.id.as_deref(),
        TranscriptEntry::Custom(entry) => entry.id.as_deref(),
    }
}

fn read_sidecar_fingerprint(path: &Path) -> Result<Option<TranscriptFingerprint>, AppError> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(AppError::Io(error)),
    };
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    if reader.read_line(&mut first_line).map_err(AppError::Io)? == 0 {
        warn!(path = %path.display(), "user-message sidecar is empty; rebuilding");
        return Ok(None);
    }
    let header = match serde_json::from_str::<SidecarHeader>(first_line.trim()) {
        Ok(header)
            if header.entry_type == SIDECAR_TYPE
                && header.schema_version == SIDECAR_SCHEMA_VERSION =>
        {
            header
        }
        Ok(_) => {
            warn!(path = %path.display(), "user-message sidecar schema is unsupported; rebuilding");
            return Ok(None);
        }
        Err(error) => {
            warn!(path = %path.display(), error = %error, "user-message sidecar header is corrupt; rebuilding");
            return Ok(None);
        }
    };
    match validate_sidecar_records(&mut reader)? {
        Some(integrity) if integrity == header.integrity => Ok(Some(header.fingerprint)),
        Some(_) => {
            warn!(path = %path.display(), "user-message sidecar content checksum mismatches; rebuilding");
            Ok(None)
        }
        None => {
            warn!(path = %path.display(), "user-message sidecar records are corrupt; rebuilding");
            Ok(None)
        }
    }
}

fn rebuild_sidecar(
    transcript_path: &Path,
    sidecar_path: &Path,
    fingerprint: &TranscriptFingerprint,
) -> Result<(), AppError> {
    // 先流式计算完整性，再流式写出；不把任何用户历史累积进内存。
    let integrity = collect_transcript_integrity(transcript_path)?;
    let file = fs::File::open(transcript_path).map_err(AppError::Io)?;
    let mut reader = BufReader::new(file);
    let mut transcript_header = String::new();
    reader
        .read_line(&mut transcript_header)
        .map_err(AppError::Io)?;

    write_file_atomic_with(sidecar_path, |writer| {
        write_json_line(
            writer,
            &SidecarHeader {
                entry_type: SIDECAR_TYPE.to_string(),
                schema_version: SIDECAR_SCHEMA_VERSION,
                fingerprint: fingerprint.clone(),
                integrity,
            },
        )?;

        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).map_err(AppError::Io)? == 0 {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<TranscriptEntry>(trimmed) {
                Ok(TranscriptEntry::Message(entry)) if is_active_normal_user(&entry) => {
                    let bytes = serialize_sidecar_message(&entry)?;
                    writer.write_all(&bytes).map_err(AppError::Io)?;
                    writer.write_all(b"\n").map_err(AppError::Io)?;
                }
                Ok(_) => {}
                Err(error) => warn!(
                    transcript = %transcript_path.display(),
                    error = %error,
                    "skipping corrupt transcript line while rebuilding user-message sidecar"
                ),
            }
        }
        Ok(())
    })
}

#[derive(Deserialize)]
struct SidecarMessageRecord {
    #[serde(rename = "type")]
    entry_type: String,
    timestamp: String,
    message: Value,
}

fn validate_sidecar_records(
    reader: &mut impl BufRead,
) -> Result<Option<SidecarContentIntegrity>, AppError> {
    let mut hasher = Sha256::new();
    let mut message_count = 0_u64;
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).map_err(AppError::Io)? == 0 {
            break;
        }
        if line.last() != Some(&b'\n') {
            return Ok(None);
        }
        line.pop();
        let record = match serde_json::from_slice::<SidecarMessageRecord>(&line) {
            Ok(record) => record,
            Err(_) => return Ok(None),
        };
        if record.entry_type != SIDECAR_MESSAGE_TYPE
            || record.timestamp.is_empty()
            || !record.message.is_object()
        {
            return Ok(None);
        }
        hasher.update(&line);
        hasher.update(b"\n");
        message_count += 1;
    }
    Ok(Some(SidecarContentIntegrity {
        message_count,
        messages_sha256: format!("{:x}", hasher.finalize()),
    }))
}

fn collect_transcript_integrity(
    transcript_path: &Path,
) -> Result<SidecarContentIntegrity, AppError> {
    let file = fs::File::open(transcript_path).map_err(AppError::Io)?;
    let mut reader = BufReader::new(file);
    let mut transcript_header = String::new();
    reader
        .read_line(&mut transcript_header)
        .map_err(AppError::Io)?;
    let mut hasher = Sha256::new();
    let mut message_count = 0_u64;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).map_err(AppError::Io)? == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<TranscriptEntry>(trimmed) {
            Ok(TranscriptEntry::Message(entry)) if is_active_normal_user(&entry) => {
                let bytes = serialize_sidecar_message(&entry)?;
                hasher.update(&bytes);
                hasher.update(b"\n");
                message_count += 1;
            }
            Ok(_) => {}
            Err(error) => warn!(
                transcript = %transcript_path.display(),
                error = %error,
                "skipping corrupt transcript line while calculating user-message sidecar integrity"
            ),
        }
    }
    Ok(SidecarContentIntegrity {
        message_count,
        messages_sha256: format!("{:x}", hasher.finalize()),
    })
}

fn serialize_sidecar_message(entry: &MessageEntry) -> Result<Vec<u8>, AppError> {
    Ok(serde_json::to_vec(&SidecarMessage {
        entry_type: SIDECAR_MESSAGE_TYPE,
        id: entry.id.as_ref(),
        timestamp: &entry.timestamp,
        message: &entry.message,
    })?)
}

fn write_json_line<T: Serialize>(writer: &mut impl Write, value: &T) -> Result<(), AppError> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n").map_err(AppError::Io)
}

fn is_active_normal_user(entry: &MessageEntry) -> bool {
    let message = &entry.message;
    message.get("role").and_then(Value::as_str) == Some("user")
        && message.get("superseded").and_then(Value::as_bool) != Some(true)
        && MessageKind::from_persisted(message.get("kind").and_then(Value::as_str)).is_normal()
}

#[cfg(test)]
#[path = "tests/user_message_sidecar_test.rs"]
mod tests;
