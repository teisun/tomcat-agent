//! SessionManager：会话 CRUD、transcript 追加与只读、上下文组装、会话级配置隔离。
//!
//! 通过 Mutex 序列化 sessions.json 的写入，保证并发安全（不锁文件）。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::infra::error::AppError;
use crate::infra::platform::normalize_path;
use chrono::Utc;

use super::store::{load_store, save_store, SessionEntry, SessionStore, DEFAULT_SESSION_KEY};
use super::transcript::{
    append_entry, get_branch, get_children, get_entry, get_leaf_entry, read_entries_tail,
    read_header, write_header, MessageEntry, SessionHeader, TranscriptEntry,
};

use crate::infra::config::{compute_context_budget_chars, ContextConfig};

const SESSIONS_FILE: &str = "sessions.json";
const DEFAULT_CONTEXT_CAP: usize = 10;
const BRANCH_MAX_ENTRIES: usize = 2000;

// ---------------------------------------------------------------------------
// Context management data structures (TASK-17)
// ---------------------------------------------------------------------------

use crate::core::agent_loop::AgentMessage;

/// 上下文管理的分组单位：一条 user 消息及其后所有 assistant/tool 消息，
/// 或一条 Compaction 生成的结构化摘要。
#[derive(Debug, Clone)]
pub enum TurnEntry {
    UserTurn { messages: Vec<AgentMessage> },
    SummaryTurn { summary: String },
}

/// API token 使用量快照（从 `StreamEvent::Usage` 捕获）。
#[derive(Debug, Clone)]
pub struct ApiUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

/// 运行时上下文状态，在 `chat_loop` 外层初始化一次、跨迭代复用。
pub struct ContextState {
    pub user_turns_list: Vec<TurnEntry>,
    pub estimate_context_chars: usize,
    pub context_budget_chars: usize,
    pub context_budget_tokens: usize,
    pub last_api_usage: Option<ApiUsage>,
    pub post_usage_appended_chars: usize,
    pub compaction_consecutive_failures: u32,
}

impl ContextState {
    /// 追加消息后增量更新估算字符数和 post-usage 增量。
    pub fn on_message_appended(&mut self, content_len: usize) {
        self.estimate_context_chars += content_len;
        self.post_usage_appended_chars += content_len;
    }

    /// 新 user turn 完成后追加到 turns 列表并更新估算。
    pub fn on_new_user_turn(&mut self, turn: TurnEntry) {
        let chars = estimate_turn_chars(&turn);
        self.estimate_context_chars += chars;
        self.post_usage_appended_chars += chars;
        self.user_turns_list.push(turn);
    }

    /// 估算当前上下文占用的 token 数。
    /// 有 API usage 时基于真实值 + 增量近似；否则 fallback 字符估算。
    pub fn estimated_token_count(&self) -> usize {
        if let Some(ref usage) = self.last_api_usage {
            let base = (usage.prompt_tokens + usage.completion_tokens) as usize;
            base + self.post_usage_appended_chars / 4
        } else {
            self.estimate_context_chars / 4
        }
    }

    /// 当前上下文利用率（0.0 ~ ∞）。
    /// `context_budget_tokens == 0` 时返回 `f64::MAX` 以安全触发 Layer 3。
    pub fn usage_ratio(&self) -> f64 {
        if self.context_budget_tokens == 0 {
            return f64::MAX;
        }
        self.estimated_token_count() as f64 / self.context_budget_tokens as f64
    }

    /// LLM 返回 Usage 事件后刷新真实 token 计数，清零增量。
    pub fn update_api_usage(&mut self, prompt_tokens: u32, completion_tokens: u32) {
        self.last_api_usage = Some(ApiUsage {
            prompt_tokens,
            completion_tokens,
        });
        self.post_usage_appended_chars = 0;
    }

    /// compaction 后使 API usage 失效，后续 fallback 到字符估算。
    pub fn invalidate_api_usage(&mut self) {
        self.last_api_usage = None;
        self.post_usage_appended_chars = 0;
    }

    /// 当前上下文是否超预算（token 维度）。
    pub fn is_over_budget(&self) -> bool {
        self.estimated_token_count() > self.context_budget_tokens
    }
}

/// 估算单个 TurnEntry 的字符数。
pub fn estimate_turn_chars(turn: &TurnEntry) -> usize {
    match turn {
        TurnEntry::UserTurn { messages } => messages
            .iter()
            .map(|m| match m {
                AgentMessage::User { text } => text.len(),
                AgentMessage::Assistant { text, tool_calls } => {
                    text.len()
                        + tool_calls
                            .iter()
                            .map(|tc| tc.name.len() + tc.arguments.len() + tc.id.len() + 40)
                            .sum::<usize>()
                }
                AgentMessage::ToolResult { content, .. } => content.len(),
                AgentMessage::System { text } => text.len(),
                AgentMessage::Steering { text, .. } => text.len(),
                AgentMessage::CompactionSummary { summary } => summary.len(),
            })
            .sum(),
        TurnEntry::SummaryTurn { summary } => summary.len(),
    }
}

/// 从 transcript 加载历史，按 user turn 分组初始化 ContextState。
/// 识别已有 Compaction entry 折叠为 SummaryTurn，避免重复压缩。
pub fn init_context_state(
    session: &SessionManager,
    config: &ContextConfig,
    system_text: &str,
) -> Result<ContextState, AppError> {
    let budget = compute_context_budget_chars(config);
    let token_budget = config.context_window.saturating_sub(config.max_output_tokens);

    let path = match session.current_transcript_path()? {
        Some(p) => p,
        None => {
            return Ok(ContextState {
                user_turns_list: Vec::new(),
                estimate_context_chars: system_text.len(),
                context_budget_chars: budget,
                context_budget_tokens: token_budget,
                last_api_usage: None,
                post_usage_appended_chars: 0,
                compaction_consecutive_failures: 0,
            });
        }
    };

    let entries = read_entries_tail(&path, BRANCH_MAX_ENTRIES)?;

    let mut turns: Vec<TurnEntry> = Vec::new();
    let mut current_turn_msgs: Vec<AgentMessage> = Vec::new();
    let mut total_chars = system_text.len();

    for entry in entries {
        match entry {
            TranscriptEntry::Compaction(ce) => {
                if !current_turn_msgs.is_empty() {
                    let turn = TurnEntry::UserTurn {
                        messages: std::mem::take(&mut current_turn_msgs),
                    };
                    total_chars += estimate_turn_chars(&turn);
                    turns.push(turn);
                }

                if ce.is_boundary == Some(true) {
                    turns.clear();
                    total_chars = system_text.len();
                }

                if let Some(ref summary) = ce.summary {
                    total_chars += summary.len();
                    turns.push(TurnEntry::SummaryTurn {
                        summary: summary.clone(),
                    });
                }
            }
            TranscriptEntry::Message(me) => {
                let role = me.message.get("role").and_then(|r| r.as_str());
                let content = me
                    .message
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("");

                if role == Some("user") && !current_turn_msgs.is_empty() {
                    let turn = TurnEntry::UserTurn {
                        messages: std::mem::take(&mut current_turn_msgs),
                    };
                    total_chars += estimate_turn_chars(&turn);
                    turns.push(turn);
                }

                let agent_msg = match role {
                    Some("user") => AgentMessage::User {
                        text: content.to_string(),
                    },
                    Some("assistant") => {
                        let tool_calls = me
                            .message
                            .get("tool_calls")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| {
                                        let obj = v.as_object()?;
                                        let id = obj.get("id")?.as_str()?.to_string();
                                        let func = obj.get("function")?.as_object()?;
                                        let name = func.get("name")?.as_str()?.to_string();
                                        let arguments = func
                                            .get("arguments")
                                            .and_then(|a| a.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        Some(crate::core::agent_loop::ToolCallInfo {
                                            id,
                                            name,
                                            arguments,
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        AgentMessage::Assistant {
                            text: content.to_string(),
                            tool_calls,
                        }
                    }
                    Some("tool") => AgentMessage::ToolResult {
                        tool_call_id: me
                            .message
                            .get("tool_call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        content: content.to_string(),
                        is_error: false,
                    },
                    _ => continue,
                };
                current_turn_msgs.push(agent_msg);
            }
            _ => {}
        }
    }

    if !current_turn_msgs.is_empty() {
        let turn = TurnEntry::UserTurn {
            messages: std::mem::take(&mut current_turn_msgs),
        };
        total_chars += estimate_turn_chars(&turn);
        turns.push(turn);
    }

    Ok(ContextState {
        user_turns_list: turns,
        estimate_context_chars: total_chars,
        context_budget_chars: budget,
        context_budget_tokens: token_budget,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        compaction_consecutive_failures: 0,
    })
}

/// 将 ContextState 中的 turns 展平为 AgentMessage 列表（不含 system prompt）。
pub fn build_context_from_state(state: &ContextState) -> Vec<AgentMessage> {
    let mut out = Vec::new();
    for turn in &state.user_turns_list {
        match turn {
            TurnEntry::UserTurn { messages } => out.extend(messages.iter().cloned()),
            TurnEntry::SummaryTurn { summary } => {
                out.push(AgentMessage::CompactionSummary {
                    summary: summary.clone(),
                });
            }
        }
    }
    out
}

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

    /// 追加 message 到当前会话的 transcript。
    pub fn append_message(&self, message: serde_json::Value) -> Result<(), AppError> {
        let path = self
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        let now = iso_ts_now()?;
        let entry = TranscriptEntry::Message(MessageEntry {
            id: None,
            parent_id: None,
            timestamp: now,
            message,
        });
        append_entry(&path, &entry)
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
        let entry = TranscriptEntry::Compaction(CompactionEntry {
            id: None,
            parent_id: None,
            timestamp: iso_ts_now()?,
            summary: summary.map(String::from),
            covered_start_id: None,
            covered_end_id: None,
            covered_count: None,
            is_boundary: None,
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
        let entry = TranscriptEntry::Compaction(CompactionEntry {
            id: None,
            parent_id: None,
            timestamp: iso_ts_now()?,
            summary: Some(summary.to_string()),
            covered_start_id,
            covered_end_id,
            covered_count: Some(covered_count),
            is_boundary: None,
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

    /// 根据会话历史组装 LLM 所需消息列表。
    ///
    /// 取最近 `recent_n` 条 transcript entry 中的 Message，然后**向前扩展**
    /// 直到首条为 `role: user`（或耗尽全部 entry）。这保证调用方注入 system
    /// 后形态为 `[system, user, …]`，避免 OpenAI 400（tool 必须跟在含
    /// tool_calls 的 assistant 之后）。
    pub fn build_context_messages(
        &self,
        recent_n: usize,
    ) -> Result<Vec<serde_json::Value>, AppError> {
        let path = match self.current_transcript_path()? {
            Some(p) => p,
            None => return Err(AppError::Config("无当前会话".to_string())),
        };

        let all_entries = read_entries_tail(&path, BRANCH_MAX_ENTRIES)?;
        let all_messages: Vec<serde_json::Value> = all_entries
            .into_iter()
            .filter_map(|e| {
                if let TranscriptEntry::Message(me) = e {
                    Some(me.message)
                } else {
                    None
                }
            })
            .collect();

        if all_messages.is_empty() {
            return Ok(Vec::new());
        }

        let start = if all_messages.len() > recent_n {
            all_messages.len() - recent_n
        } else {
            0
        };

        let mut anchor = start;
        while anchor > 0 {
            if all_messages[anchor].get("role").and_then(|r| r.as_str()) == Some("user") {
                break;
            }
            anchor -= 1;
        }

        Ok(all_messages[anchor..].to_vec())
    }

    /// 会话级上下文窗口条数；MVP 固定 DEFAULT_CONTEXT_CAP。
    pub fn context_cap(&self) -> usize {
        DEFAULT_CONTEXT_CAP
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
        get_branch(&path, leaf_id, BRANCH_MAX_ENTRIES)
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

// 供 append_* 使用的 transcript 条目类型
use super::transcript::{
    CompactionEntry, LabelEntry, ModelChangeEntry, SessionInfoEntry, ThinkingLevelChangeEntry,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_sessions_dir() -> PathBuf {
        let c = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        std::env::temp_dir().join(format!("pi_wasm_mgr_{}_{}_{}", std::process::id(), ms, c))
    }

    #[test]
    fn create_session_and_list() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        let entry = mgr.create_session(key, Some("/tmp".to_string())).unwrap();
        assert!(!entry.session_id.is_empty());
        assert!(entry.updated_at > 0);
        let list = mgr.list_sessions().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, key);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_store_empty_when_no_file() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let store = mgr.load_store().unwrap();
        assert!(store.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_then_get_entries_and_build_context() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();
        let entries = mgr.get_entries(10).unwrap();
        assert!(entries.is_empty());
        let ctx = mgr.build_context_messages(10).unwrap();
        assert!(ctx.is_empty());
        mgr.append_message(serde_json::json!({"role":"user","content":"hi"}))
            .unwrap();
        let entries2 = mgr.get_entries(10).unwrap();
        assert_eq!(entries2.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_session_removes_from_store() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();
        assert_eq!(mgr.list_sessions().unwrap().len(), 1);
        mgr.delete_session(key).unwrap();
        assert!(mgr.list_sessions().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn get_session_returns_none_for_unknown_key() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let opt = mgr.get_session("unknown:key").unwrap();
        assert!(opt.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_sessions_dir_with_absolute_path() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path_str = dir.to_string_lossy();
        let mgr = SessionManager::from_sessions_dir(path_str.as_ref()).unwrap();
        assert!(mgr.store_path().ends_with("sessions.json"));
        assert!(mgr.load_store().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn transcript_path_format() {
        let dir = temp_sessions_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let p = mgr.transcript_path("sid_123");
        assert!(p.ends_with("sid_123.jsonl"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn get_session_returns_some_after_create() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        let created = mgr.create_session(key, None).unwrap();
        let opt = mgr.get_session(key).unwrap();
        assert!(opt.is_some());
        let entry = opt.unwrap();
        assert_eq!(entry.session_id, created.session_id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_session_header_after_create() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();
        let header = mgr.read_session_header().unwrap();
        assert!(header.is_some());
        assert!(!header.unwrap().id.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_session_header_none_when_no_session() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let header = mgr.read_session_header().unwrap();
        assert!(header.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn context_cap_returns_default() {
        let dir = temp_sessions_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        assert_eq!(mgr.context_cap(), 10);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn get_entry_with_session_returns_option() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();
        mgr.append_message(serde_json::json!({"role":"user","content":"hi"}))
            .unwrap();
        let opt = mgr.get_entry("unknown_id").unwrap();
        assert!(opt.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn get_children_with_session_returns_vec() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();
        let children = mgr.get_children("parent", 5).unwrap();
        assert!(children.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn get_leaf_entry_with_session_returns_last() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();
        mgr.append_message(serde_json::json!({"role":"user","content":"hi"}))
            .unwrap();
        let leaf = mgr.get_leaf_entry().unwrap();
        assert!(leaf.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn get_branch_with_session_returns_vec() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();
        let branch = mgr.get_branch("any_leaf").unwrap();
        assert!(branch.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_session_modifies_store() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();
        let before = mgr.get_session(key).unwrap().unwrap().updated_at;
        mgr.update_session(key, |e| {
            e.cwd = Some("/updated".to_string());
        })
        .unwrap();
        let after = mgr.get_session(key).unwrap().unwrap();
        assert!(after.updated_at >= before);
        assert_eq!(after.cwd.as_deref(), Some("/updated"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_thinking_level_change_succeeds() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();
        let r = mgr.append_thinking_level_change("full");
        assert!(r.is_ok());
        let entries = mgr.get_entries(10).unwrap();
        assert_eq!(entries.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_model_change_succeeds() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();
        let r = mgr.append_model_change(Some("openai"), Some("gpt-4"));
        assert!(r.is_ok());
        let entries = mgr.get_entries(10).unwrap();
        assert_eq!(entries.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_context_messages_anchors_on_user() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();

        mgr.append_message(serde_json::json!({"role":"user","content":"q1"}))
            .unwrap();
        mgr.append_message(serde_json::json!({"role":"assistant","content":"a1","tool_calls":[{"id":"c1","type":"function","function":{"name":"read_file","arguments":"{}"}}]}))
            .unwrap();
        mgr.append_message(serde_json::json!({"role":"tool","tool_call_id":"c1","content":"ok"}))
            .unwrap();
        mgr.append_message(serde_json::json!({"role":"assistant","content":"done"}))
            .unwrap();
        mgr.append_message(serde_json::json!({"role":"user","content":"q2"}))
            .unwrap();

        // cap=2 would naively start at "assistant:done" + "user:q2", but anchor
        // should expand back to the nearest user before assistant
        let msgs = mgr.build_context_messages(2).unwrap();
        let first_role = msgs[0]["role"].as_str().unwrap();
        assert_eq!(first_role, "user", "首条应为 user 而非 {:?}", msgs[0]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_context_messages_all_user_stays_same() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();

        for i in 0..5 {
            mgr.append_message(serde_json::json!({"role":"user","content":format!("q{}",i)}))
                .unwrap();
        }

        let msgs = mgr.build_context_messages(3).unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["role"].as_str().unwrap(), "user");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_context_messages_empty_transcript() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();

        let msgs = mgr.build_context_messages(10).unwrap();
        assert!(msgs.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_context_state_empty_session() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();

        let cfg = ContextConfig::default();
        let state = init_context_state(&mgr, &cfg, "system prompt").unwrap();
        assert!(state.user_turns_list.is_empty());
        assert_eq!(state.estimate_context_chars, "system prompt".len());
        assert_eq!(state.context_budget_chars, 816_000);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_context_state_with_messages() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();

        mgr.append_message(serde_json::json!({"role":"user","content":"q1"})).unwrap();
        mgr.append_message(serde_json::json!({"role":"assistant","content":"a1"})).unwrap();
        mgr.append_message(serde_json::json!({"role":"user","content":"q2"})).unwrap();
        mgr.append_message(serde_json::json!({"role":"assistant","content":"a2"})).unwrap();

        let cfg = ContextConfig::default();
        let state = init_context_state(&mgr, &cfg, "sys").unwrap();
        assert_eq!(state.user_turns_list.len(), 2);
        assert!(state.estimate_context_chars > 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_context_state_with_compaction_entry() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();

        mgr.append_compaction(Some("summary of old turns")).unwrap();
        mgr.append_message(serde_json::json!({"role":"user","content":"q_after"})).unwrap();
        mgr.append_message(serde_json::json!({"role":"assistant","content":"a_after"})).unwrap();

        let cfg = ContextConfig::default();
        let state = init_context_state(&mgr, &cfg, "sys").unwrap();
        assert_eq!(state.user_turns_list.len(), 2);
        if let TurnEntry::SummaryTurn { summary } = &state.user_turns_list[0] {
            assert_eq!(summary, "summary of old turns");
        } else {
            panic!("first turn should be SummaryTurn");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_context_from_state_flattens_turns() {
        let state = ContextState {
            user_turns_list: vec![
                TurnEntry::SummaryTurn { summary: "summary".to_string() },
                TurnEntry::UserTurn {
                    messages: vec![
                        AgentMessage::User { text: "hello".to_string() },
                        AgentMessage::Assistant { text: "world".to_string(), tool_calls: vec![] },
                    ],
                },
            ],
            estimate_context_chars: 100,
            context_budget_chars: 1000,
            context_budget_tokens: 250,
            last_api_usage: None,
            post_usage_appended_chars: 0,
            compaction_consecutive_failures: 0,
        };
        let msgs = build_context_from_state(&state);
        assert_eq!(msgs.len(), 3);
        assert!(matches!(&msgs[0], AgentMessage::CompactionSummary { .. }));
        assert!(matches!(&msgs[1], AgentMessage::User { .. }));
        assert!(matches!(&msgs[2], AgentMessage::Assistant { .. }));
    }

    #[test]
    fn init_context_state_no_session() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());

        let cfg = ContextConfig::default();
        let state = init_context_state(&mgr, &cfg, "sys").unwrap();
        assert!(state.user_turns_list.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_context_state_boundary_discards_prior() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();

        mgr.append_message(serde_json::json!({"role":"user","content":"old q1"})).unwrap();
        mgr.append_message(serde_json::json!({"role":"assistant","content":"old a1"})).unwrap();
        mgr.append_message(serde_json::json!({"role":"user","content":"old q2"})).unwrap();
        mgr.append_message(serde_json::json!({"role":"assistant","content":"old a2"})).unwrap();

        let path = mgr.current_transcript_path().unwrap().unwrap();
        let boundary_entry = TranscriptEntry::Compaction(CompactionEntry {
            id: None,
            parent_id: None,
            timestamp: "2026-01-01T00:00:00.000Z".to_string(),
            summary: Some("boundary summary".to_string()),
            covered_start_id: None,
            covered_end_id: None,
            covered_count: Some(2),
            is_boundary: Some(true),
        });
        super::super::transcript::append_entry(&path, &boundary_entry).unwrap();

        mgr.append_message(serde_json::json!({"role":"user","content":"new q"})).unwrap();
        mgr.append_message(serde_json::json!({"role":"assistant","content":"new a"})).unwrap();

        let cfg = ContextConfig::default();
        let state = init_context_state(&mgr, &cfg, "sys").unwrap();

        assert_eq!(state.user_turns_list.len(), 2, "boundary + 1 new turn");

        let has_boundary_summary = state.user_turns_list.iter().any(|t| {
            matches!(t, TurnEntry::SummaryTurn { summary } if summary == "boundary summary")
        });
        assert!(has_boundary_summary, "should contain boundary summary");

        let has_old = state.user_turns_list.iter().any(|t| {
            if let TurnEntry::UserTurn { messages } = t {
                messages.iter().any(|m| {
                    matches!(m, AgentMessage::User { text } if text.contains("old"))
                })
            } else {
                false
            }
        });
        assert!(!has_old, "old turns before boundary should be discarded");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_context_state_non_boundary_compaction_preserves_prior() {
        let dir = temp_sessions_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(dir.clone());
        let key = mgr.current_session_key();
        mgr.create_session(key, None).unwrap();

        mgr.append_message(serde_json::json!({"role":"user","content":"q1"})).unwrap();
        mgr.append_message(serde_json::json!({"role":"assistant","content":"a1"})).unwrap();
        mgr.append_compaction(Some("non-boundary summary")).unwrap();
        mgr.append_message(serde_json::json!({"role":"user","content":"q2"})).unwrap();

        let cfg = ContextConfig::default();
        let state = init_context_state(&mgr, &cfg, "sys").unwrap();

        assert!(state.user_turns_list.len() >= 3, "should preserve pre-compaction turn + summary + post turn");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
