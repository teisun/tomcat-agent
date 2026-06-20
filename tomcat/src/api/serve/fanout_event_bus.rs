//! `serve` 进程级 fanout event bus。
//!
//! 用于把共享 `AgentRegistry` 发出的子 agent 生命周期事件，按 `sessionId`
//! 精确转发回对应会话自己的 `EventBus`。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;

use crate::infra::{DefaultEventBus, EventBus, EventContext, EventListenerId};
use crate::AppError;

pub(crate) struct FanoutEventBus {
    local: DefaultEventBus,
    session_buses: RwLock<HashMap<String, Arc<dyn EventBus>>>,
}

impl FanoutEventBus {
    pub(crate) fn new() -> Self {
        Self {
            local: DefaultEventBus::new(),
            session_buses: RwLock::new(HashMap::new()),
        }
    }

    pub(crate) fn register_session_bus(&self, session_id: String, bus: Arc<dyn EventBus>) {
        self.session_buses.write().insert(session_id, bus);
    }

    pub(crate) fn unregister_session_bus(&self, session_id: &str) {
        self.session_buses.write().remove(session_id);
    }
}

#[async_trait]
impl EventBus for FanoutEventBus {
    fn on(&self, event_name: &str, callback: crate::infra::event_bus::EventCallback) -> EventListenerId {
        self.local.on(event_name, callback)
    }

    fn on_plugin(
        &self,
        event_name: &str,
        plugin_id: &str,
        callback: crate::infra::event_bus::EventCallback,
    ) -> EventListenerId {
        self.local.on_plugin(event_name, plugin_id, callback)
    }

    fn once(
        &self,
        event_name: &str,
        callback: crate::infra::event_bus::EventCallback,
    ) -> EventListenerId {
        self.local.once(event_name, callback)
    }

    fn once_plugin(
        &self,
        event_name: &str,
        plugin_id: &str,
        callback: crate::infra::event_bus::EventCallback,
    ) -> EventListenerId {
        self.local.once_plugin(event_name, plugin_id, callback)
    }

    fn off(&self, listener_id: EventListenerId) {
        self.local.off(listener_id);
    }

    fn emit_sync(&self, event_name: &str, context: EventContext) -> Result<(), AppError> {
        self.local.emit_sync(event_name, context.clone())?;
        if let Some(session_id) = context.session_id.as_deref() {
            if let Some(bus) = self.session_buses.read().get(session_id).cloned() {
                bus.emit_sync(event_name, context)?;
            }
        }
        Ok(())
    }

    async fn emit_async(&self, event_name: &str, context: EventContext) -> Result<(), AppError> {
        self.emit_sync(event_name, context)
    }

    fn remove_plugin_listeners(&self, plugin_id: &str) {
        self.local.remove_plugin_listeners(plugin_id);
    }
}
