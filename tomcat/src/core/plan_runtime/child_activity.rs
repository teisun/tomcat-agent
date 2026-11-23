//! A child-local tap: raw events still reach the ordinary session bus. The
//! parent gets only a change of phase, never thinking text, arguments or output.
use crate::core::agent_registry::AgentRegistry;
use crate::infra::{event_bus::EventCallback, EventBus, EventContext, EventListenerId};
use std::sync::Arc;
pub(super) struct ChildActivity {
    pub bus: Arc<dyn EventBus>,
    pub registry: Arc<AgentRegistry>,
    pub parent: String,
    pub child: String,
}
#[async_trait::async_trait]
impl EventBus for ChildActivity {
    fn on(&self, n: &str, cb: EventCallback) -> EventListenerId {
        self.bus.on(n, cb)
    }
    fn once(&self, n: &str, cb: EventCallback) -> EventListenerId {
        self.bus.once(n, cb)
    }
    fn on_plugin(&self, n: &str, p: &str, cb: EventCallback) -> EventListenerId {
        self.bus.on_plugin(n, p, cb)
    }
    fn once_plugin(&self, n: &str, p: &str, cb: EventCallback) -> EventListenerId {
        self.bus.once_plugin(n, p, cb)
    }
    fn off(&self, id: EventListenerId) {
        self.bus.off(id)
    }
    fn remove_plugin_listeners(&self, p: &str) {
        self.bus.remove_plugin_listeners(p)
    }
    fn emit_sync(&self, n: &str, c: EventContext) -> Result<(), crate::AppError> {
        if c.session_id.as_deref() == Some(self.child.as_str()) {
            let phase = match n {
                "turn_start" => Some("Thinking…"),
                "auto_compaction_start" | "foreground_compaction_start" => {
                    Some("Compacting context…")
                }
                "message_end" => Some("Preparing results…"),
                "tool_execution_start" => Some(match c.payload["toolName"].as_str() {
                    Some("read") => "Reading files…",
                    Some("search_files" | "list_dir") => "Searching workspace…",
                    Some("bash") => "Checking workspace…",
                    Some("todos") => "Updating checklist…",
                    _ => "Using tool…",
                }),
                _ => None,
            };
            if let Some(phase) = phase {
                self.registry
                    .record_activity(&self.parent, &self.child, phase);
            }
        }
        self.bus.emit_sync(n, c)
    }
    async fn emit_async(&self, n: &str, c: EventContext) -> Result<(), crate::AppError> {
        self.emit_sync(n, c)
    }
}
