use super::helpers::{parse_chat_request, parse_tool, plugin_id_from_instance};
use super::types::HostApiDispatcher;
use crate::core::tools::primitive::{BashTaskOutputChunk, BashTaskRegistry};
use crate::core::{ChatRequest, EditOperation, LlmProvider, LlmScene, StreamEvent};
use crate::ext::host_binding::HostResponse;
use crate::infra::error::AppError;
use crate::infra::event_bus::{EventListenerId, ScopedEventEmitter};
use dashmap::mapref::entry::Entry;
use futures_util::StreamExt;
use std::sync::Arc;

const HOST_TASK_OUTPUT_BLOCK_DEFAULT_WAIT_MS: u64 = 5_000;
const HOST_TASK_OUTPUT_BLOCK_MIN_WAIT_MS: u64 = 5_000;
const HOST_TASK_OUTPUT_BLOCK_MAX_WAIT_MS: u64 = 60_000;
const HOST_TASK_OUTPUT_WAIT_TAIL_MAX_BYTES: u64 = 4_096;

fn clamp_host_task_output_wait_ms(wait_ms_raw: Option<u64>) -> u64 {
    match wait_ms_raw {
        Some(0) => HOST_TASK_OUTPUT_BLOCK_MIN_WAIT_MS,
        Some(v) => v.clamp(
            HOST_TASK_OUTPUT_BLOCK_MIN_WAIT_MS,
            HOST_TASK_OUTPUT_BLOCK_MAX_WAIT_MS,
        ),
        None => HOST_TASK_OUTPUT_BLOCK_DEFAULT_WAIT_MS,
    }
}

async fn read_task_output_blocking(
    registry: &Arc<BashTaskRegistry>,
    task_id: &str,
    since: Option<u64>,
    wait_ms: u64,
) -> Result<serde_json::Value, AppError> {
    let since_value = since.unwrap_or(0);
    let wake_reason = match tokio::time::timeout(
        std::time::Duration::from_millis(wait_ms),
        registry.wait_for_finish(task_id),
    )
    .await
    {
        Ok(wait) => {
            wait?;
            return chunk_with_wake_reason(
                registry.read_output(task_id, Some(since_value)).await?,
                "finished",
            );
        }
        Err(_) => "wait_window_elapsed",
    };
    let snapshot = registry
        .read_output_tail(
            task_id,
            Some(since_value),
            HOST_TASK_OUTPUT_WAIT_TAIL_MAX_BYTES,
        )
        .await?;
    if snapshot.finished {
        chunk_with_wake_reason(
            registry.read_output(task_id, Some(since_value)).await?,
            "finished",
        )
    } else {
        chunk_with_wake_reason(snapshot, wake_reason)
    }
}

fn chunk_with_wake_reason(
    chunk: BashTaskOutputChunk,
    wake_reason: &str,
) -> Result<serde_json::Value, AppError> {
    let mut value = serde_json::to_value(chunk).map_err(AppError::Serialize)?;
    if let serde_json::Value::Object(ref mut map) = value {
        map.insert(
            "wakeReason".to_string(),
            serde_json::Value::String(wake_reason.to_string()),
        );
    }
    Ok(value)
}

impl HostApiDispatcher {
    fn llm_for_request(&self, req: &ChatRequest) -> Result<Arc<dyn LlmProvider>, AppError> {
        let model = req.model.trim();
        if !model.is_empty() && model != "default" {
            let resolver = self
                .llm_resolver
                .as_ref()
                .ok_or_else(|| AppError::Plugin("LlmResolver not configured (007)".into()))?;
            return Ok(resolver.resolve(LlmScene::Main, Some(model))?.provider_impl);
        }

        self.llm
            .clone()
            .ok_or_else(|| AppError::Plugin("LlmProvider not configured (004)".into()))
    }

    pub(super) async fn do_read_file(
        &self,
        plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let p = match &self.primitive {
            None => return Ok(HostResponse::err("PrimitiveExecutor not configured (005)")),
            Some(exec) => exec,
        };
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("readFile: missing path".to_string()))?;
        let content = p.read_file(path, plugin_id).await?;
        Ok(HostResponse::ok(serde_json::json!({ "content": content })))
    }

    pub(super) async fn do_write_file(
        &self,
        plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let p = match &self.primitive {
            None => return Ok(HostResponse::err("PrimitiveExecutor not configured (005)")),
            Some(exec) => exec,
        };
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("writeFile: missing path".to_string()))?;
        let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let overwrite = params
            .get("overwrite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let result = p.write_file(path, content, overwrite, plugin_id).await?;
        Ok(HostResponse::ok(
            serde_json::to_value(result).map_err(AppError::Serialize)?,
        ))
    }

    pub(super) async fn do_edit_file(
        &self,
        plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let p = match &self.primitive {
            None => return Ok(HostResponse::err("PrimitiveExecutor not configured (005)")),
            Some(exec) => exec,
        };
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("editFile: missing path".to_string()))?;
        let edits: Vec<EditOperation> = params
            .get("edits")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let result = p.edit_file(path, edits, plugin_id).await?;
        Ok(HostResponse::ok(
            serde_json::to_value(result).map_err(AppError::Serialize)?,
        ))
    }

    pub(super) async fn do_execute_bash(
        &self,
        plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let p = match &self.primitive {
            None => return Ok(HostResponse::err("PrimitiveExecutor not configured (005)")),
            Some(exec) => exec,
        };
        let command = params
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("executeBash: missing command".to_string()))?;
        let cwd = params.get("cwd").and_then(|v| v.as_str()).map(String::from);
        let argv_store: Option<Vec<String>> =
            params.get("args").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            });
        let argv_ref = argv_store.as_deref();
        // T2-P0-016 PR-E.2：扩展 `executeBash` HostCall 参数，可选 `foreground_wait_ms`；
        // 与 `tool_exec` 同口径在 trait 层接受 `Option<u64>`，未提供则用 config 默认。
        let foreground_wait_ms = params.get("foreground_wait_ms").and_then(|v| v.as_u64());
        let result = p
            .execute_bash(
                command,
                cwd.as_deref(),
                plugin_id,
                argv_ref,
                foreground_wait_ms,
            )
            .await?;
        Ok(HostResponse::ok(
            serde_json::to_value(result).map_err(AppError::Serialize)?,
        ))
    }

    pub(super) async fn do_task_output(
        &self,
        _plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let Some(registry) = self.bash_task_registry.as_ref() else {
            return Ok(HostResponse::err("BashTaskRegistry not configured (008)"));
        };
        let task_id = params
            .get("taskId")
            .or_else(|| params.get("task_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("taskOutput: missing taskId".to_string()))?;
        let since = params.get("since").and_then(|v| v.as_u64());
        let block = params
            .get("block")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let value = if block {
            let wait_ms = clamp_host_task_output_wait_ms(
                params
                    .get("waitMs")
                    .or_else(|| params.get("wait_ms"))
                    .and_then(|v| v.as_u64()),
            );
            read_task_output_blocking(registry, task_id, since, wait_ms).await?
        } else {
            serde_json::to_value(registry.read_output(task_id, since).await?)
                .map_err(AppError::Serialize)?
        };
        Ok(HostResponse::ok(value))
    }

    pub(super) async fn do_task_stop(
        &self,
        _plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let Some(registry) = self.bash_task_registry.as_ref() else {
            return Ok(HostResponse::err("BashTaskRegistry not configured (008)"));
        };
        let task_id = params
            .get("taskId")
            .or_else(|| params.get("task_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("taskStop: missing taskId".to_string()))?;
        registry.stop(task_id).await?;
        Ok(HostResponse::ok(
            serde_json::json!({ "taskId": task_id, "stopped": true }),
        ))
    }

    pub(super) async fn do_chat(
        &self,
        _plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let _permit = self
            .llm_semaphore
            .acquire()
            .await
            .map_err(|_| AppError::Plugin("LLM semaphore closed".into()))?;
        let req = parse_chat_request(params)?;
        let llm = self.llm_for_request(&req)?;
        let resp = llm.chat(req).await?;
        Ok(HostResponse::ok(
            serde_json::to_value(resp).map_err(AppError::Serialize)?,
        ))
    }

    pub(super) async fn do_chat_stream(
        &self,
        _plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let _permit = self
            .llm_semaphore
            .acquire()
            .await
            .map_err(|_| AppError::Plugin("LLM semaphore closed".into()))?;
        let req = parse_chat_request(params)?;
        let llm = self.llm_for_request(&req)?;
        let mut stream = llm.chat_stream(req).await?;
        let mut content = String::new();
        while let Some(ev) = stream.next().await {
            let ev = ev?;
            if let StreamEvent::ContentDelta { delta } = ev {
                content.push_str(&delta);
            }
        }
        Ok(HostResponse::ok(serde_json::json!({ "content": content })))
    }

    pub(super) fn do_llm_get_model(&self, instance_id: &str) -> HostResponse {
        self.do_context_get_model(instance_id)
    }

    pub(super) fn do_llm_set_model(params: &serde_json::Value) -> HostResponse {
        let model = params.get("model").and_then(|v| v.as_str()).unwrap_or("");
        tracing::debug!("[llm.setModel] plugin requested model={} (MVP stub)", model);
        HostResponse::ok(serde_json::json!({ "model": model }))
    }

    pub(super) async fn do_register_tool(
        &self,
        instance_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let tools = match &self.tools {
            None => return Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(t) => t,
        };
        let plugin_id = plugin_id_from_instance(instance_id);
        let tool = parse_tool(params, plugin_id)?;
        tools.register_tool(tool, plugin_id).await?;
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("registerTool: missing name".to_string()))?;
        match self.plugin_tools.entry(plugin_id.to_string()) {
            Entry::Occupied(mut ent) => {
                let tools = ent.get_mut();
                if !tools.iter().any(|existing| existing == name) {
                    tools.push(name.to_string());
                }
            }
            Entry::Vacant(ent) => {
                ent.insert(vec![name.to_string()]);
            }
        }
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    pub(super) async fn do_unregister_tool(
        &self,
        instance_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let tools = match &self.tools {
            None => return Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(t) => t,
        };
        let plugin_id = plugin_id_from_instance(instance_id);
        let name = params
            .get("toolName")
            .or_else(|| params.get("tool_name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("unregisterTool: missing toolName".to_string()))?;
        tools.unregister_tool(name, plugin_id).await?;
        if let Some(mut entry) = self.plugin_tools.get_mut(plugin_id) {
            entry.retain(|existing| existing != name);
        }
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    pub(super) async fn do_list_tools(
        &self,
        _plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let tools = match &self.tools {
            None => return Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(t) => t,
        };
        let filter_plugin = params.get("pluginId").and_then(|v| v.as_str());
        let list = tools.list_tools(filter_plugin).await?;
        Ok(HostResponse::ok(
            serde_json::to_value(list).map_err(AppError::Serialize)?,
        ))
    }

    pub(super) async fn do_call_tool(
        &self,
        instance_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let tools = match &self.tools {
            None => return Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(t) => t,
        };
        let caller_plugin_id = plugin_id_from_instance(instance_id);
        let session_id = self.session_id_for_instance(instance_id);
        let name = params
            .get("toolName")
            .or_else(|| params.get("tool_name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("callTool: missing toolName".to_string()))?;
        let tool_params = params
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let result = tools
            .call_tool(name, tool_params, caller_plugin_id, session_id.as_deref())
            .await?;
        Ok(HostResponse::ok(result))
    }

    pub(super) async fn do_get_active_tools(
        &self,
        _plugin_id: &str,
        _params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let tools = match &self.tools {
            None => return Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(t) => t,
        };
        let list = tools.list_tools(None).await?;
        let names: Vec<&str> = list.iter().map(|t| t.name.as_str()).collect();
        Ok(HostResponse::ok(
            serde_json::to_value(names).map_err(AppError::Serialize)?,
        ))
    }

    pub(super) async fn do_set_active_tools(
        &self,
        _plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let _tools = match &self.tools {
            None => return Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(t) => t,
        };
        let _tool_names = params
            .get("toolNames")
            .or_else(|| params.get("tool_names"))
            .and_then(|v| v.as_array());
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    pub(super) async fn do_register_command(
        &self,
        instance_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let plugin_id = plugin_id_from_instance(instance_id);
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("registerCommand: missing name".to_string()))?;
        let description = params
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        tracing::debug!(
            "[registerCommand] plugin={} cmd={} desc={}",
            plugin_id,
            name,
            description
        );
        match self.plugin_commands.entry(plugin_id.to_string()) {
            Entry::Occupied(mut ent) => {
                let v = ent.get_mut();
                if let Some(i) = v.iter().position(|(n, _)| n == name) {
                    v[i] = (name.to_string(), description.to_string());
                } else {
                    v.push((name.to_string(), description.to_string()));
                }
            }
            Entry::Vacant(ent) => {
                ent.insert(vec![(name.to_string(), description.to_string())]);
            }
        }
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    pub(super) async fn do_events(
        &self,
        instance_id: &str,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let plugin_id = plugin_id_from_instance(instance_id);
        let event_name = params
            .get("eventName")
            .or_else(|| params.get("event_name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("events: missing eventName".to_string()))?;
        match method {
            "on" => {
                let id = self
                    .event_bus
                    .on_plugin(event_name, plugin_id, Box::new(|_| Ok(())));
                match self.plugin_event_listeners.entry(plugin_id.to_string()) {
                    Entry::Occupied(mut ent) => ent.get_mut().push(id),
                    Entry::Vacant(ent) => {
                        ent.insert(vec![id]);
                    }
                }
                Ok(HostResponse::ok(serde_json::json!({ "listenerId": id.0 })))
            }
            "once" => {
                let id = self
                    .event_bus
                    .once_plugin(event_name, plugin_id, Box::new(|_| Ok(())));
                match self.plugin_event_listeners.entry(plugin_id.to_string()) {
                    Entry::Occupied(mut ent) => ent.get_mut().push(id),
                    Entry::Vacant(ent) => {
                        ent.insert(vec![id]);
                    }
                }
                Ok(HostResponse::ok(serde_json::json!({ "listenerId": id.0 })))
            }
            "off" => {
                let id = params
                    .get("listenerId")
                    .or_else(|| params.get("listener_id"))
                    .and_then(|v| v.as_u64())
                    .map(EventListenerId)
                    .ok_or_else(|| {
                        AppError::Plugin("events.off: missing listenerId".to_string())
                    })?;
                self.event_bus.off(id);
                if let Some(mut entry) = self.plugin_event_listeners.get_mut(plugin_id) {
                    entry.retain(|existing| *existing != id);
                }
                Ok(HostResponse::ok(serde_json::Value::Null))
            }
            "emit" => {
                let payload = params
                    .get("payload")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let emitter = ScopedEventEmitter::new_optional(
                    self.event_bus.clone(),
                    self.session_id_for_instance(instance_id),
                );
                emitter.emit_payload_with_plugin_id(event_name, payload, plugin_id)?;
                Ok(HostResponse::ok(serde_json::Value::Null))
            }
            _ => Ok(HostResponse::err(format!(
                "events: unknown method {}",
                method
            ))),
        }
    }

    pub(super) fn session_id_for_instance(&self, instance_id: &str) -> Option<String> {
        if let Some((session_id, _)) = instance_id.split_once('/') {
            if !session_id.is_empty() {
                return Some(session_id.to_string());
            }
        }
        self.session
            .as_ref()
            .and_then(|session| session.current_session_id().ok().flatten())
    }

    pub(super) fn session_for_id(
        &self,
        session_id: &str,
    ) -> Option<std::sync::Arc<crate::core::SessionManager>> {
        if let Some(entry) = self.session_registry.get(session_id) {
            if let Some(session) = entry.value().upgrade() {
                return Some(session);
            }
        }
        self.session.clone()
    }

    pub(super) fn session_for_instance(
        &self,
        instance_id: &str,
    ) -> Option<std::sync::Arc<crate::core::SessionManager>> {
        let session_id = self.session_id_for_instance(instance_id)?;
        self.session_for_id(&session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::DefaultEventBus;

    #[test]
    fn clamp_host_task_output_wait_ms_caps_at_sixty_seconds() {
        assert_eq!(clamp_host_task_output_wait_ms(None), 5_000);
        assert_eq!(clamp_host_task_output_wait_ms(Some(0)), 5_000);
        assert_eq!(clamp_host_task_output_wait_ms(Some(1_000)), 5_000);
        assert_eq!(clamp_host_task_output_wait_ms(Some(60_000)), 60_000);
        assert_eq!(clamp_host_task_output_wait_ms(Some(600_000)), 60_000);
    }

    #[tokio::test]
    async fn do_task_output_block_false_returns_current_chunk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
        let ticket = registry
            .spawn(
                "printf ext-host-tail".to_string(),
                None,
                Some(dir.path().to_path_buf()),
            )
            .await
            .expect("spawn");
        registry
            .wait_for_finish(&ticket.task_id)
            .await
            .expect("finish");
        let dispatcher = HostApiDispatcher::new(Arc::new(DefaultEventBus::new()))
            .with_bash_task_registry(registry);

        let response = dispatcher
            .do_task_output("plugin", &serde_json::json!({ "taskId": ticket.task_id }))
            .await
            .expect("host response");
        assert!(response.ok);
        let data = response.data.expect("chunk data");
        assert_eq!(data["finished"], serde_json::json!(true));
        assert!(data["content"]
            .as_str()
            .unwrap_or_default()
            .contains("ext-host-tail"));
    }

    #[tokio::test]
    async fn do_task_output_unknown_task_id_returns_err() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dispatcher = HostApiDispatcher::new(Arc::new(DefaultEventBus::new()))
            .with_bash_task_registry(Arc::new(BashTaskRegistry::new(
                dir.path().join("tool-results"),
            )));

        let err = dispatcher
            .do_task_output("plugin", &serde_json::json!({ "taskId": "missing-task" }))
            .await
            .expect_err("unknown task id should error");
        assert!(err.to_string().contains("bash task not found"));
    }
}
