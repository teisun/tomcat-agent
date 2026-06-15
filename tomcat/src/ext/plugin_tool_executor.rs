use crate::core::tools::contract::registry::{Tool, ToolExecutor};
use crate::ext::{HostApiDispatcher, PluginManager};
use crate::infra::error::AppError;
use crate::infra::wire;
use async_trait::async_trait;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

pub struct PluginToolExecutor {
    plugin_manager: Weak<PluginManager>,
    dispatcher: Mutex<Weak<HostApiDispatcher>>,
    timeout: Duration,
    next_call_id: AtomicU64,
}

impl PluginToolExecutor {
    pub fn new(plugin_manager: Weak<PluginManager>) -> Arc<Self> {
        Arc::new(Self {
            plugin_manager,
            dispatcher: Mutex::new(Weak::new()),
            timeout: Duration::from_secs(120),
            next_call_id: AtomicU64::new(1),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_timeout(
        plugin_manager: Weak<PluginManager>,
        timeout: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            plugin_manager,
            dispatcher: Mutex::new(Weak::new()),
            timeout,
            next_call_id: AtomicU64::new(1),
        })
    }

    pub fn attach_dispatcher(&self, dispatcher: Weak<HostApiDispatcher>) {
        *self.dispatcher.lock() = dispatcher;
    }

    fn alloc_call_id(&self, session_id: &str, plugin_id: &str, tool_name: &str) -> String {
        let seq = self.next_call_id.fetch_add(1, Ordering::Relaxed);
        format!("{session_id}/{plugin_id}/{tool_name}/{seq}")
    }
}

#[async_trait]
impl ToolExecutor for PluginToolExecutor {
    async fn execute(
        &self,
        tool: &Tool,
        params: serde_json::Value,
        caller_plugin_id: &str,
        session_id: Option<&str>,
    ) -> Result<serde_json::Value, AppError> {
        let session_id = session_id
            .ok_or_else(|| AppError::Tool(format!("插件工具执行缺少 session_id: {}", tool.name)))?;
        let plugin_manager = self
            .plugin_manager
            .upgrade()
            .ok_or_else(|| AppError::Plugin("plugin manager unavailable".to_string()))?;
        let dispatcher = self
            .dispatcher
            .lock()
            .upgrade()
            .ok_or_else(|| AppError::Plugin("host dispatcher unavailable".to_string()))?;

        let _handle = plugin_manager
            .start_session_vm(session_id, &tool.plugin_id)
            .await?;

        let call_id = self.alloc_call_id(session_id, &tool.plugin_id, &tool.name);
        let rx = dispatcher.register_command_waiter(&call_id);
        plugin_manager.dispatch_session_event(
            session_id,
            &tool.plugin_id,
            wire::vm::WIRE_COMMAND_INVOKE,
            serde_json::json!({
                "kind": "tool",
                "callId": call_id,
                "toolName": tool.name,
                "params": params,
                "callerPluginId": caller_plugin_id,
            }),
            serde_json::json!({
                "sessionId": session_id,
                "toolName": tool.name,
                "callerPluginId": caller_plugin_id,
            }),
        )?;

        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(message))) => Err(AppError::Tool(message)),
            Ok(Err(_closed)) => Err(AppError::Plugin(format!(
                "插件工具执行结果通道关闭: {}",
                tool.name
            ))),
            Err(_) => {
                dispatcher.drop_command_waiter(&call_id);
                Err(AppError::Tool(format!("插件工具执行超时: {}", tool.name)))
            }
        }
    }
}
