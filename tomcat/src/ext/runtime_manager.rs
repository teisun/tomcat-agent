//! # VM 运行时管理器
//!
//! 以 `session_id + plugin_id` 双键管理 `VmActorHandle`，
//! 支持多会话隔离、lazy init 和 session 级批量清理。

use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::vm_actor::VmActorHandle;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

struct RuntimeEntry {
    handle: VmActorHandle,
    last_used_ms: AtomicU64,
}

impl RuntimeEntry {
    fn new(handle: VmActorHandle) -> Self {
        Self {
            handle,
            last_used_ms: AtomicU64::new(now_ms()),
        }
    }

    fn touch(&self) {
        self.last_used_ms.store(now_ms(), Ordering::Relaxed);
    }

    fn idle_for(&self, now_ms: u64) -> Duration {
        Duration::from_millis(now_ms.saturating_sub(self.last_used_ms.load(Ordering::Relaxed)))
    }
}

/// VM 实例的唯一标识：会话 + 插件。
#[derive(Debug, Hash, Eq, PartialEq, Clone)]
pub struct VmRuntimeKey {
    pub session_id: String,
    pub plugin_id: String,
}

impl VmRuntimeKey {
    pub fn new(session_id: impl Into<String>, plugin_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            plugin_id: plugin_id.into(),
        }
    }
}

impl std::fmt::Display for VmRuntimeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.session_id, self.plugin_id)
    }
}

/// 管理所有活跃的 VM actor handle，按 `VmRuntimeKey` 索引。
pub struct RuntimeManager {
    handles: DashMap<VmRuntimeKey, RuntimeEntry>,
}

impl RuntimeManager {
    pub fn new() -> Self {
        Self {
            handles: DashMap::new(),
        }
    }

    pub fn get(&self, key: &VmRuntimeKey) -> Option<VmActorHandle> {
        self.handles.get(key).map(|entry| {
            entry.value().touch();
            entry.value().handle.clone()
        })
    }

    pub fn contains(&self, key: &VmRuntimeKey) -> bool {
        self.handles.contains_key(key)
    }

    pub fn insert(&self, key: VmRuntimeKey, handle: VmActorHandle) {
        self.handles.insert(key, RuntimeEntry::new(handle));
    }

    pub fn touch(&self, key: &VmRuntimeKey) -> bool {
        if let Some(entry) = self.handles.get(key) {
            entry.value().touch();
            true
        } else {
            false
        }
    }

    pub fn remove(&self, key: &VmRuntimeKey) -> Option<VmActorHandle> {
        self.handles.remove(key).map(|(_, entry)| entry.handle)
    }

    /// 移除指定 session 下的所有 VM actor，返回被移除的 handle 列表。
    pub fn remove_session(&self, session_id: &str) -> Vec<VmActorHandle> {
        let keys_to_remove: Vec<VmRuntimeKey> = self
            .handles
            .iter()
            .filter(|entry| entry.key().session_id == session_id)
            .map(|entry| entry.key().clone())
            .collect();

        keys_to_remove
            .into_iter()
            .filter_map(|k| self.handles.remove(&k).map(|(_, entry)| entry.handle))
            .collect()
    }

    /// 移除指定插件在所有 session 下的 VM actor，返回被移除的 key + handle。
    pub fn remove_plugin(&self, plugin_id: &str) -> Vec<(VmRuntimeKey, VmActorHandle)> {
        let keys_to_remove: Vec<VmRuntimeKey> = self
            .handles
            .iter()
            .filter(|entry| entry.key().plugin_id == plugin_id)
            .map(|entry| entry.key().clone())
            .collect();

        keys_to_remove
            .into_iter()
            .filter_map(|key| {
                self.handles
                    .remove(&key)
                    .map(|(_, entry)| (key, entry.handle))
            })
            .collect()
    }

    /// 回收空闲超过 TTL 的运行时实例。
    pub fn reap_idle(&self, ttl: Duration) -> Vec<(VmRuntimeKey, VmActorHandle)> {
        let now = now_ms();
        let keys_to_remove: Vec<VmRuntimeKey> = self
            .handles
            .iter()
            .filter(|entry| entry.value().idle_for(now) >= ttl)
            .map(|entry| entry.key().clone())
            .collect();

        keys_to_remove
            .into_iter()
            .filter_map(|key| {
                self.handles
                    .remove(&key)
                    .map(|(_, entry)| (key, entry.handle))
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.handles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }
}

impl Default for RuntimeManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 线程安全的共享 RuntimeManager。
pub type SharedRuntimeManager = Arc<RuntimeManager>;
