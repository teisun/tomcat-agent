//! 全局事件总线：on/once/off/emit_sync/emit_async/remove_plugin_listeners。
//! 单 listener 抛错不中断其他 listener 与主流程。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use tracing::warn;

use crate::error::AppError;

/// 监听器 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventListenerId(pub u64);

/// 事件上下文，传递给回调。
#[derive(Debug, Clone)]
pub struct EventContext {
    pub event_name: String,
    pub payload: serde_json::Value,
    pub plugin_id: Option<String>,
    pub priority: i32,
}

impl EventContext {
    pub fn new(event_name: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            event_name: event_name.into(),
            payload,
            plugin_id: None,
            priority: 0,
        }
    }

    pub fn with_plugin_id(mut self, plugin_id: impl Into<String>) -> Self {
        self.plugin_id = Some(plugin_id.into());
        self
    }

    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }
}

pub type EventCallback = Box<dyn FnMut(EventContext) -> Result<(), AppError> + Send + Sync>;

struct ListenerEntry {
    id: EventListenerId,
    plugin_id: Option<String>,
    priority: i32,
    once: bool,
    callback: EventCallback,
}

#[async_trait]
pub trait EventBus: Send + Sync + 'static {
    fn on(&self, event_name: &str, callback: EventCallback) -> EventListenerId;
    fn once(&self, event_name: &str, callback: EventCallback) -> EventListenerId;
    fn off(&self, listener_id: EventListenerId);
    fn emit_sync(&self, event_name: &str, context: EventContext) -> Result<(), AppError>;
    async fn emit_async(&self, event_name: &str, context: EventContext) -> Result<(), AppError>;
    fn remove_plugin_listeners(&self, plugin_id: &str);
}

pub struct DefaultEventBus {
    next_id: AtomicU64,
    listeners: std::sync::RwLock<HashMap<String, Vec<ListenerEntry>>>,
    id_to_event: std::sync::RwLock<HashMap<EventListenerId, String>>,
}

impl Default for DefaultEventBus {
    fn default() -> Self {
        Self {
            next_id: AtomicU64::new(0),
            listeners: std::sync::RwLock::new(HashMap::new()),
            id_to_event: std::sync::RwLock::new(HashMap::new()),
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

    /// 供插件注册时传入 plugin_id，便于 remove_plugin_listeners 清理。
    pub fn add_listener(&self, event_name: &str, once: bool, plugin_id: Option<String>, priority: i32, callback: EventCallback) -> EventListenerId {
        let id = self.next_id();
        let entry = ListenerEntry {
            id,
            plugin_id: plugin_id.clone(),
            priority,
            once,
            callback,
        };
        {
            let mut listeners = self.listeners.write().unwrap();
            listeners
                .entry(event_name.to_string())
                .or_default()
                .push(entry);
        }
        {
            let mut id_to_event = self.id_to_event.write().unwrap();
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
            let mut id_to_event = self.id_to_event.write().unwrap();
            id_to_event.remove(&listener_id)
        };
        if let Some(name) = event_name {
            let mut listeners = self.listeners.write().unwrap();
            if let Some(vec) = listeners.get_mut(&name) {
                vec.retain(|e| e.id != listener_id);
            }
        }
    }

    fn emit_sync(&self, event_name: &str, context: EventContext) -> Result<(), AppError> {
        let mut to_remove: Vec<(usize, EventListenerId)> = Vec::new();
        {
            let mut listeners = self.listeners.write().unwrap();
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
        let mut id_to_event = self.id_to_event.write().unwrap();
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
            let mut listeners = self.listeners.write().unwrap();
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
        let mut id_to_event = self.id_to_event.write().unwrap();
        for id in ids_to_remove {
            id_to_event.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_returns_id_and_emit_sync_calls() {
        let bus = DefaultEventBus::new();
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let c = called.clone();
        let id = bus.on("test", Box::new(move |_ctx| {
            c.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }));
        let ctx = EventContext::new("test", serde_json::Value::Null);
        bus.emit_sync("test", ctx).unwrap();
        assert!(called.load(std::sync::atomic::Ordering::SeqCst));
        bus.off(id);
    }

    #[test]
    fn once_removes_after_emit() {
        let bus = DefaultEventBus::new();
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = count.clone();
        bus.once("once", Box::new(move |_| {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }));
        bus.emit_sync("once", EventContext::new("once", serde_json::Value::Null)).unwrap();
        bus.emit_sync("once", EventContext::new("once", serde_json::Value::Null)).unwrap();
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn single_listener_error_does_not_abort_others() {
        let bus = DefaultEventBus::new();
        let ok_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ok_c = ok_called.clone();
        bus.on("err", Box::new(move |_| Err(AppError::Event("fail".to_string()))));
        bus.on("err", Box::new(move |_| {
            ok_c.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }));
        let ctx = EventContext::new("err", serde_json::Value::Null);
        let _ = bus.emit_sync("err", ctx);
        assert!(ok_called.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn remove_plugin_listeners_removes_by_plugin_id() {
        let bus = DefaultEventBus::new();
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let c = called.clone();
        bus.add_listener("ev", false, Some("plugin_a".to_string()), 0, Box::new(move |_| {
            c.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }));
        bus.remove_plugin_listeners("plugin_a");
        bus.emit_sync("ev", EventContext::new("ev", serde_json::Value::Null)).unwrap();
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn off_removes_listener() {
        let bus = DefaultEventBus::new();
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let c = called.clone();
        let id = bus.on("off_test", Box::new(move |_| {
            c.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }));
        bus.off(id);
        bus.emit_sync("off_test", EventContext::new("off_test", serde_json::Value::Null)).unwrap();
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn priority_order_higher_first() {
        let bus = DefaultEventBus::new();
        let order = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i32>::new()));
        let o1 = order.clone();
        let o2 = order.clone();
        bus.add_listener("pri", false, None, 10, Box::new(move |_| {
            o1.lock().unwrap().push(10);
            Ok(())
        }));
        bus.add_listener("pri", false, None, 5, Box::new(move |_| {
            o2.lock().unwrap().push(5);
            Ok(())
        }));
        bus.emit_sync("pri", EventContext::new("pri", serde_json::Value::Null)).unwrap();
        let v = order.lock().unwrap().clone();
        assert_eq!(v, [10, 5]);
    }

    #[test]
    fn event_context_with_plugin_id_and_priority() {
        let ctx = EventContext::new("ev", serde_json::json!({}))
            .with_plugin_id("plugin-1")
            .with_priority(42);
        assert_eq!(ctx.plugin_id.as_deref(), Some("plugin-1"));
        assert_eq!(ctx.priority, 42);
    }

    #[test]
    fn emit_sync_empty_event_name_no_listeners() {
        let bus = DefaultEventBus::new();
        let ctx = EventContext::new("no_listeners", serde_json::Value::Null);
        bus.emit_sync("no_listeners", ctx).unwrap();
    }

    #[tokio::test]
    async fn emit_async_calls_listeners() {
        let bus = DefaultEventBus::new();
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let c = called.clone();
        bus.on("async_ev", Box::new(move |_| {
            c.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }));
        let ctx = EventContext::new("async_ev", serde_json::Value::Null);
        bus.emit_async("async_ev", ctx).await.unwrap();
        assert!(called.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn listener_panic_is_caught_others_still_run() {
        let bus = DefaultEventBus::new();
        let ok_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ok_c = ok_called.clone();
        bus.on("panic_ev", Box::new(move |_| panic!("listener panic")));
        bus.on("panic_ev", Box::new(move |_| {
            ok_c.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }));
        let ctx = EventContext::new("panic_ev", serde_json::Value::Null);
        bus.emit_sync("panic_ev", ctx).unwrap();
        assert!(ok_called.load(std::sync::atomic::Ordering::SeqCst));
    }
}
