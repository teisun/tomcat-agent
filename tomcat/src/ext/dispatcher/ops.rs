use super::helpers::{parse_chat_request, parse_tool};
use super::types::HostApiDispatcher;
use crate::core::{EditOperation, StreamEvent};
use crate::ext::host_binding::HostResponse;
use crate::infra::error::AppError;
use crate::infra::event_bus::{EventContext, EventListenerId};
use dashmap::mapref::entry::Entry;
use futures_util::StreamExt;

impl HostApiDispatcher {
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
        // T2-P0-016 PR-E.2：扩展 `executeBash` HostCall 参数，可选 `timeout_ms`；
        // 与 `tool_exec` 同口径在 trait 层接受 `Option<u64>`，未提供则用 config 默认。
        let timeout_ms = params.get("timeout_ms").and_then(|v| v.as_u64());
        let result = p
            .execute_bash(command, cwd.as_deref(), plugin_id, argv_ref, timeout_ms)
            .await?;
        Ok(HostResponse::ok(
            serde_json::to_value(result).map_err(AppError::Serialize)?,
        ))
    }

    pub(super) async fn do_chat(
        &self,
        _plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let llm = match &self.llm {
            None => return Ok(HostResponse::err("LlmProvider not configured (004)")),
            Some(l) => l,
        };
        let _permit = self
            .llm_semaphore
            .acquire()
            .await
            .map_err(|_| AppError::Plugin("LLM semaphore closed".into()))?;
        let req = parse_chat_request(params)?;
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
        let llm = match &self.llm {
            None => return Ok(HostResponse::err("LlmProvider not configured (004)")),
            Some(l) => l,
        };
        let _permit = self
            .llm_semaphore
            .acquire()
            .await
            .map_err(|_| AppError::Plugin("LLM semaphore closed".into()))?;
        let req = parse_chat_request(params)?;
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

    pub(super) fn do_llm_get_model() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "model": serde_json::Value::Null }))
    }

    pub(super) fn do_llm_set_model(params: &serde_json::Value) -> HostResponse {
        let model = params.get("model").and_then(|v| v.as_str()).unwrap_or("");
        tracing::debug!("[llm.setModel] plugin requested model={} (MVP stub)", model);
        HostResponse::ok(serde_json::json!({ "model": model }))
    }

    pub(super) async fn do_register_tool(
        &self,
        plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let tools = match &self.tools {
            None => return Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(t) => t,
        };
        let tool = parse_tool(params, plugin_id)?;
        tools.register_tool(tool, plugin_id).await?;
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    pub(super) async fn do_unregister_tool(
        &self,
        plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let tools = match &self.tools {
            None => return Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(t) => t,
        };
        let name = params
            .get("toolName")
            .or_else(|| params.get("tool_name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("unregisterTool: missing toolName".to_string()))?;
        tools.unregister_tool(name, plugin_id).await?;
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
        plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let tools = match &self.tools {
            None => return Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(t) => t,
        };
        let name = params
            .get("toolName")
            .or_else(|| params.get("tool_name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("callTool: missing toolName".to_string()))?;
        let tool_params = params
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let result = tools.call_tool(name, tool_params, plugin_id).await?;
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
        plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
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
        plugin_id: &str,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let event_name = params
            .get("eventName")
            .or_else(|| params.get("event_name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("events: missing eventName".to_string()))?;
        match method {
            "on" => {
                let id = self.event_bus.on(event_name, Box::new(|_| Ok(())));
                Ok(HostResponse::ok(serde_json::json!({ "listenerId": id.0 })))
            }
            "once" => {
                let id = self.event_bus.once(event_name, Box::new(|_| Ok(())));
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
                Ok(HostResponse::ok(serde_json::Value::Null))
            }
            "emit" => {
                let payload = params
                    .get("payload")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let ctx = EventContext::new(event_name, payload).with_plugin_id(plugin_id);
                self.event_bus.emit_sync(event_name, ctx)?;
                Ok(HostResponse::ok(serde_json::Value::Null))
            }
            _ => Ok(HostResponse::err(format!(
                "events: unknown method {}",
                method
            ))),
        }
    }
}
