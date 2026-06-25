//! 会话元数据 store（sessions.json）：`sessions{id→entry}` + `current{key→id}` 的读写与持久化。
//!
//! 列表与路由由此提供；原子写通过「写临时文件 → 重命名」保证。

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::infra::error::AppError;
use crate::infra::platform::{read_file_utf8, write_file_atomic};

/// MVP 默认 sessionKey：单 Agent 单入口。
pub const DEFAULT_SESSION_KEY: &str = "agent:main:main";

/// sessions.json 的根类型：会话档案 + 每个 scope 的 current 指针。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionStore {
    #[serde(default)]
    pub sessions: HashMap<String, SessionEntry>,
    #[serde(default)]
    pub current: HashMap<String, String>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty() && self.current.is_empty()
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }
}

/// 会话元数据条目（sessions.json 中每个 sessionId 对应一条）。
/// 与 Architecture session-storage 一致，camelCase 序列化。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    #[serde(default)]
    pub session_key: String,
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
    /// 最近一次会话级 restore 成功落到的 checkpoint（仅 TurnEnd/Interrupt）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checkpoint_id: Option<String>,
    /// 会话标题：首条 user message 首行截断 ≤40 字符生成一次、持久化、永不覆盖。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// 从路径加载 SessionStore；文件不存在或为空时返回空 store。
pub fn load_store(path: &Path) -> Result<SessionStore, AppError> {
    let content = match read_file_utf8(path) {
        Ok(s) => s,
        Err(AppError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            return reset_store(path);
        }
        Err(err) => return Err(err),
    };
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return reset_store(path);
    }
    let mut store: SessionStore = match serde_json::from_str(trimmed) {
        Ok(store) => store,
        Err(err) => {
            warn!(
                path = %path.display(),
                error = %err,
                "session store parse failed; rebuilding empty store"
            );
            return reset_store(path);
        }
    };
    repair_missing_session_keys(&mut store);
    prune_stale_current_pointers(&mut store);
    Ok(store)
}

/// 原子写入 SessionStore 到 path（临时文件 + rename）。
pub fn save_store(path: &Path, store: &SessionStore) -> Result<(), AppError> {
    let content = serde_json::to_string_pretty(store)?;
    write_file_atomic(path, content.as_bytes())
}

fn repair_missing_session_keys(store: &mut SessionStore) {
    let reverse: HashMap<String, String> = store
        .current
        .iter()
        .map(|(session_key, session_id)| (session_id.clone(), session_key.clone()))
        .collect();
    for (session_id, entry) in &mut store.sessions {
        if entry.session_key.trim().is_empty() {
            if let Some(session_key) = reverse.get(session_id) {
                entry.session_key = session_key.clone();
            }
        }
    }
}

fn prune_stale_current_pointers(store: &mut SessionStore) {
    store
        .current
        .retain(|_, session_id| store.sessions.contains_key(session_id));
}

fn reset_store(path: &Path) -> Result<SessionStore, AppError> {
    let store = SessionStore::new();
    save_store(path, &store)?;
    Ok(store)
}
