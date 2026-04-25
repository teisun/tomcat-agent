//! 会话元数据 store（sessions.json）：sessionKey → SessionEntry 的读写与持久化。
//!
//! 列表与路由由此提供；原子写通过「写临时文件 → 重命名」保证。

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::infra::error::AppError;
use crate::infra::platform::{read_file_utf8, write_file_atomic};

/// MVP 默认 sessionKey：单 Agent 单入口。
pub const DEFAULT_SESSION_KEY: &str = "agent:main:main";

/// sessions.json 的根类型：sessionKey → 元数据条目。
pub type SessionStore = HashMap<String, SessionEntry>;

/// 会话元数据条目（sessions.json 中每个 sessionKey 对应一条）。
/// 与 Architecture session-storage 一致，camelCase 序列化。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    /// 当前 transcript 文件 id，对应 `<sessionId>.jsonl`
    pub session_id: String,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction_count: Option<u32>,
    /// 与会话 `ContextState.session_obs.compaction_tokens_freed` 同步（估算 tok 累计）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction_tokens_freed: Option<u64>,
    /// L0 落盘原始字符累计（Unicode），与 `ContextState.session_obs.tool_result_chars_persisted` 同步。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result_chars_persisted: Option<u64>,
}

/// 从路径加载 SessionStore；文件不存在或为空时返回空 HashMap。
pub fn load_store(path: &Path) -> Result<SessionStore, AppError> {
    let content = match read_file_utf8(path) {
        Ok(s) => s,
        Err(_) => return Ok(SessionStore::new()),
    };
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(SessionStore::new());
    }
    let store: SessionStore = serde_json::from_str(trimmed)?;
    Ok(store)
}

/// 原子写入 SessionStore 到 path（临时文件 + rename）。
pub fn save_store(path: &Path, store: &SessionStore) -> Result<(), AppError> {
    let content = serde_json::to_string_pretty(store)?;
    write_file_atomic(path, content.as_bytes())
}
