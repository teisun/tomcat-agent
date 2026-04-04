//! # VM 运行时管理器
//!
//! 以 `session_id + plugin_id` 双键管理 `VmActorHandle`，
//! 支持多会话隔离、lazy init 和 session 级批量清理。

use dashmap::DashMap;
use std::sync::Arc;

use super::vm_actor::VmActorHandle;

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
    handles: DashMap<VmRuntimeKey, VmActorHandle>,
}

impl RuntimeManager {
    pub fn new() -> Self {
        Self {
            handles: DashMap::new(),
        }
    }

    pub fn get(&self, key: &VmRuntimeKey) -> Option<VmActorHandle> {
        self.handles.get(key).map(|r| r.value().clone())
    }

    pub fn insert(&self, key: VmRuntimeKey, handle: VmActorHandle) {
        self.handles.insert(key, handle);
    }

    pub fn remove(&self, key: &VmRuntimeKey) -> Option<VmActorHandle> {
        self.handles.remove(key).map(|(_, h)| h)
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
            .filter_map(|k| self.handles.remove(&k).map(|(_, h)| h))
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

#[cfg(test)]
mod tests;
