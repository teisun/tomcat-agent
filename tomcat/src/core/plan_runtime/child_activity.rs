//! A child-local tap: raw events still reach the ordinary session bus. The
//! parent gets only a short activity label, not the raw arguments or output.
use crate::core::agent_registry::AgentRegistry;
use crate::infra::{event_bus::EventCallback, EventBus, EventContext, EventListenerId};
use std::sync::Arc;
pub(super) struct ChildActivity {
    pub bus: Arc<dyn EventBus>,
    pub registry: Arc<AgentRegistry>,
    pub parent: String,
    pub child: String,
}
fn activity(event: &str, payload: &serde_json::Value) -> Option<String> {
    let args = &payload["args"];
    let basename = || {
        args["path"]
            .as_str()?
            .trim_end_matches(['/', '\\'])
            .rsplit(['/', '\\'])
            .next()
            .filter(|s| !s.is_empty())
    };
    Some(match event {
        "turn_start" => "Thinking…".into(),
        "auto_compaction_start" | "foreground_compaction_start" => "Compacting context…".into(),
        "tool_execution_start" => match payload["toolName"].as_str() {
            Some("read") => {
                if let Some(paths) = args["paths"].as_array() {
                    format!("Reading {} files", paths.len())
                } else {
                    basename()
                        .map_or_else(|| "Reading files…".into(), |name| format!("Reading {name}"))
                }
            }
            Some("list_dir") => basename().map_or_else(
                || "Listing directory…".into(),
                |name| format!("Listing {name}"),
            ),
            Some("search_files") => args["pattern"].as_str().map_or_else(
                || "Searching workspace…".into(),
                |pattern| {
                    let mut chars = pattern.chars();
                    let mut excerpt: String = chars
                        .by_ref()
                        .take(40)
                        .map(|c| if c.is_control() { ' ' } else { c })
                        .collect();
                    if chars.next().is_some() {
                        excerpt.push('…');
                    }
                    format!("Searching \"{excerpt}\"")
                },
            ),
            Some("bash") => "Checking workspace…".into(),
            Some("todos") => "Updating checklist…".into(),
            _ => "Using tool…".into(),
        },
        _ => return None,
    })
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
            if let Some(phase) = activity(n, &c.payload) {
                self.registry
                    .record_activity(&self.parent, &self.child, &phase);
            }
        }
        self.bus.emit_sync(n, c)
    }
    async fn emit_async(&self, n: &str, c: EventContext) -> Result<(), crate::AppError> {
        self.emit_sync(n, c)
    }
}

#[cfg(test)]
mod tests {
    use super::activity;
    use serde_json::json;
    #[test]
    fn activity_summarizes_selected_args_and_ignores_message_end() {
        let label = |tool, args| {
            activity(
                "tool_execution_start",
                &json!({"toolName":tool,"args":args}),
            )
            .unwrap()
        };
        assert_eq!(
            label("read", json!({"path":"src/example.rs"})),
            "Reading example.rs"
        );
        assert_eq!(
            label("read", json!({"paths":[{"path":"one"},{"path":"two"}]})),
            "Reading 2 files"
        );
        assert_eq!(
            label("list_dir", json!({"path":r"C:\src\ui\"})),
            "Listing ui"
        );
        assert_eq!(
            label("search_files", json!({"pattern":"needle"})),
            "Searching \"needle\""
        );
        assert_eq!(
            label("search_files", json!({"pattern":"查".repeat(41)})),
            format!("Searching \"{}…\"", "查".repeat(40))
        );
        assert_eq!(activity("message_end", &json!({})), None);
    }
}
