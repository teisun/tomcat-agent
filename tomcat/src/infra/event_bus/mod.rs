//! # 全局事件总线 (Event Bus)
//!
//! 全进程共享的发布/订阅总线：业务侧（agent_loop / chat / dispatcher）emit
//! 事件，扩展侧（pi-mono / 插件 / TUI / 审计）on 订阅；与 [`super::events`]
//! 的 enum 配合形成"强类型 emit + 字符串 subscribe"的混合契约。
//!
//! ## 五个公共入口
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                                                                          │
//! │  on(event, cb)         ─► 持久订阅，返回 EventListenerId                 │
//! │  once(event, cb)       ─► 单次订阅，触发后自动 off                       │
//! │  off(listener_id)      ─► 显式注销                                       │
//! │  emit_sync(event, ctx) ─► 立即按 priority 降序逐个调用回调（不阻塞 actor）│
//! │  emit_async(event, ctx)─► 当前实现转调 emit_sync（保留异步签名以便后续  │
//! │                            升级到 mpsc + spawn 模型，不破坏调用方）       │
//! │  remove_plugin_listeners(plugin_id) ─► 插件卸载兜底，按 plugin_id 批量 off│
//! │                                                                          │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## 内部数据结构（双表）
//!
//! ```text
//! ┌─ DefaultEventBus ───────────────────────────────────────────────────────┐
//! │                                                                          │
//! │  next_id: AtomicU64                  ── 单调递增，listener_id 全局唯一  │
//! │                                                                          │
//! │  listeners: RwLock<HashMap<                                              │
//! │      event_name: String,              ── snake_case，与 wire 常量一致   │
//! │      Vec<ListenerEntry { id, plugin_id, priority, once, callback }>     │
//! │  >>                                                                      │
//! │                                                                          │
//! │  id_to_event: RwLock<HashMap<EventListenerId, event_name>>               │
//! │  ── off 时不需要 O(N×M) 扫全表，O(1) 反查事件名再单 Vec 内 retain        │
//! │                                                                          │
//! └─────────────────────────────────────────────────────────────────────────┘
//!
//!         emit_sync(event, ctx)
//!              │
//!              │ ① RwLock::read(listeners)
//!              │ ② 取 listeners[event] 并按 priority 降序排
//!              │ ③ 逐个 catch_unwind(|| cb(ctx.clone()))
//!              │     ├─ Ok(Ok(_))  ► 继续下一个
//!              │     ├─ Ok(Err(e)) ► tracing::warn 单条事件错，继续下一个
//!              │     └─ Err(_)     ► panic 被吞，tracing::warn，继续下一个
//!              │ ④ 删除 once==true 的项
//!              ▼
//!         Result<(), AppError>  ── 永远 Ok，单 listener 失败不外抛
//! ```
//!
//! ## 不变量
//!
//! - **隔离性**：单 listener panic / Err 不中断其余回调，业务主流程绝不被订阅者
//!   拖慢或抛错（`emit_sync` 内部用 `catch_unwind`）。
//! - **优先级**：`priority` 大者先执行，相同 priority 按注册顺序——保证渲染层
//!   能比审计层先消费 `MessageUpdate`，UI 不滞后。
//! - **插件卸载兜底**：`remove_plugin_listeners` 配合 [`crate::ext::plugin::PluginManager::unload_plugin`]
//!   保证插件 listener 不残留为僵尸订阅。
//!
//! ## 与 [`super::events`] 的边界
//!
//! - `events/mod.rs`：定义事件 **类型** 与 **wire 字面量**。
//! - 本文件：负责 **运行时分发**（订阅表 + 触发循环 + panic 隔离）。
//! - 业务侧 emit 流程：构造 `AgentEvent::Foo` → `serde_json::to_value` →
//!   `EventContext::new(WIRE_FOO, payload)` → `event_bus.emit_sync(WIRE_FOO, ctx)`。

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use tracing::warn;

use super::error::AppError;

/// 监听器唯一 ID，用于 [`EventBus::off`] 注销。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventListenerId(pub u64);

/// 事件上下文，在触发时传递给回调；可携带事件名、payload、来源插件与优先级。
#[derive(Debug, Clone)]
pub struct EventContext {
    pub event_name: String,
    pub payload: serde_json::Value,
    pub plugin_id: Option<String>,
    pub priority: i32,
}

impl EventContext {
    /// 构造事件上下文，`plugin_id` 与 `priority` 默认为 None/0，可用 `with_plugin_id`/`with_priority` 链式设置。
    pub fn new(event_name: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            event_name: event_name.into(),
            payload,
            plugin_id: None,
            priority: 0,
        }
    }

    /// 设置来源插件 ID，便于 [`EventBus::remove_plugin_listeners`] 按插件清理。
    pub fn with_plugin_id(mut self, plugin_id: impl Into<String>) -> Self {
        self.plugin_id = Some(plugin_id.into());
        self
    }

    /// 设置优先级，数值越大越先执行。
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }
}

/// 事件回调类型：接收 [`EventContext`]，返回 [`Result`]；需满足 `Send + Sync` 以在跨线程事件总线中使用。
pub type EventCallback = Box<dyn FnMut(EventContext) -> Result<(), AppError> + Send + Sync>;

struct ListenerEntry {
    id: EventListenerId,
    plugin_id: Option<String>,
    priority: i32,
    once: bool,
    callback: EventCallback,
}

/// 全局事件总线 trait：注册/注销监听、同步/异步触发、按插件清理。单 listener 错误或 panic 仅记录日志，不中断其余回调。
#[async_trait]
pub trait EventBus: Send + Sync + 'static {
    /// 注册持久监听，返回用于 [`EventBus::off`] 的 ID。
    fn on(&self, event_name: &str, callback: EventCallback) -> EventListenerId;
    /// 注册单次监听，触发一次后自动移除。
    fn once(&self, event_name: &str, callback: EventCallback) -> EventListenerId;
    /// 按 ID 移除监听器。
    fn off(&self, listener_id: EventListenerId);
    /// 同步触发事件，按 priority 降序执行回调；不因单个回调返回 Err 或 panic 而返回 Err。
    fn emit_sync(&self, event_name: &str, context: EventContext) -> Result<(), AppError>;
    /// 异步触发，当前实现内部调用 emit_sync。
    async fn emit_async(&self, event_name: &str, context: EventContext) -> Result<(), AppError>;
    /// 移除指定插件注册的所有监听，插件卸载时调用以防泄漏。
    fn remove_plugin_listeners(&self, plugin_id: &str);
}

/// 默认事件总线实现，基于 `RwLock` + `HashMap`，线程安全。
pub struct DefaultEventBus {
    next_id: AtomicU64,
    listeners: RwLock<HashMap<String, Vec<ListenerEntry>>>,
    id_to_event: RwLock<HashMap<EventListenerId, String>>,
}

impl Default for DefaultEventBus {
    fn default() -> Self {
        Self {
            next_id: AtomicU64::new(0),
            listeners: RwLock::new(HashMap::new()),
            id_to_event: RwLock::new(HashMap::new()),
        }
    }
}

impl DefaultEventBus {
    pub fn new() -> Self {
        Self::default()
    }

    fn next_id(&self) -> EventListenerId {
        EventListenerId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// 供插件注册时传入 `plugin_id`，便于 [`EventBus::remove_plugin_listeners`] 清理。
    ///
    /// # Arguments
    /// * `event_name` - 事件名（字符串，与 pi-mono 一致）
    /// * `once` - 是否仅触发一次后移除
    /// * `plugin_id` - 可选插件 ID，卸载时用于批量移除
    /// * `priority` - 优先级，数值越大越先执行
    /// * `callback` - 回调
    pub fn add_listener(
        &self,
        event_name: &str,
        once: bool,
        plugin_id: Option<String>,
        priority: i32,
        callback: EventCallback,
    ) -> EventListenerId {
        let id = self.next_id();
        let entry = ListenerEntry {
            id,
            plugin_id: plugin_id.clone(),
            priority,
            once,
            callback,
        };
        {
            let mut listeners = self.listeners.write();
            listeners
                .entry(event_name.to_string())
                .or_default()
                .push(entry);
        }
        {
            let mut id_to_event = self.id_to_event.write();
            id_to_event.insert(id, event_name.to_string());
        }
        id
    }
}

#[async_trait]
impl EventBus for DefaultEventBus {
    fn on(&self, event_name: &str, callback: EventCallback) -> EventListenerId {
        self.add_listener(event_name, false, None, 0, callback)
    }

    fn once(&self, event_name: &str, callback: EventCallback) -> EventListenerId {
        self.add_listener(event_name, true, None, 0, callback)
    }

    fn off(&self, listener_id: EventListenerId) {
        let event_name = {
            let mut id_to_event = self.id_to_event.write();
            id_to_event.remove(&listener_id)
        };
        if let Some(name) = event_name {
            let mut listeners = self.listeners.write();
            if let Some(vec) = listeners.get_mut(&name) {
                vec.retain(|e| e.id != listener_id);
            }
        }
    }

    fn emit_sync(&self, event_name: &str, context: EventContext) -> Result<(), AppError> {
        let mut to_remove: Vec<(usize, EventListenerId)> = Vec::new();
        {
            let mut listeners = self.listeners.write();
            let vec = match listeners.get_mut(event_name) {
                Some(v) => v,
                None => return Ok(()),
            };
            vec.sort_by_key(|e| std::cmp::Reverse(e.priority));
            for (i, entry) in vec.iter_mut().enumerate() {
                let ctx = context.clone();
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    (entry.callback)(ctx)
                }));
                match res {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => warn!("event listener error: {} {:?}", event_name, e),
                    Err(panic) => warn!("event listener panic: {} {:?}", event_name, panic),
                }
                if entry.once {
                    to_remove.push((i, entry.id));
                }
            }
            for (idx, _) in to_remove.iter().copied().rev() {
                vec.remove(idx);
            }
        }
        let mut id_to_event = self.id_to_event.write();
        for (_, id) in to_remove {
            id_to_event.remove(&id);
        }
        Ok(())
    }

    async fn emit_async(&self, event_name: &str, context: EventContext) -> Result<(), AppError> {
        self.emit_sync(event_name, context)
    }

    fn remove_plugin_listeners(&self, plugin_id: &str) {
        let mut ids_to_remove = Vec::new();
        {
            let mut listeners = self.listeners.write();
            for vec in listeners.values_mut() {
                let mut to_remove = Vec::new();
                for (i, e) in vec.iter().enumerate() {
                    if e.plugin_id.as_deref() == Some(plugin_id) {
                        to_remove.push((i, e.id));
                    }
                }
                for (i, id) in to_remove.into_iter().rev() {
                    vec.remove(i);
                    ids_to_remove.push(id);
                }
            }
        }
        let mut id_to_event = self.id_to_event.write();
        for id in ids_to_remove {
            id_to_event.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests;
