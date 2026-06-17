//! # VM 运行时管理器
//!
//! 以 `session_id + plugin_id` 双键管理 `VmActorHandle`，
//! 支持多会话隔离、lazy init 和 session 级批量清理。

use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::runtime::DEFAULT_PLUGIN_IDLE_TTL_MS;
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

/// 插件 VM 实例的唯一标识：会话 + 插件。
#[derive(Debug, Hash, Eq, PartialEq, Clone)]
pub struct PluginRuntimeKey {
    pub session_id: String,
    pub plugin_id: String,
}

impl PluginRuntimeKey {
    pub fn new(session_id: impl Into<String>, plugin_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            plugin_id: plugin_id.into(),
        }
    }
}

impl std::fmt::Display for PluginRuntimeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.session_id, self.plugin_id)
    }
}

/// 管理所有活跃的 VM actor handle，按 `PluginRuntimeKey` 索引。
pub struct PluginRuntimeManager {
    handles: DashMap<PluginRuntimeKey, RuntimeEntry>,
    idle_ttl: Duration,
}

impl PluginRuntimeManager {
    pub fn new() -> Self {
        Self::with_idle_ttl(Duration::from_millis(DEFAULT_PLUGIN_IDLE_TTL_MS))
    }

    pub fn with_idle_ttl(idle_ttl: Duration) -> Self {
        Self {
            handles: DashMap::new(),
            idle_ttl,
        }
    }

    pub fn get(&self, key: &PluginRuntimeKey) -> Option<VmActorHandle> {
        self.handles.get(key).map(|entry| {
            entry.value().touch();
            entry.value().handle.clone()
        })
    }

    pub fn contains(&self, key: &PluginRuntimeKey) -> bool {
        self.handles.contains_key(key)
    }

    pub fn insert(&self, key: PluginRuntimeKey, handle: VmActorHandle) {
        self.handles.insert(key, RuntimeEntry::new(handle));
    }

    pub fn touch(&self, key: &PluginRuntimeKey) -> bool {
        if let Some(entry) = self.handles.get(key) {
            entry.value().touch();
            true
        } else {
            false
        }
    }

    pub fn remove(&self, key: &PluginRuntimeKey) -> Option<VmActorHandle> {
        self.handles.remove(key).map(|(_, entry)| entry.handle)
    }

    /// 移除指定 session 下的所有 VM actor，返回被移除的 key + handle 列表。
    pub fn remove_session_entries(
        &self,
        session_id: &str,
    ) -> Vec<(PluginRuntimeKey, VmActorHandle)> {
        let keys_to_remove: Vec<PluginRuntimeKey> = self
            .handles
            .iter()
            .filter(|entry| entry.key().session_id == session_id)
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

    /// 移除指定 session 下的所有 VM actor，返回被移除的 handle 列表。
    pub fn remove_session(&self, session_id: &str) -> Vec<VmActorHandle> {
        self.remove_session_entries(session_id)
            .into_iter()
            .map(|(_, handle)| handle)
            .collect()
    }

    /// 移除指定插件在所有 session 下的 VM actor，返回被移除的 key + handle。
    pub fn remove_plugin(&self, plugin_id: &str) -> Vec<(PluginRuntimeKey, VmActorHandle)> {
        let keys_to_remove: Vec<PluginRuntimeKey> = self
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
    pub fn reap_idle(&self, ttl: Duration) -> Vec<(PluginRuntimeKey, VmActorHandle)> {
        let now = now_ms();
        let keys_to_remove: Vec<PluginRuntimeKey> = self
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

    /// 按当前管理器配置的 TTL 机会式回收空闲运行时。
    pub fn reap_configured_idle(&self) -> Vec<(PluginRuntimeKey, VmActorHandle)> {
        if self.idle_ttl.is_zero() {
            return Vec::new();
        }
        self.reap_idle(self.idle_ttl)
    }

    pub fn configured_idle_ttl(&self) -> Duration {
        self.idle_ttl
    }

    pub fn len(&self) -> usize {
        self.handles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }
}

impl Default for PluginRuntimeManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 线程安全的共享插件运行时管理器。
pub type SharedPluginRuntimeManager = Arc<PluginRuntimeManager>;
