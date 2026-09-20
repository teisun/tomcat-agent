//! 会话元数据 store（sessions.json）：`sessions{id→entry}` + `current{key→id}` 的读写与持久化。
//!
//! 列表与路由由此提供；原子写通过「写临时文件 → 重命名」保证。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 用户显式选择并由宿主验证的项目根；与运行 cwd 分开持久化。
    pub project_root: Option<String>,
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
    /// 最近一次 turn 末观察到的上下文利用率，供 `get_state` / reload UI 恢复。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_utilization_ratio: Option<f64>,
    /// 最近一次会话级 restore 成功落到的 checkpoint（仅 TurnEnd/Interrupt）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checkpoint_id: Option<String>,
    /// 会话标题：首条 user message 首行截断 ≤40 字符生成一次、持久化、永不覆盖。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// 从路径加载 SessionStore。缺失文件只返回内存中的空 store；现存空白或损坏文件
/// 直接报错，读取绝不重写用户的 sessions.json。
pub fn load_store(path: &Path) -> Result<SessionStore, AppError> {
    let content = match read_file_utf8(path) {
        Ok(content) => content,
        Err(AppError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SessionStore::new());
        }
        Err(error) => return Err(error),
    };
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config(format!(
            "会话存储为空，拒绝覆盖: {}",
            path.display()
        )));
    }
    let mut store: SessionStore = serde_json::from_str(trimmed).map_err(|error| {
        AppError::Config(format!(
            "会话存储无法解析，拒绝覆盖 {}: {error}",
            path.display()
        ))
    })?;
    repair_missing_session_keys(&mut store);
    prune_stale_current_pointers(&mut store);
    Ok(store)
}

/// 原子写入 SessionStore 到 path（临时文件 + rename）。
pub fn save_store(path: &Path, store: &SessionStore) -> Result<(), AppError> {
    let content = serde_json::to_string_pretty(store)?;
    write_file_atomic(path, content.as_bytes())
}

/// 预检用于模型删除：现存 store 必须是可安全定位 `sessions.*.modelOverride` 的 JSON。
/// 缺失 store 表示还没有会话，不创建文件。
pub(crate) fn precheck_model_override_store(path: &Path) -> Result<(), AppError> {
    with_store_write_lock(path, || read_store_document(path).map(|_| ()))
}

/// 只删除等于目标模型的 session override，保留 current、标题、用量以及未知 JSON 字段。
///
/// 这条路径不能反序列化后再用 `SessionStore` 整体序列化，因为未来版本写入的未知字段
/// 会在那种读改写中被静默丢掉。
pub(crate) fn clear_model_overrides_in_store(
    path: &Path,
    model_id: &str,
) -> Result<usize, AppError> {
    with_store_write_lock(path, || {
        let Some(mut document) = read_store_document(path)? else {
            return Ok(0);
        };
        let sessions = document
            .as_object_mut()
            .and_then(|root| root.get_mut("sessions"))
            .and_then(serde_json::Value::as_object_mut)
            .expect("read_store_document validates sessions object");
        let mut cleared = 0;
        for (session_id, entry) in sessions.iter_mut() {
            let entry = entry
                .as_object_mut()
                .expect("read_store_document validates session entry");
            let matches = match entry.get("modelOverride") {
                Some(value) => {
                    value.as_str().ok_or_else(|| {
                        AppError::Config(format!(
                            "会话存储中的 modelOverride 必须是字符串: {} (session {session_id})",
                            path.display()
                        ))
                    })? == model_id
                }
                None => false,
            };
            if matches {
                entry.remove("modelOverride");
                cleared += 1;
            }
        }
        if cleared > 0 {
            let content = serde_json::to_string_pretty(&document)?;
            write_file_atomic(path, content.as_bytes())?;
        }
        Ok(cleared)
    })
}

fn read_store_document(path: &Path) -> Result<Option<serde_json::Value>, AppError> {
    let content = match read_file_utf8(path) {
        Ok(content) => content,
        Err(AppError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None)
        }
        Err(error) => return Err(error),
    };
    if content.trim().is_empty() {
        return Err(AppError::Config(format!(
            "会话存储为空，拒绝覆盖: {}",
            path.display()
        )));
    }
    let document: serde_json::Value = serde_json::from_str(&content).map_err(|error| {
        AppError::Config(format!(
            "会话存储无法解析，拒绝覆盖 {}: {error}",
            path.display()
        ))
    })?;
    let root = document
        .as_object()
        .ok_or_else(|| AppError::Config(format!("会话存储根节点必须是对象: {}", path.display())))?;
    let sessions = root
        .get("sessions")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            AppError::Config(format!("会话存储缺少 sessions 对象: {}", path.display()))
        })?;
    if root
        .get("current")
        .is_some_and(|current| !current.is_object())
    {
        return Err(AppError::Config(format!(
            "会话存储 current 必须是对象: {}",
            path.display()
        )));
    }
    for (session_id, entry) in sessions {
        if !entry.is_object() {
            return Err(AppError::Config(format!(
                "会话存储 session 条目必须是对象: {} (session {session_id})",
                path.display()
            )));
        }
    }
    Ok(Some(document))
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

fn store_mutation_locks() -> &'static Mutex<HashMap<PathBuf, Arc<Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn normalized_store_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    })
}

/// 同一进程内所有指向同一 sessions.json 的读改写操作共用一把锁。
///
/// 这不是跨进程协议：它只避免两个 SessionManager 或一次模型清理互相覆盖
/// 同一个完整 JSON 文件里的标题、用量等无关字段。
pub(crate) fn with_store_write_lock<T>(
    path: &Path,
    work: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    let key = normalized_store_path(path);
    let lock = {
        let mut locks = store_mutation_locks()
            .lock()
            .map_err(|error| AppError::Config(format!("会话存储锁注册表异常: {error}")))?;
        Arc::clone(locks.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))))
    };
    let _guard = lock
        .lock()
        .map_err(|error| AppError::Config(format!("会话存储写入锁异常: {error}")))?;
    work()
}
