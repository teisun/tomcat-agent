use crate::ext::plugin::RegisteredFunction;
use crate::ext::{EventEnvelope, HostApiDispatcher, PluginManager};
use crate::infra::error::AppError;
use crate::infra::i18n::tr;
use crate::infra::wire;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

struct CommandWaiterGuard<'a> {
    dispatcher: &'a HostApiDispatcher,
    call_id: &'a str,
}

impl Drop for CommandWaiterGuard<'_> {
    fn drop(&mut self) {
        self.dispatcher.drop_command_waiter(self.call_id);
    }
}

pub struct PluginFunctionInvoker {
    plugin_manager: Weak<PluginManager>,
    dispatcher: Mutex<Weak<HostApiDispatcher>>,
    timeout: Duration,
    next_call_id: AtomicU64,
}

impl PluginFunctionInvoker {
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

    fn alloc_call_id(&self, session_id: &str, plugin_id: &str, function_name: &str) -> String {
        let seq = self.next_call_id.fetch_add(1, Ordering::Relaxed);
        format!("{session_id}/{plugin_id}/{function_name}/{seq}")
    }

    pub async fn execute(
        &self,
        function: &RegisteredFunction,
        params: serde_json::Value,
        session_id: Option<&str>,
    ) -> Result<serde_json::Value, AppError> {
        let session_id = session_id.ok_or_else(|| {
            AppError::Plugin(tr(
                "plugin.functionSession",
                &[("function", &function.function), ("point", &function.point)],
            ))
        })?;
        let plugin_manager = self
            .plugin_manager
            .upgrade()
            .ok_or_else(|| AppError::Plugin(tr("plugin.managerUnavailable", &[])))?;
        let dispatcher = self
            .dispatcher
            .lock()
            .upgrade()
            .ok_or_else(|| AppError::Plugin(tr("plugin.dispatcherUnavailable", &[])))?;

        let plugin_info = plugin_manager
            .get_plugin(&function.plugin_id)
            .ok_or_else(|| {
                AppError::Plugin(tr("plugin.notLoaded", &[("id", &function.plugin_id)]))
            })?;
        if plugin_info.source != function.source
            || canonicalize_or_keep(&plugin_info.plugin_root)
                != canonicalize_or_keep(&function.plugin_root)
        {
            return Err(AppError::Plugin(tr(
                "plugin.functionSourceChanged",
                &[
                    ("id", &function.plugin_id),
                    ("expected", &function.plugin_root.display().to_string()),
                    ("actual", &plugin_info.plugin_root.display().to_string()),
                ],
            )));
        }

        let handle = plugin_manager
            .start_session_vm(session_id, &function.plugin_id)
            .await?;
        // Reload drains other sessions' VMs rather than interrupting them. A new
        // host-function call must not run old code under the replacement's route.
        if !plugin_manager.is_instance_current(&handle.instance_id) {
            return Err(AppError::Plugin(tr(
                "plugin.functionInstanceChanged",
                &[("id", &function.plugin_id)],
            )));
        }

        let call_id = self.alloc_call_id(session_id, &function.plugin_id, &function.function);
        let rx = dispatcher.register_command_waiter(&call_id);
        let _waiter = CommandWaiterGuard {
            dispatcher: &dispatcher,
            call_id: &call_id,
        };
        dispatcher.deliver_event(
            &handle.instance_id,
            EventEnvelope {
                event_type: wire::vm::WIRE_COMMAND_INVOKE.into(),
                data: serde_json::json!({
                    "kind": "function",
                    "callId": call_id,
                    "functionName": function.function,
                    "point": function.point,
                    "params": params,
                }),
                context: serde_json::json!({
                    "sessionId": session_id,
                    "pluginId": function.plugin_id,
                    "functionName": function.function,
                    "point": function.point,
                }),
            },
        )?;

        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(message))) => Err(AppError::QuickJS(message)),
            Ok(Err(_closed)) => Err(AppError::Plugin(tr(
                "plugin.functionClosed",
                &[("name", &function.function)],
            ))),
            Err(_) => Err(AppError::Plugin(tr(
                "plugin.functionTimeout",
                &[("name", &function.function)],
            ))),
        }
    }
}

fn canonicalize_or_keep(path: &Path) -> PathBuf {
    crate::infra::platform::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
