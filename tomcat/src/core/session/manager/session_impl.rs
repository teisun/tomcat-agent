//! SessionManager struct and its implementation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use chrono::Utc;

use crate::core::session::append_message_chain::{
    collect_recent_chat_messages_from_tail, validate_append_message,
};
use crate::core::session::resume_index::remove_resume_index;
use crate::core::session::store::{
    load_store, save_store, SessionEntry, SessionStore, DEFAULT_SESSION_KEY,
};
use crate::core::session::transcript::{
    append_entry, append_entry_with_sync, get_branch, get_children, get_entry, get_leaf_entry,
    mark_message_entries_after_anchor_superseded, read_entries_tail, read_entries_tail_before,
    read_header, rewrite_message_summary_titles_by_id, write_header, BranchSummaryEntry,
    CustomEntry, LabelEntry, MessageEntry, MessageSummaryTitleRewrite, ModelChangeEntry,
    SessionHeader, SessionInfoEntry, SyncLevel, ThinkingLevelChangeEntry, ThinkingTraceEntry,
    TranscriptEntry, TranscriptPage,
};
use crate::infra::error::AppError;
use crate::infra::platform::normalize_path;

use super::types::ContextState;
use super::MessageAppendSink;

static APPEND_SEQ: AtomicU64 = AtomicU64::new(0);
static SESSION_ID_SEQ: AtomicU64 = AtomicU64::new(0);
const VALIDATE_TAIL_CAP: usize = 64;
const SESSIONS_FILE: &str = "sessions.json";
const TITLE_MAX_CHARS: usize = 40;

/// 判断当前 title 是否仍为由首条 user 消息规则派生的占位。
pub fn is_rule_derived_title(title: &str, user_text: &str) -> bool {
    title == derive_title_from_user_message(user_text)
}

/// 从首条 user message 文本派生会话标题：取首个非空行、trim、超过 40 字符截断加省略号；
/// 全空则回退 "New session"。纯函数，无副作用，便于单测。
pub fn derive_title_from_user_message(text: &str) -> String {
    let first_line = text
        .lines()
        .map(|line| line.trim())
        .find(|line| !line.is_empty());
    match first_line {
        Some(line) => {
            let count = line.chars().count();
            if count > TITLE_MAX_CHARS {
                let truncated: String = line.chars().take(TITLE_MAX_CHARS).collect();
                format!("{truncated}\u{2026}")
            } else {
                line.to_string()
            }
        }
        None => "New session".to_string(),
    }
}

/// 从 transcript / wire 的 `content` 字段中提取用户真实输入文本。
///
/// 兼容两种落盘形态：
/// - 纯字符串：`"content": "hello"`
/// - 结构化 parts：`"content": [{"type":"input_text","text":"hello"}]`
///
/// 对 parts 仅拼接 `input_text` 片段，忽略 reference / image / file；这样标题只来源于
/// 用户显式输入，不把上下文标签或附件元数据混进标题主体。
pub(crate) fn extract_user_text_from_content(content: &serde_json::Value) -> Option<String> {
    match content {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(parts) => {
            let mut text = String::new();
            let mut saw_input_text = false;
            for part in parts {
                if part.get("type").and_then(|v| v.as_str()) != Some("input_text") {
                    continue;
                }
                if let Some(chunk) = part.get("text").and_then(|v| v.as_str()) {
                    saw_input_text = true;
                    text.push_str(chunk);
                }
            }
            saw_input_text.then_some(text)
        }
        _ => None,
    }
}

struct AppendInFlightGuard {
    counter: Arc<AtomicUsize>,
}

impl Drop for AppendInFlightGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 会话管理器：持有 store 路径与写入锁，提供 CRUD 与 transcript 读写。
pub struct SessionManager {
    /// 会话根目录（已展开 ~）
    sessions_dir: PathBuf,
    /// sessions.json 完整路径
    store_path: PathBuf,
    /// 当前 manager 绑定的 scope key。
    session_key: String,
    /// 运行中 live 会话的进程内绑定；仅对当前 manager 自己的 session_key 生效。
    ///
    /// 这允许磁盘 `current[key]` 继续承担“跨进程默认指针”的角色，同时保证已启动
    /// 的 chat 在会话存活期间始终写回同一个 session_id。
    pinned_session_id: Arc<parking_lot::RwLock<Option<String>>>,
    /// 序列化 store 写入，禁止锁文件
    write_mutex: Arc<Mutex<()>>,
    transcript_mutexes: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
    append_in_flight: Arc<AtomicUsize>,
}

impl Clone for SessionManager {
    fn clone(&self) -> Self {
        Self {
            sessions_dir: self.sessions_dir.clone(),
            store_path: self.store_path.clone(),
            session_key: self.session_key.clone(),
            pinned_session_id: Arc::clone(&self.pinned_session_id),
            write_mutex: Arc::clone(&self.write_mutex),
            transcript_mutexes: Arc::clone(&self.transcript_mutexes),
            append_in_flight: Arc::clone(&self.append_in_flight),
        }
    }
}

impl SessionManager {
    /// 从已展开的 sessions_dir 路径创建；内部会使用 sessions_dir/sessions.json。
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self::new_scoped(sessions_dir, DEFAULT_SESSION_KEY.to_string())
    }

    /// 从已展开的 sessions_dir 路径创建一个绑定 scope 的 manager。
    pub fn new_scoped(sessions_dir: PathBuf, session_key: impl Into<String>) -> Self {
        let store_path = sessions_dir.join(SESSIONS_FILE);
        Self {
            sessions_dir: sessions_dir.clone(),
            store_path,
            session_key: session_key.into(),
            pinned_session_id: Arc::new(parking_lot::RwLock::new(None)),
            write_mutex: Arc::new(Mutex::new(())),
            transcript_mutexes: Arc::new(Mutex::new(HashMap::new())),
            append_in_flight: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 从配置中的 sessions_dir 字符串创建（会做 normalize_path）。
    pub fn from_sessions_dir(sessions_dir: &str) -> Result<Self, AppError> {
        Self::from_scoped_sessions_dir(sessions_dir, DEFAULT_SESSION_KEY)
    }

    /// 从配置中的 sessions_dir 字符串创建一个绑定 scope 的 manager。
    pub fn from_scoped_sessions_dir(
        sessions_dir: &str,
        session_key: impl Into<String>,
    ) -> Result<Self, AppError> {
        let path = normalize_path(sessions_dir)?;
        Ok(Self::new_scoped(path, session_key))
    }

    pub fn store_path(&self) -> &Path {
        &self.store_path
    }

    pub fn sessions_dir(&self) -> &Path {
        &self.sessions_dir
    }

    pub fn append_in_flight_counter(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.append_in_flight)
    }

    /// 加载当前 store；文件不存在或空则返回空 map。
    pub fn load_store(&self) -> Result<SessionStore, AppError> {
        load_store(&self.store_path)
    }

    /// 原子写 store；内部用 Mutex 序列化。
    fn save_store(&self, store: &SessionStore) -> Result<(), AppError> {
        save_store(&self.store_path, store)
    }

    fn with_store_mut<T>(
        &self,
        f: impl FnOnce(&mut SessionStore) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let _guard = self
            .write_mutex
            .lock()
            .map_err(|e| AppError::Config(format!("session store 写入锁异常: {}", e)))?;
        let mut store = load_store(&self.store_path)?;
        let output = f(&mut store)?;
        self.save_store(&store)?;
        Ok(output)
    }

    fn transcript_mutex_for_path(&self, path: &Path) -> Result<Arc<Mutex<()>>, AppError> {
        let mut registry = self
            .transcript_mutexes
            .lock()
            .map_err(|e| AppError::Config(format!("transcript 锁注册表异常: {}", e)))?;
        Ok(registry
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone())
    }

    fn with_transcript_lock<T>(
        &self,
        path: &Path,
        f: impl FnOnce() -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let lock = self.transcript_mutex_for_path(path)?;
        let _guard = lock
            .lock()
            .map_err(|e| AppError::Config(format!("transcript 写入锁异常: {}", e)))?;
        f()
    }

    /// 当前会话 key。
    pub fn current_session_key(&self) -> &str {
        &self.session_key
    }

    /// 将当前 manager 绑定到某个已解析的 session_id。
    pub fn pin_session(&self, session_id: &str) {
        *self.pinned_session_id.write() = Some(session_id.to_string());
    }

    fn has_pinned_session(&self) -> bool {
        self.pinned_session_id.read().is_some()
    }

    /// 解析某个 session_key 此刻应指向哪个 session_id。
    ///
    /// 对当前 live manager 自己的 key，进程内 pin 优先于磁盘 `current[key]`；对其它
    /// scope 的 key 仍完全沿用磁盘 current 语义，避免 pin 越权污染别的 scope。
    fn resolve_active_session_id(&self, store: &SessionStore, session_key: &str) -> Option<String> {
        if session_key == self.current_session_key() {
            if let Some(session_id) = self.pinned_session_id.read().clone() {
                return Some(session_id);
            }
        }
        store.current.get(session_key).cloned()
    }

    /// 当前会话条目；无当前映射时返回 None。
    pub fn current_session_entry(&self) -> Result<Option<SessionEntry>, AppError> {
        self.get_session(self.current_session_key())
    }

    /// 当前会话 session_id；无当前映射时返回 None。
    pub fn current_session_id(&self) -> Result<Option<String>, AppError> {
        Ok(self.current_session_entry()?.map(|entry| entry.session_id))
    }

    fn session_entry_for_key(
        &self,
        store: &SessionStore,
        session_key: &str,
    ) -> Option<SessionEntry> {
        let session_id = self.resolve_active_session_id(store, session_key)?;
        store.sessions.get(&session_id).cloned()
    }

    fn scope_entries(store: &SessionStore, session_key: &str) -> Vec<(String, SessionEntry)> {
        let mut entries: Vec<(String, SessionEntry)> = store
            .sessions
            .iter()
            .filter(|(_, entry)| entry.session_key == session_key)
            .map(|(session_id, entry)| (session_id.clone(), entry.clone()))
            .collect();
        entries.sort_by(|(_, left), (_, right)| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.session_id.cmp(&left.session_id))
        });
        entries
    }

    fn repoint_current_after_removal(
        store: &mut SessionStore,
        session_key: &str,
        removed_id: &str,
    ) {
        let current_matches = store
            .current
            .get(session_key)
            .is_some_and(|current_id| current_id == removed_id);
        if !current_matches {
            return;
        }
        let replacement = Self::scope_entries(store, session_key)
            .into_iter()
            .map(|(session_id, _)| session_id)
            .next();
        match replacement {
            Some(session_id) => {
                store.current.insert(session_key.to_string(), session_id);
            }
            None => {
                store.current.remove(session_key);
            }
        }
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
            session_key: session_key.to_string(),
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
            context_utilization_ratio: None,
            last_checkpoint_id: None,
            title: None,
        };
        self.with_store_mut(|store| {
            store.sessions.insert(session_id.clone(), entry.clone());
            store.current.insert(session_key.to_string(), session_id);
            Ok(())
        })?;
        Ok(entry)
    }

    /// 为当前固定 key 创建新的 session，并把 current 映射切到它。
    pub fn new_current_session(&self, cwd: Option<String>) -> Result<SessionEntry, AppError> {
        let entry = self.create_session(self.current_session_key(), cwd)?;
        if self.has_pinned_session() {
            self.pin_session(&entry.session_id);
        }
        Ok(entry)
    }

    /// 确保当前固定 key 已绑定某个 session；缺失时创建新的 current session。
    pub fn ensure_current_session(&self, cwd: Option<String>) -> Result<SessionEntry, AppError> {
        if let Some(entry) = self.current_session_entry()? {
            return Ok(entry);
        }
        self.new_current_session(cwd)
    }

    /// 把当前固定 key 切到某个已存在的 session_id。
    pub fn switch_current_to_session_id(&self, session_id: &str) -> Result<SessionEntry, AppError> {
        let entry = self.with_store_mut(|store| {
            let Some(entry) = store.sessions.get_mut(session_id) else {
                return Err(AppError::Config(format!("会话不存在: {session_id}")));
            };
            if entry.session_key != self.current_session_key() {
                return Err(AppError::Config(format!(
                    "会话不属于当前 scope: {session_id}"
                )));
            }
            entry.updated_at = Utc::now().timestamp_millis();
            let entry = entry.clone();
            store.current.insert(
                self.current_session_key().to_string(),
                session_id.to_string(),
            );
            Ok(entry)
        })?;
        if self.has_pinned_session() {
            self.pin_session(&entry.session_id);
        }
        Ok(entry)
    }

    /// 列出 sessions 目录下所有历史 session_id（按文件名倒序，通常也是时间倒序）。
    pub fn list_session_ids(&self) -> Result<Vec<String>, AppError> {
        let rd = match std::fs::read_dir(&self.sessions_dir) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(AppError::Io(e)),
        };
        let mut ids: Vec<String> = rd
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("jsonl"))
            .filter_map(|path| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(ToOwned::to_owned)
            })
            .collect();
        ids.sort();
        ids.reverse();
        Ok(ids)
    }

    /// 按 sessionKey 获取元数据。
    pub fn get_session(&self, session_key: &str) -> Result<Option<SessionEntry>, AppError> {
        let store = self.load_store()?;
        Ok(self.session_entry_for_key(&store, session_key))
    }

    /// 按 session_id 获取元数据。
    pub fn get_session_by_id(&self, session_id: &str) -> Result<Option<SessionEntry>, AppError> {
        let store = self.load_store()?;
        Ok(store.sessions.get(session_id).cloned())
    }

    /// 列出当前 scope 下的所有 session（按 updated_at 倒序）。
    pub fn list_sessions(&self) -> Result<Vec<(String, SessionEntry)>, AppError> {
        let store = self.load_store()?;
        Ok(Self::scope_entries(&store, self.current_session_key()))
    }

    /// 更新某 sessionKey 当前指向会话的元数据（如 model_override、cwd）。
    pub fn update_session(
        &self,
        session_key: &str,
        f: impl FnOnce(&mut SessionEntry),
    ) -> Result<(), AppError> {
        self.with_store_mut(|store| {
            let Some(session_id) = self.resolve_active_session_id(store, session_key) else {
                return Ok(());
            };
            if let Some(entry) = store.sessions.get_mut(&session_id) {
                entry.updated_at = Utc::now().timestamp_millis();
                f(entry);
            }
            Ok(())
        })
    }

    /// 首条 user message 写入后，若当前会话尚无 title，则派生并持久化一次。
    /// 已有非占位 title 直接返回；非 user message 直接返回。
    fn ensure_title_from_message(&self, message: &serde_json::Value) -> Result<(), AppError> {
        let session_key = self.current_session_key().to_string();
        self.ensure_title_for_session_key(&session_key, message)
    }

    /// 按 sessionKey 派生并持久化 title（首条 user、无 title 时才写）。
    fn ensure_title_for_session_key(
        &self,
        session_key: &str,
        message: &serde_json::Value,
    ) -> Result<(), AppError> {
        let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role != "user" {
            return Ok(());
        }
        let text = message
            .get("content")
            .and_then(extract_user_text_from_content)
            .unwrap_or_default();
        let store = self.load_store()?;
        let session_id = match self.resolve_active_session_id(&store, session_key) {
            Some(id) => id,
            None => return Ok(()),
        };
        if store
            .sessions
            .get(&session_id)
            .and_then(|entry| entry.title.as_ref())
            .is_some_and(|title| !is_rule_derived_title(title, &text))
        {
            return Ok(());
        }
        let title = derive_title_from_user_message(&text);
        self.update_session(session_key, |entry| {
            if entry.title.is_none()
                || entry
                    .title
                    .as_ref()
                    .is_some_and(|existing| is_rule_derived_title(existing, &text))
            {
                entry.title = Some(title.clone());
            }
        })
    }

    /// 按 session_id 派生并持久化 title（插件多实例路由路径）。
    fn ensure_title_for_session_id(
        &self,
        session_id: &str,
        message: &serde_json::Value,
    ) -> Result<(), AppError> {
        let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role != "user" {
            return Ok(());
        }
        let needs_title = self
            .load_store()?
            .sessions
            .get(session_id)
            .and_then(|entry| entry.title.as_ref())
            .is_none();
        if !needs_title {
            return Ok(());
        }
        let text = message
            .get("content")
            .and_then(extract_user_text_from_content)
            .unwrap_or_default();
        let title = derive_title_from_user_message(&text);
        self.with_store_mut(|store| {
            if let Some(entry) = store.sessions.get_mut(session_id) {
                if entry.title.is_none() {
                    entry.updated_at = Utc::now().timestamp_millis();
                    entry.title = Some(title.clone());
                }
            }
            Ok(())
        })
    }

    /// 更新当前会话的 model_override，并落一条可审计的 model_change transcript 事件。
    pub fn switch_current_model(
        &self,
        provider: Option<&str>,
        model_id: Option<&str>,
    ) -> Result<(), AppError> {
        let key = self.current_session_key();
        if self.get_session(key)?.is_none() {
            return Err(AppError::Config("无当前会话".to_string()));
        }
        let normalized_model = model_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        self.update_session(key, |entry| {
            entry.model_override = normalized_model.clone();
        })?;
        self.append_model_change(provider, normalized_model.as_deref())
    }

    /// 将 `ContextState` 中会话级可观测累计写入 `sessions.json`（user turn 末节流刷盘）。
    pub fn persist_context_observability(&self, state: &ContextState) -> Result<(), AppError> {
        let key = self.current_session_key();
        self.update_session(key, |e| {
            e.compaction_count = Some(state.session_obs.compaction_count);
            e.compaction_tokens_freed = Some(state.session_obs.compaction_tokens_freed as u64);
            e.tool_result_chars_persisted =
                Some(state.session_obs.tool_result_chars_persisted as u64);
            e.context_utilization_ratio = Some(state.live.context_utilization_ratio);
        })
    }

    /// 删除会话：从 store 移除并删除 transcript 文件（若存在）。
    pub fn delete_session(&self, session_id: &str) -> Result<(), AppError> {
        let entry = self.with_store_mut(|store| {
            let Some(entry) = store.sessions.get(session_id).cloned() else {
                return Err(AppError::Config(format!("会话不存在: {session_id}")));
            };
            if entry.session_key != self.current_session_key() {
                return Err(AppError::Config(format!(
                    "会话不属于当前 scope: {session_id}"
                )));
            }
            store.sessions.remove(session_id);
            Self::repoint_current_after_removal(store, &entry.session_key, session_id);
            Ok(entry)
        })?;
        let path = self.transcript_path(&entry.session_id);
        let _ = std::fs::remove_file(&path);
        let _ = remove_resume_index(&path);
        Ok(())
    }

    /// 归档：仅从 store 移除会话元数据，不删 transcript 文件。
    pub fn archive_session(&self, session_id: &str) -> Result<(), AppError> {
        self.with_store_mut(|store| {
            let Some(entry) = store.sessions.get(session_id).cloned() else {
                return Err(AppError::Config(format!("会话不存在: {session_id}")));
            };
            if entry.session_key != self.current_session_key() {
                return Err(AppError::Config(format!(
                    "会话不属于当前 scope: {session_id}"
                )));
            }
            store.sessions.remove(session_id);
            Self::repoint_current_after_removal(store, &entry.session_key, session_id);
            Ok(())
        })
    }

    /// 获取当前会话的 transcript 路径；无当前会话返回 None。
    pub fn current_transcript_path(&self) -> Result<Option<PathBuf>, AppError> {
        Ok(self
            .current_session_entry()?
            .map(|entry| self.transcript_path(&entry.session_id)))
    }

    fn message_sync_level(message: &serde_json::Value) -> SyncLevel {
        let role = message
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let has_tool_calls = message
            .get("tool_calls")
            .and_then(|v| v.as_array())
            .map(|items| !items.is_empty())
            .unwrap_or(false);
        match role {
            "assistant" if has_tool_calls => SyncLevel::Flush,
            _ => SyncLevel::SyncData,
        }
    }

    fn append_message_internal(
        &self,
        message: serde_json::Value,
        chain_violation_is_invariant: bool,
        forced_id: Option<&str>,
    ) -> Result<String, AppError> {
        self.append_in_flight.fetch_add(1, Ordering::SeqCst);
        let _guard = AppendInFlightGuard {
            counter: Arc::clone(&self.append_in_flight),
        };
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        self.with_transcript_lock(&path, || {
            let recent = read_entries_tail(&path, VALIDATE_TAIL_CAP).unwrap_or_default();
            let recent_msgs = collect_recent_chat_messages_from_tail(&recent);
            if let Err(reason) = validate_append_message(&message, &recent_msgs) {
                let err = AppError::invariant("append_message_chain", reason);
                return if chain_violation_is_invariant {
                    Err(err)
                } else {
                    Err(AppError::Config(err.to_string()))
                };
            }
            let id = forced_id
                .map(ToOwned::to_owned)
                .unwrap_or_else(generate_entry_id);
            let now = iso_ts_now()?;
            let sync = Self::message_sync_level(&message);
            let message_for_title = message.clone();
            let entry = TranscriptEntry::Message(MessageEntry {
                id: Some(id.clone()),
                parent_id: None,
                timestamp: now,
                message,
            });
            append_entry_with_sync(&path, &entry, sync)?;
            let _ = self.ensure_title_from_message(&message_for_title);
            Ok(id)
        })
    }

    // 同一 transcript 文件通过 per-file mutex 串行化；不同 transcript 仍可并行追加。
    /// 追加 message 到当前会话的 transcript；返回新行的 `MessageEntry.id`（§5.7 MessageId）。
    pub fn append_message(&self, message: serde_json::Value) -> Result<String, AppError> {
        self.append_message_internal(message, true, None)
    }

    /// 以指定的 transcript `MessageEntry.id` 追加当前会话消息。
    pub fn append_message_with_id(
        &self,
        message: serde_json::Value,
        forced_id: &str,
    ) -> Result<String, AppError> {
        self.append_message_internal(message, true, Some(forced_id))
    }

    /// 追加 message（dispatcher/插件路径：校验失败返回 Err 而非 panic）。
    /// 返回新行的 `MessageEntry.id`（§5.7 MessageId）。
    pub fn try_append_message(&self, message: serde_json::Value) -> Result<String, AppError> {
        self.append_message_internal(message, false, None)
    }

    /// 追加 message 到指定 session 的 transcript（插件多实例路由）。
    pub fn try_append_message_to_session(
        &self,
        session_id: &str,
        message: serde_json::Value,
    ) -> Result<String, AppError> {
        self.append_in_flight.fetch_add(1, Ordering::SeqCst);
        let _guard = AppendInFlightGuard {
            counter: Arc::clone(&self.append_in_flight),
        };
        if self.get_session_by_id(session_id)?.is_none() {
            return Err(AppError::Config(format!("会话不存在: {session_id}")));
        }
        let path = self.transcript_path(session_id);
        self.with_transcript_lock(&path, || {
            let recent = read_entries_tail(&path, VALIDATE_TAIL_CAP).unwrap_or_default();
            let recent_msgs = collect_recent_chat_messages_from_tail(&recent);
            if let Err(reason) = validate_append_message(&message, &recent_msgs) {
                return Err(AppError::Config(
                    AppError::invariant("append_message_chain", reason).to_string(),
                ));
            }
            let id = generate_entry_id();
            let now = iso_ts_now()?;
            let sync = Self::message_sync_level(&message);
            let message_for_title = message.clone();
            let entry = TranscriptEntry::Message(MessageEntry {
                id: Some(id.clone()),
                parent_id: None,
                timestamp: now,
                message,
            });
            append_entry_with_sync(&path, &entry, sync)?;
            let _ = self.ensure_title_for_session_id(session_id, &message_for_title);
            Ok(id)
        })
    }

    /// 追加 thinking_level_change。
    pub fn append_thinking_level_change(&self, thinking_level: &str) -> Result<(), AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        self.with_transcript_lock(&path, || {
            let entry = TranscriptEntry::ThinkingLevelChange(ThinkingLevelChangeEntry {
                id: None,
                parent_id: None,
                timestamp: iso_ts_now()?,
                thinking_level: Some(thinking_level.to_string()),
            });
            append_entry(&path, &entry)
        })
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
        self.with_transcript_lock(&path, || {
            let entry = TranscriptEntry::ThinkingTrace(ThinkingTraceEntry {
                id: None,
                parent_id: None,
                timestamp: iso_ts_now()?,
                text: text.to_string(),
                signature: signature.map(str::to_string),
            });
            append_entry(&path, &entry)
        })
    }

    /// 追加自定义 transcript 事件（如 checkpoint.restore）。
    pub fn append_custom_entry(&self, extra: serde_json::Value) -> Result<(), AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        self.with_transcript_lock(&path, || {
            let entry = TranscriptEntry::Custom(CustomEntry {
                id: Some(generate_entry_id()),
                parent_id: None,
                timestamp: iso_ts_now()?,
                extra,
            });
            append_entry(&path, &entry)
        })
    }

    /// 按 `message.id` 重写指定 session transcript 中 assistant message 的 `summary_title`。
    ///
    /// 用于异步 utility 标题生成完成后，覆盖先前持久化的规则占位标题。
    pub fn rewrite_message_summary_title_in_session(
        &self,
        session_id: &str,
        message_id: &str,
        summary_title: &str,
    ) -> Result<usize, AppError> {
        let path = self.transcript_path(session_id);
        self.with_transcript_lock(&path, || {
            rewrite_message_summary_titles_by_id(
                &path,
                &[MessageSummaryTitleRewrite {
                    message_id: message_id.to_string(),
                    summary_title: summary_title.to_string(),
                }],
            )
        })
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
        self.with_transcript_lock(&path, || {
            let entry = TranscriptEntry::ModelChange(ModelChangeEntry {
                id: None,
                parent_id: None,
                timestamp: iso_ts_now()?,
                provider: provider.map(String::from),
                model_id: model_id.map(String::from),
            });
            append_entry(&path, &entry)
        })
    }

    /// 追加 compaction（基本版，不含覆盖范围信息）。
    pub fn append_compaction(&self, summary: Option<&str>) -> Result<(), AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        self.with_transcript_lock(&path, || {
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
        })
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
        self.with_transcript_lock(&path, || {
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
        })
    }

    /// 追加 session_info（如会话名称）。
    pub fn append_session_info(&self, name: &str) -> Result<(), AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        self.with_transcript_lock(&path, || {
            let entry = TranscriptEntry::SessionInfo(SessionInfoEntry {
                id: None,
                parent_id: None,
                timestamp: iso_ts_now()?,
                name: Some(name.to_string()),
            });
            append_entry(&path, &entry)
        })
    }

    /// 追加 label。
    pub fn append_label_change(&self, label: &str) -> Result<(), AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        self.with_transcript_lock(&path, || {
            let entry = TranscriptEntry::Label(LabelEntry {
                id: None,
                parent_id: None,
                timestamp: iso_ts_now()?,
                label: Some(label.to_string()),
            });
            append_entry(&path, &entry)
        })
    }

    pub fn mark_messages_after_anchor_superseded(&self, anchor: &str) -> Result<usize, AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        self.with_transcript_lock(&path, || {
            mark_message_entries_after_anchor_superseded(&path, anchor)
        })
    }

    /// 获取当前会话 transcript 中最近 cap 条 entry。
    pub fn get_entries(&self, cap: usize) -> Result<Vec<TranscriptEntry>, AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        read_entries_tail(&path, cap)
    }

    pub fn get_entries_before(
        &self,
        cap: usize,
        before: Option<u64>,
    ) -> Result<TranscriptPage, AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        read_entries_tail_before(&path, cap, before)
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

    pub fn get_entries_for_session(
        &self,
        session_id: &str,
        cap: usize,
    ) -> Result<Vec<TranscriptEntry>, AppError> {
        if self.get_session_by_id(session_id)?.is_none() {
            return Err(AppError::Config(format!("会话不存在: {session_id}")));
        }
        read_entries_tail(&self.transcript_path(session_id), cap)
    }

    pub fn get_entries_before_for_session(
        &self,
        session_id: &str,
        cap: usize,
        before: Option<u64>,
    ) -> Result<TranscriptPage, AppError> {
        if self.get_session_by_id(session_id)?.is_none() {
            return Err(AppError::Config(format!("会话不存在: {session_id}")));
        }
        read_entries_tail_before(&self.transcript_path(session_id), cap, before)
    }

    pub fn get_entry_for_session(
        &self,
        session_id: &str,
        id: &str,
    ) -> Result<Option<TranscriptEntry>, AppError> {
        if self.get_session_by_id(session_id)?.is_none() {
            return Err(AppError::Config(format!("会话不存在: {session_id}")));
        }
        get_entry(&self.transcript_path(session_id), id)
    }

    pub fn get_leaf_entry_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<TranscriptEntry>, AppError> {
        if self.get_session_by_id(session_id)?.is_none() {
            return Err(AppError::Config(format!("会话不存在: {session_id}")));
        }
        get_leaf_entry(&self.transcript_path(session_id))
    }

    pub fn get_branch_for_session(
        &self,
        session_id: &str,
        leaf_id: &str,
    ) -> Result<Vec<TranscriptEntry>, AppError> {
        if self.get_session_by_id(session_id)?.is_none() {
            return Err(AppError::Config(format!("会话不存在: {session_id}")));
        }
        get_branch(
            &self.transcript_path(session_id),
            leaf_id,
            super::BRANCH_MAX_ENTRIES,
        )
    }

    /// 读取当前会话 transcript 的 header。
    pub fn read_session_header(&self) -> Result<Option<SessionHeader>, AppError> {
        let path = match self.current_transcript_path()? {
            Some(p) => p,
            None => return Ok(None),
        };
        read_header(&path).map(Some)
    }

    pub fn read_session_header_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionHeader>, AppError> {
        if self.get_session_by_id(session_id)?.is_none() {
            return Ok(None);
        }
        read_header(&self.transcript_path(session_id)).map(Some)
    }
}

impl MessageAppendSink for SessionManager {
    fn append_message(&self, value: serde_json::Value) -> Result<String, AppError> {
        SessionManager::append_message(self, value)
    }

    fn append_message_with_id(
        &self,
        value: serde_json::Value,
        forced_id: &str,
    ) -> Result<String, AppError> {
        SessionManager::append_message_with_id(self, value, forced_id)
    }
}

fn uuid_for_session() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let seq = SESSION_ID_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let mut hasher = DefaultHasher::new();
    nanos.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    seq.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
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
