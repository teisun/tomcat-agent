//! SessionManager struct and its implementation.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use chrono::Utc;

use crate::core::session::append_message_chain::{
    collect_recent_chat_messages_from_tail, validate_append_message,
};
use crate::core::session::store::{
    load_store, save_store, SessionEntry, SessionStore, DEFAULT_SESSION_KEY,
};
use crate::core::session::transcript::{
    append_entry, get_branch, get_children, get_entry, get_leaf_entry, read_entries_tail,
    read_header, write_header, BranchSummaryEntry, CustomEntry, LabelEntry, MessageEntry,
    ModelChangeEntry, SessionHeader, SessionInfoEntry, ThinkingLevelChangeEntry,
    ThinkingTraceEntry, TranscriptEntry,
};
use crate::infra::error::AppError;
use crate::infra::platform::normalize_path;

use super::types::ContextState;

static APPEND_SEQ: AtomicU64 = AtomicU64::new(0);
const VALIDATE_TAIL_CAP: usize = 64;
const SESSIONS_FILE: &str = "sessions.json";

/// 会话管理器：持有 store 路径与写入锁，提供 CRUD 与 transcript 读写。
pub struct SessionManager {
    /// 会话根目录（已展开 ~）
    sessions_dir: PathBuf,
    /// sessions.json 完整路径
    store_path: PathBuf,
    /// 序列化 store 写入，禁止锁文件
    write_mutex: Mutex<()>,
}

impl SessionManager {
    /// 从已展开的 sessions_dir 路径创建；内部会使用 sessions_dir/sessions.json。
    pub fn new(sessions_dir: PathBuf) -> Self {
        let store_path = sessions_dir.join(SESSIONS_FILE);
        Self {
            sessions_dir: sessions_dir.clone(),
            store_path,
            write_mutex: Mutex::new(()),
        }
    }

    /// 从配置中的 sessions_dir 字符串创建（会做 normalize_path）。
    pub fn from_sessions_dir(sessions_dir: &str) -> Result<Self, AppError> {
        let path = normalize_path(sessions_dir)?;
        Ok(Self::new(path))
    }

    pub fn store_path(&self) -> &Path {
        &self.store_path
    }

    pub fn sessions_dir(&self) -> &Path {
        &self.sessions_dir
    }

    /// 加载当前 store；文件不存在或空则返回空 map。
    pub fn load_store(&self) -> Result<SessionStore, AppError> {
        load_store(&self.store_path)
    }

    /// 原子写 store；内部用 Mutex 序列化。
    fn save_store(&self, store: &SessionStore) -> Result<(), AppError> {
        let _guard = self
            .write_mutex
            .lock()
            .map_err(|e| AppError::Config(format!("session store 写入锁异常: {}", e)))?;
        save_store(&self.store_path, store)
    }

    /// 当前会话 key；MVP 固定为 DEFAULT_SESSION_KEY。
    pub fn current_session_key(&self) -> &'static str {
        DEFAULT_SESSION_KEY
    }

    /// 获取某 sessionKey 的 transcript 文件路径（基于 session_id）。
    pub fn transcript_path(&self, session_id: &str) -> PathBuf {
        self.sessions_dir.join(format!("{}.jsonl", session_id))
    }

    /// 创建新会话：生成 session_id，写入 store 并创建空 transcript 文件。
    pub fn create_session(
        &self,
        session_key: &str,
        cwd: Option<String>,
    ) -> Result<SessionEntry, AppError> {
        let now = Utc::now().timestamp_millis();
        let session_id = format!("{}_{}", now, uuid_for_session());
        let path = self.transcript_path(&session_id);
        let header = SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: session_id.clone(),
            timestamp: iso_ts(now),
            cwd: cwd.clone(),
        };
        write_header(&path, &header)?;
        let entry = SessionEntry {
            session_id: session_id.clone(),
            updated_at: now,
            session_file: Some(path.to_string_lossy().to_string()),
            cwd,
            thinking_level: None,
            model_override: None,
            input_tokens: None,
            output_tokens: None,
            compaction_count: None,
            compaction_tokens_freed: None,
            tool_result_chars_persisted: None,
            last_checkpoint_id: None,
        };
        let mut store = self.load_store()?;
        store.insert(session_key.to_string(), entry.clone());
        self.save_store(&store)?;
        Ok(entry)
    }

    /// 按 sessionKey 获取元数据。
    pub fn get_session(&self, session_key: &str) -> Result<Option<SessionEntry>, AppError> {
        let store = self.load_store()?;
        Ok(store.get(session_key).cloned())
    }

    /// 列出所有 sessionKey 及其条目（来自 sessions.json）。
    pub fn list_sessions(&self) -> Result<Vec<(String, SessionEntry)>, AppError> {
        let store = self.load_store()?;
        Ok(store.into_iter().collect())
    }

    /// 更新某 sessionKey 的元数据（如 model_override、cwd）。
    pub fn update_session(
        &self,
        session_key: &str,
        f: impl FnOnce(&mut SessionEntry),
    ) -> Result<(), AppError> {
        let mut store = self.load_store()?;
        if let Some(entry) = store.get_mut(session_key) {
            entry.updated_at = Utc::now().timestamp_millis();
            f(entry);
        }
        self.save_store(&store)
    }

    /// 将 `ContextState` 中会话级可观测累计写入 `sessions.json`（user turn 末节流刷盘）。
    pub fn persist_context_observability(&self, state: &ContextState) -> Result<(), AppError> {
        let key = self.current_session_key();
        self.update_session(key, |e| {
            e.compaction_count = Some(state.session_obs.compaction_count);
            e.compaction_tokens_freed = Some(state.session_obs.compaction_tokens_freed as u64);
            e.tool_result_chars_persisted =
                Some(state.session_obs.tool_result_chars_persisted as u64);
        })
    }

    /// 删除会话：从 store 移除并删除 transcript 文件（若存在）。
    pub fn delete_session(&self, session_key: &str) -> Result<(), AppError> {
        let mut store = self.load_store()?;
        let entry = store.remove(session_key);
        self.save_store(&store)?;
        if let Some(e) = entry {
            let path = self.transcript_path(&e.session_id);
            let _ = std::fs::remove_file(&path);
        }
        Ok(())
    }

    /// 归档：仅从 store 移除当前会话的 key 指向，不删文件（可选：移动文件到 archive 子目录，MVP 仅移除）。
    pub fn archive_session(&self, session_key: &str) -> Result<(), AppError> {
        let mut store = self.load_store()?;
        store.remove(session_key);
        self.save_store(&store)
    }

    /// 获取当前会话的 transcript 路径；无当前会话返回 None。
    pub fn current_transcript_path(&self) -> Result<Option<PathBuf>, AppError> {
        let store = self.load_store()?;
        let key = self.current_session_key();
        Ok(store.get(key).map(|e| self.transcript_path(&e.session_id)))
    }

    // TODO: 并发 append 存在 TOCTOU 竞态，当前假设单线程串行调用；后续若引入并发需加文件锁或 Mutex
    /// 追加 message 到当前会话的 transcript（核心路径：校验失败 panic!）。
    /// 返回新行的 `MessageEntry.id`（§5.7 MessageId）。
    pub fn append_message(&self, message: serde_json::Value) -> Result<String, AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        let recent = read_entries_tail(&path, VALIDATE_TAIL_CAP).unwrap_or_default();
        let recent_msgs = collect_recent_chat_messages_from_tail(&recent);
        if let Err(reason) = validate_append_message(&message, &recent_msgs) {
            panic!("[append_message] chain violation: {reason}");
        }
        let id = generate_entry_id();
        let now = iso_ts_now()?;
        let entry = TranscriptEntry::Message(MessageEntry {
            id: Some(id.clone()),
            parent_id: None,
            timestamp: now,
            message,
        });
        append_entry(&path, &entry)?;
        Ok(id)
    }

    /// 追加 message（dispatcher/插件路径：校验失败返回 Err 而非 panic）。
    /// 返回新行的 `MessageEntry.id`（§5.7 MessageId）。
    pub fn try_append_message(&self, message: serde_json::Value) -> Result<String, AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        let recent = read_entries_tail(&path, VALIDATE_TAIL_CAP).unwrap_or_default();
        let recent_msgs = collect_recent_chat_messages_from_tail(&recent);
        if let Err(reason) = validate_append_message(&message, &recent_msgs) {
            return Err(AppError::Config(format!("chain violation: {reason}")));
        }
        let id = generate_entry_id();
        let now = iso_ts_now()?;
        let entry = TranscriptEntry::Message(MessageEntry {
            id: Some(id.clone()),
            parent_id: None,
            timestamp: now,
            message,
        });
        append_entry(&path, &entry)?;
        Ok(id)
    }

    /// 追加 thinking_level_change。
    pub fn append_thinking_level_change(&self, thinking_level: &str) -> Result<(), AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        let entry = TranscriptEntry::ThinkingLevelChange(ThinkingLevelChangeEntry {
            id: None,
            parent_id: None,
            timestamp: iso_ts_now()?,
            thinking_level: Some(thinking_level.to_string()),
        });
        append_entry(&path, &entry)
    }

    /// 追加 `thinking_trace`（`llm.thinking.persist=true` 时由 chat 层调用）。
    ///
    /// 注意：该条目仅用于调试 / 审计回放，不参与 `init_context_state` hydrate；
    /// 上行 messages 仍保持 `build_context_from_state -> messages.clone()` 不变。
    pub fn append_thinking_trace(
        &self,
        text: &str,
        signature: Option<&str>,
    ) -> Result<(), AppError> {
        if text.is_empty() {
            return Ok(());
        }
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        let entry = TranscriptEntry::ThinkingTrace(ThinkingTraceEntry {
            id: None,
            parent_id: None,
            timestamp: iso_ts_now()?,
            text: text.to_string(),
            signature: signature.map(str::to_string),
        });
        append_entry(&path, &entry)
    }

    /// 追加自定义 transcript 事件（如 checkpoint.restore）。
    pub fn append_custom_entry(&self, extra: serde_json::Value) -> Result<(), AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        let entry = TranscriptEntry::Custom(CustomEntry {
            id: Some(generate_entry_id()),
            parent_id: None,
            timestamp: iso_ts_now()?,
            extra,
        });
        append_entry(&path, &entry)
    }

    /// 追加 model_change。
    pub fn append_model_change(
        &self,
        provider: Option<&str>,
        model_id: Option<&str>,
    ) -> Result<(), AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        let entry = TranscriptEntry::ModelChange(ModelChangeEntry {
            id: None,
            parent_id: None,
            timestamp: iso_ts_now()?,
            provider: provider.map(String::from),
            model_id: model_id.map(String::from),
        });
        append_entry(&path, &entry)
    }

    /// 追加 compaction（基本版，不含覆盖范围信息）。
    pub fn append_compaction(&self, summary: Option<&str>) -> Result<(), AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        let entry = TranscriptEntry::BranchSummary(BranchSummaryEntry {
            id: None,
            parent_id: None,
            timestamp: iso_ts_now()?,
            summary: summary.map(String::from),
            covered_start_id: None,
            covered_end_id: None,
            covered_count: None,
            is_boundary: None,
            preheat_compaction_id: None,
            estimated_covered_tokens_before: None,
            estimated_summary_tokens: None,
            estimated_tokens_saved: None,
            error: None,
            attempts: None,
        });
        append_entry(&path, &entry)
    }

    /// 追加含覆盖范围的 compaction entry。
    pub fn append_compaction_with_range(
        &self,
        summary: &str,
        covered_start_id: Option<String>,
        covered_end_id: Option<String>,
        covered_count: usize,
    ) -> Result<(), AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        let entry = TranscriptEntry::BranchSummary(BranchSummaryEntry {
            id: None,
            parent_id: None,
            timestamp: iso_ts_now()?,
            summary: Some(summary.to_string()),
            covered_start_id,
            covered_end_id,
            covered_count: Some(covered_count),
            is_boundary: None,
            preheat_compaction_id: None,
            estimated_covered_tokens_before: None,
            estimated_summary_tokens: None,
            estimated_tokens_saved: None,
            error: None,
            attempts: None,
        });
        append_entry(&path, &entry)
    }

    /// 追加 session_info（如会话名称）。
    pub fn append_session_info(&self, name: &str) -> Result<(), AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        let entry = TranscriptEntry::SessionInfo(SessionInfoEntry {
            id: None,
            parent_id: None,
            timestamp: iso_ts_now()?,
            name: Some(name.to_string()),
        });
        append_entry(&path, &entry)
    }

    /// 追加 label。
    pub fn append_label_change(&self, label: &str) -> Result<(), AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        let entry = TranscriptEntry::Label(LabelEntry {
            id: None,
            parent_id: None,
            timestamp: iso_ts_now()?,
            label: Some(label.to_string()),
        });
        append_entry(&path, &entry)
    }

    /// 获取当前会话 transcript 中最近 cap 条 entry。
    pub fn get_entries(&self, cap: usize) -> Result<Vec<TranscriptEntry>, AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        read_entries_tail(&path, cap)
    }

    /// get_entry 代理到当前会话 transcript。
    pub fn get_entry(&self, id: &str) -> Result<Option<TranscriptEntry>, AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        get_entry(&path, id)
    }

    /// get_children 代理。
    pub fn get_children(
        &self,
        parent_id: &str,
        cap: usize,
    ) -> Result<Vec<TranscriptEntry>, AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        get_children(&path, parent_id, cap)
    }

    /// get_leaf_entry 代理。
    pub fn get_leaf_entry(&self) -> Result<Option<TranscriptEntry>, AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        get_leaf_entry(&path)
    }

    /// get_branch 代理。
    pub fn get_branch(&self, leaf_id: &str) -> Result<Vec<TranscriptEntry>, AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        get_branch(&path, leaf_id, super::BRANCH_MAX_ENTRIES)
    }

    /// 读取当前会话 transcript 的 header。
    pub fn read_session_header(&self) -> Result<Option<SessionHeader>, AppError> {
        let path = match self.current_transcript_path()? {
            Some(p) => p,
            None => return Ok(None),
        };
        read_header(&path).map(Some)
    }
}

fn uuid_for_session() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut h);
    format!("{:016x}", h.finish())
}

fn iso_ts(ms: i64) -> String {
    let secs = ms / 1000;
    let nsecs = (ms % 1000).unsigned_abs() as u32 * 1_000_000;
    let dt = chrono::DateTime::from_timestamp(secs, nsecs).unwrap_or_else(Utc::now);
    dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn iso_ts_now() -> Result<String, AppError> {
    Ok(Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

pub fn generate_entry_id() -> String {
    let micros = Utc::now().timestamp_micros();
    let seq = APPEND_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{micros}_{seq}")
}
