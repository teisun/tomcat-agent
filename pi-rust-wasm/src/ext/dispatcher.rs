//! # 宿主 API 统一分发器 (HostApiDispatcher)
//!
//! 单入口多路复用：根据 HostRequest 的 module/method 路由到对应 Processor。
//! 与 Architecture 宿主API层（host-api-layer）3.3 一致；支持 4 原语、LLM、工具、事件、会话 API。

use crate::core::{
    ChatMessage, ChatRequest, EditOperation, LlmProvider, PrimitiveExecutor, SessionManager,
    StreamEvent, Tool, ToolRegistry,
};
use crate::infra::error::AppError;
use crate::infra::event_bus::{EventBus, EventContext, EventListenerId};
use crate::infra::{AuditRecorder, HostcallAuditEntry};
use futures_util::StreamExt;
use std::sync::Arc;

use super::host_binding::{HostRequest, HostResponse};

/// 宿主 API 分发器：无状态、Send + Sync，支持多 Agent 并发。
/// 各 Processor 以 Option 注入，未注入时返回明确错误。
pub struct HostApiDispatcher {
    event_bus: Arc<dyn EventBus>,
    primitive: Option<Arc<dyn PrimitiveExecutor>>,
    tools: Option<Arc<dyn ToolRegistry>>,
    llm: Option<Arc<dyn LlmProvider>>,
    session: Option<Arc<SessionManager>>,
    audit: Option<Arc<dyn AuditRecorder>>,
}

impl HostApiDispatcher {
    /// 构造分发器；EventBus 必选，其余可选。
    pub fn new(event_bus: Arc<dyn EventBus>) -> Self {
        Self {
            event_bus,
            primitive: None,
            tools: None,
            llm: None,
            session: None,
            audit: None,
        }
    }

    /// 注入 4 原语执行器。
    pub fn with_primitive(mut self, p: Arc<dyn PrimitiveExecutor>) -> Self {
        self.primitive = Some(p);
        self
    }

    /// 注入工具注册中心。
    pub fn with_tools(mut self, t: Arc<dyn ToolRegistry>) -> Self {
        self.tools = Some(t);
        self
    }

    /// 注入 LLM Provider。
    pub fn with_llm(mut self, l: Arc<dyn LlmProvider>) -> Self {
        self.llm = Some(l);
        self
    }

    /// 注入 SessionManager（会话 API）。
    pub fn with_session(mut self, s: Arc<SessionManager>) -> Self {
        self.session = Some(s);
        self
    }

    /// 注入审计记录器（每笔 Hostcall 记录）。
    pub fn with_audit(mut self, a: Arc<dyn AuditRecorder>) -> Self {
        self.audit = Some(a);
        self
    }

    /// 同步分发入口：在独立 runtime 上 block_on(dispatch_async)，兼容同步调用方（如 host_binding）。
    ///
    /// # Errors
    /// * 与 [`dispatch_async`] 相同；此外若无法创建 tokio Runtime 会 panic（进程启动期单次调用，通常不会失败）。
    pub fn dispatch(
        &self,
        instance_id: &str,
        request: HostRequest,
    ) -> Result<HostResponse, AppError> {
        // 同步入口仅在 hostcall 时调用，进程内创建 Runtime 在常规环境下不会失败。
        let rt = tokio::runtime::Runtime::new().expect("create runtime for sync dispatch");
        rt.block_on(self.dispatch_async(instance_id, request))
    }

    /// 异步分发入口：按 module/method 路由，每笔 Hostcall 可选记录审计。
    ///
    /// # Errors
    /// * 返回的 `HostResponse` 中 `ok == false` 表示业务错误；未注入对应 Processor 时返回明确错误信息（如 "005"）。
    pub async fn dispatch_async(
        &self,
        instance_id: &str,
        request: HostRequest,
    ) -> Result<HostResponse, AppError> {
        let module = request.module.clone();
        let method = request.method.clone();
        let params = request.params.clone();

        let result = match (request.module.as_str(), request.method.as_str()) {
            ("log" | "agent", "log")
            | ("agent", "info")
            | ("agent", "warn")
            | ("agent", "error")
            | ("agent", "debug") => self.do_log(&method, &params),
            ("fs" | "primitive", "readFile") => self.do_read_file(instance_id, &params).await,
            ("fs" | "primitive", "writeFile") => self.do_write_file(instance_id, &params).await,
            ("fs" | "primitive", "editFile") => self.do_edit_file(instance_id, &params).await,
            ("fs" | "primitive", "executeBash") => self.do_execute_bash(instance_id, &params).await,
            ("llm", "createChatCompletion") => self.do_chat(instance_id, &params).await,
            ("llm", "createChatCompletionStream") => {
                self.do_chat_stream(instance_id, &params).await
            }
            ("tools", "registerTool") => self.do_register_tool(instance_id, &params).await,
            ("tools", "unregisterTool") => self.do_unregister_tool(instance_id, &params).await,
            ("tools", "getToolList") => self.do_list_tools(instance_id, &params).await,
            ("tools", "callTool") => self.do_call_tool(instance_id, &params).await,
            ("tools", "getActiveTools") => self.do_get_active_tools(instance_id, &params).await,
            ("tools", "setActiveTools") => self.do_set_active_tools(instance_id, &params).await,
            ("tools", "registerCommand") => self.do_register_command(instance_id, &params).await,
            ("events", "on")
            | ("events", "subscribe")
            | ("events", "once")
            | ("events", "off")
            | ("events", "emit") => {
                let effective_method = if method == "subscribe" { "on" } else { &method };
                self.do_events(instance_id, effective_method, &params).await
            }
            ("session" | "agent", "getCurrentSession") => {
                self.do_get_current_session(&params).await
            }
            ("session", "getMessages") => self.do_get_messages(&params).await,
            ("session", "sendMessage") => self.do_send_message(&params).await,
            ("agent", "sendMessage") => self.do_agent_send_message(&params),
            ("agent", "sendUserMessage") => self.do_agent_send_user_message(&params),
            ("context", "isIdle") => Ok(Self::do_context_is_idle()),
            ("context", "abort") => Ok(Self::do_context_abort()),
            ("context", "getCwd") => Ok(Self::do_context_get_cwd()),
            ("context", "getModel") => Ok(Self::do_context_get_model()),
            ("context", "uiNotify") => Ok(self.do_context_ui_notify(&params)),
            ("context", "uiSelect") | ("context", "uiConfirm") | ("context", "uiInput") => {
                Ok(Self::do_context_ui_stub(&method))
            }
            ("context", "getSystemPrompt") => Ok(Self::do_context_get_system_prompt()),
            ("context", "hasPendingMessages") => Ok(Self::do_context_has_pending()),
            ("context", "shutdown") => Ok(Self::do_context_shutdown()),
            ("context", "getContextUsage") => Ok(Self::do_context_usage()),
            ("context", "compact") => Ok(Self::do_context_compact()),
            _ => Ok(HostResponse::err(format!(
                "unknown API: {}.{}",
                module, method
            ))),
        };

        let success = result.is_ok();
        let detail = result.as_ref().err().map(|e| e.to_string());
        let response = match &result {
            Ok(r) => r.clone(),
            Err(e) => HostResponse::err(e.to_string()),
        };

        if let Some(audit) = &self.audit {
            audit.record_hostcall(HostcallAuditEntry {
                plugin_id: instance_id.to_string(),
                module,
                method,
                success,
                detail,
            });
        }

        Ok(response)
    }

    fn do_log(&self, _method: &str, params: &serde_json::Value) -> Result<HostResponse, AppError> {
        let msg = params.get("message").and_then(|v| v.as_str()).unwrap_or("");
        tracing::info!("[plugin log] {}", msg);
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    async fn do_read_file(
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

    async fn do_write_file(
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

    async fn do_edit_file(
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

    async fn do_execute_bash(
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
        let result = p.execute_bash(command, cwd.as_deref(), plugin_id).await?;
        Ok(HostResponse::ok(
            serde_json::to_value(result).map_err(AppError::Serialize)?,
        ))
    }

    async fn do_chat(
        &self,
        _plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let llm = match &self.llm {
            None => return Ok(HostResponse::err("LlmProvider not configured (004)")),
            Some(l) => l,
        };
        let req = parse_chat_request(params)?;
        let resp = llm.chat(req).await?;
        Ok(HostResponse::ok(
            serde_json::to_value(resp).map_err(AppError::Serialize)?,
        ))
    }

    async fn do_chat_stream(
        &self,
        _plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let llm = match &self.llm {
            None => return Ok(HostResponse::err("LlmProvider not configured (004)")),
            Some(l) => l,
        };
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

    async fn do_register_tool(
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

    async fn do_unregister_tool(
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

    async fn do_list_tools(
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

    async fn do_call_tool(
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

    /// 返回当前已启用的工具名列表（与 pi-mono getActiveTools 对齐）。
    async fn do_get_active_tools(
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

    /// 设置活跃工具集（按名称过滤启用/禁用）。MVP 阶段仅返回确认，不实际变更状态。
    async fn do_set_active_tools(
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
        // MVP: 接受请求但不实际变更工具启用状态，后续迭代实现完整的 active/inactive 切换。
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    /// 注册命令（与 pi-mono ExtensionAPI.registerCommand 对齐）。MVP 阶段仅记录，不执行。
    async fn do_register_command(
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
        tracing::info!(
            "[registerCommand] plugin={} cmd={} desc={}",
            plugin_id,
            name,
            description
        );
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    async fn do_events(
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
                // 宿主侧注册占位回调；实际 JS 回调由 __pi_dispatch_event 触发 pi_bridge.js 中的 __pi_hooks。
                // TODO: 长生命周期 VM 就绪后，此处应注入真实回调（通过 WasmInstance 回调到插件 JS）。
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

    async fn do_get_current_session(
        &self,
        _params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let session = match &self.session {
            None => return Ok(HostResponse::err("SessionManager not configured")),
            Some(s) => s,
        };
        let key = session.current_session_key();
        let entry = session.get_session(key)?;
        let data = match entry {
            Some(e) => serde_json::to_value(e).map_err(AppError::Serialize)?,
            None => serde_json::Value::Null,
        };
        Ok(HostResponse::ok(data))
    }

    async fn do_get_messages(&self, params: &serde_json::Value) -> Result<HostResponse, AppError> {
        let session = match &self.session {
            None => return Ok(HostResponse::err("SessionManager not configured")),
            Some(s) => s,
        };
        let cap = params.get("cap").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let entries = session.get_entries(cap)?;
        let list: Vec<serde_json::Value> = entries
            .into_iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect();
        Ok(HostResponse::ok(serde_json::json!(list)))
    }

    // -- agent module: sendMessage / sendUserMessage -----------------------
    fn do_agent_send_message(&self, params: &serde_json::Value) -> Result<HostResponse, AppError> {
        let message = params
            .get("message")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        tracing::info!("[plugin sendMessage] {:?}", message);
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    fn do_agent_send_user_message(
        &self,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let content = params
            .get("content")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        tracing::info!("[plugin sendUserMessage] {:?}", content);
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    // -- context module (for pi_bridge.js ctx proxy) ----------------------
    fn do_context_is_idle() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "idle": true }))
    }

    fn do_context_abort() -> HostResponse {
        tracing::info!("[context] abort requested by plugin");
        HostResponse::ok(serde_json::Value::Null)
    }

    fn do_context_get_cwd() -> HostResponse {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        HostResponse::ok(serde_json::json!({ "cwd": cwd }))
    }

    fn do_context_get_model() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "model": serde_json::Value::Null }))
    }

    fn do_context_ui_notify(&self, params: &serde_json::Value) -> HostResponse {
        let msg = params.get("message").and_then(|v| v.as_str()).unwrap_or("");
        let kind = params
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("info");
        tracing::info!("[context.ui.notify] [{}] {}", kind, msg);
        HostResponse::ok(serde_json::Value::Null)
    }

    fn do_context_ui_stub(method: &str) -> HostResponse {
        HostResponse::ok(serde_json::json!({ "stub": true, "method": method }))
    }

    fn do_context_get_system_prompt() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "prompt": "" }))
    }

    fn do_context_has_pending() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "pending": false }))
    }

    fn do_context_shutdown() -> HostResponse {
        tracing::warn!("[context] shutdown requested by plugin");
        HostResponse::ok(serde_json::Value::Null)
    }

    fn do_context_usage() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "tokens": null, "contextWindow": 0, "percent": null }))
    }

    fn do_context_compact() -> HostResponse {
        tracing::info!("[context] compact requested by plugin");
        HostResponse::ok(serde_json::Value::Null)
    }

    async fn do_send_message(&self, params: &serde_json::Value) -> Result<HostResponse, AppError> {
        let session = match &self.session {
            None => return Ok(HostResponse::err("SessionManager not configured")),
            Some(s) => s,
        };
        let message = params
            .get("message")
            .cloned()
            .ok_or_else(|| AppError::Plugin("sendMessage: missing message".to_string()))?;
        session.append_message(message)?;
        Ok(HostResponse::ok(serde_json::Value::Null))
    }
}

fn parse_chat_request(params: &serde_json::Value) -> Result<ChatRequest, AppError> {
    let messages: Vec<ChatMessage> = params
        .get("messages")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let model = params
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();
    Ok(ChatRequest {
        messages,
        model,
        temperature: params
            .get("temperature")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32),
        max_tokens: params
            .get("maxTokens")
            .or_else(|| params.get("max_tokens"))
            .and_then(|v| v.as_u64())
            .map(|u| u as u32),
        stream: params.get("stream").and_then(|v| v.as_bool()),
        model_override: None,
        tools: None,
    })
}

fn parse_tool(params: &serde_json::Value, plugin_id: &str) -> Result<Tool, AppError> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Plugin("registerTool: missing name".to_string()))?
        .to_string();
    let label = params
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or(&name)
        .to_string();
    let description = params
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let parameters = params
        .get("parameters")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(Tool {
        name,
        label,
        description,
        parameters,
        plugin_id: plugin_id.to_string(),
        is_enabled: true,
        created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        BashResult, ChatResponse, ChatResponseChoice, DirEntry, EditFileResult, PrimitiveOperation,
        WriteFileResult,
    };
    use crate::infra::DefaultEventBus;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[tokio::test]
    async fn dispatch_unknown_api_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "unknown".to_string(),
            method: "foo".to_string(),
            params: serde_json::json!({}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("unknown API"));
    }

    #[tokio::test]
    async fn dispatch_log_succeeds() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "agent".to_string(),
            method: "log".to_string(),
            params: serde_json::json!({ "message": "hello" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_read_file_without_primitive_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "fs".to_string(),
            method: "readFile".to_string(),
            params: serde_json::json!({ "path": "/tmp/x", "pluginId": "p1" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("005"));
    }

    #[tokio::test]
    async fn dispatch_session_get_current_without_session_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "session".to_string(),
            method: "getCurrentSession".to_string(),
            params: serde_json::json!({}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("SessionManager not configured"));
    }

    #[tokio::test]
    async fn dispatch_events_on_returns_listener_id() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "events".to_string(),
            method: "on".to_string(),
            params: serde_json::json!({ "eventName": "test_event" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
        let data = res.data.unwrap();
        assert!(data.get("listenerId").is_some());
    }

    #[tokio::test]
    async fn dispatch_events_emit_succeeds() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "events".to_string(),
            method: "emit".to_string(),
            params: serde_json::json!({ "eventName": "ev", "payload": {} }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_with_audit_records_hostcall() {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        struct CountAudit;
        impl AuditRecorder for CountAudit {
            fn record_primitive(&self, _: crate::infra::PrimitiveAuditEntry) {}
            fn record_tool_call(&self, _: crate::infra::ToolAuditEntry) {}
            fn record_hostcall(&self, _: crate::infra::HostcallAuditEntry) {
                COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }
        let bus = Arc::new(DefaultEventBus::new());
        let audit = Arc::new(CountAudit);
        let d = HostApiDispatcher::new(bus).with_audit(audit);
        let req = HostRequest {
            module: "agent".to_string(),
            method: "log".to_string(),
            params: serde_json::json!({ "message": "audit test" }),
            call_id: None,
        };
        let _ = d.dispatch_async("inst-1", req).await.unwrap();
        assert_eq!(COUNT.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dispatch_tools_without_registry_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "tools".to_string(),
            method: "getToolList".to_string(),
            params: serde_json::json!({}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("006"));
    }

    #[tokio::test]
    async fn dispatch_llm_without_provider_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "llm".to_string(),
            method: "createChatCompletion".to_string(),
            params: serde_json::json!({ "messages": [], "model": "gpt-4" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("004"));
    }

    struct MockPrimitive;
    #[async_trait::async_trait]
    impl PrimitiveExecutor for MockPrimitive {
        async fn read_file(&self, _path: &str, _plugin_id: &str) -> Result<String, AppError> {
            Ok("mock_content".to_string())
        }
        async fn list_dir(&self, _path: &str, _plugin_id: &str) -> Result<Vec<DirEntry>, AppError> {
            Ok(vec![])
        }
        async fn write_file(
            &self,
            path: &str,
            _content: &str,
            _overwrite: bool,
            _plugin_id: &str,
        ) -> Result<WriteFileResult, AppError> {
            Ok(WriteFileResult {
                path: path.to_string(),
                written: true,
            })
        }
        async fn edit_file(
            &self,
            path: &str,
            _edits: Vec<EditOperation>,
            _plugin_id: &str,
        ) -> Result<EditFileResult, AppError> {
            Ok(EditFileResult {
                path: path.to_string(),
                applied: true,
            })
        }
        async fn execute_bash(
            &self,
            _command: &str,
            _cwd: Option<&str>,
            _plugin_id: &str,
        ) -> Result<BashResult, AppError> {
            Ok(BashResult {
                stdout: "ok".to_string(),
                stderr: String::new(),
                exit_code: 0,
            })
        }
        async fn require_user_confirmation(
            &self,
            _op: PrimitiveOperation,
            _preview: &str,
            _plugin_id: &str,
        ) -> Result<bool, AppError> {
            Ok(true)
        }
    }

    struct MockLlm;
    #[async_trait::async_trait]
    impl LlmProvider for MockLlm {
        fn provider_name(&self) -> &str {
            "mock"
        }
        async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
            Ok(ChatResponse {
                id: Some("id".to_string()),
                choices: vec![ChatResponseChoice {
                    index: 0,
                    message: ChatMessage::assistant("hi"),
                    finish_reason: Some("stop".to_string()),
                }],
                usage: None,
            })
        }
        async fn chat_stream(
            &self,
            _req: ChatRequest,
        ) -> Result<
            Box<dyn futures_util::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
            AppError,
        > {
            use futures_util::stream;
            Ok(Box::new(stream::iter(vec![Ok(
                StreamEvent::ContentDelta {
                    delta: "hi".to_string(),
                },
            )])))
        }
        fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
            Ok(0)
        }
    }

    struct MockToolRegistry;
    #[async_trait::async_trait]
    impl ToolRegistry for MockToolRegistry {
        async fn register_tool(&self, _tool: Tool, _plugin_id: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn unregister_tool(&self, _name: &str, _plugin_id: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn get_tool(&self, _name: &str) -> Result<Tool, AppError> {
            Err(AppError::Tool("not found".to_string()))
        }
        async fn list_tools(&self, _plugin_id: Option<&str>) -> Result<Vec<Tool>, AppError> {
            Ok(vec![])
        }
        async fn call_tool(
            &self,
            _name: &str,
            _params: serde_json::Value,
            _plugin_id: &str,
        ) -> Result<serde_json::Value, AppError> {
            Ok(serde_json::json!({ "content": "ok", "details": null }))
        }
        fn unregister_plugin_tools(&self, _plugin_id: &str) {}
    }

    #[tokio::test]
    async fn dispatch_read_file_with_primitive_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_primitive(Arc::new(MockPrimitive));
        let req = HostRequest {
            module: "fs".to_string(),
            method: "readFile".to_string(),
            params: serde_json::json!({ "path": "/tmp/x", "pluginId": "p1" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
        assert_eq!(
            res.data
                .as_ref()
                .and_then(|d| d.get("content").and_then(|c| c.as_str())),
            Some("mock_content")
        );
    }

    #[tokio::test]
    async fn dispatch_write_file_with_primitive_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_primitive(Arc::new(MockPrimitive));
        let req = HostRequest {
            module: "fs".to_string(),
            method: "writeFile".to_string(),
            params: serde_json::json!({ "path": "/tmp/x", "content": "body", "pluginId": "p1" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_edit_file_with_primitive_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_primitive(Arc::new(MockPrimitive));
        let req = HostRequest {
            module: "fs".to_string(),
            method: "editFile".to_string(),
            params: serde_json::json!({ "path": "/tmp/x", "edits": [], "pluginId": "p1" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_execute_bash_with_primitive_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_primitive(Arc::new(MockPrimitive));
        let req = HostRequest {
            module: "fs".to_string(),
            method: "executeBash".to_string(),
            params: serde_json::json!({ "command": "echo x", "pluginId": "p1" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_chat_with_llm_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_llm(Arc::new(MockLlm));
        let req = HostRequest {
            module: "llm".to_string(),
            method: "createChatCompletion".to_string(),
            params: serde_json::json!({ "messages": [], "model": "gpt-4" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_chat_stream_with_llm_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_llm(Arc::new(MockLlm));
        let req = HostRequest {
            module: "llm".to_string(),
            method: "createChatCompletionStream".to_string(),
            params: serde_json::json!({ "messages": [], "model": "gpt-4" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
        assert!(res
            .data
            .as_ref()
            .and_then(|d| d.get("content").and_then(|c| c.as_str()))
            .is_some());
    }

    #[tokio::test]
    async fn dispatch_register_tool_with_registry_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tools(Arc::new(MockToolRegistry));
        let req = HostRequest {
            module: "tools".to_string(),
            method: "registerTool".to_string(),
            params: serde_json::json!({ "name": "t1", "label": "T1", "description": "d", "parameters": {} }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_list_tools_with_registry_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tools(Arc::new(MockToolRegistry));
        let req = HostRequest {
            module: "tools".to_string(),
            method: "getToolList".to_string(),
            params: serde_json::json!({}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
        assert!(res.data.as_ref().map(|d| d.is_array()).unwrap_or(false));
    }

    #[tokio::test]
    async fn dispatch_call_tool_with_registry_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tools(Arc::new(MockToolRegistry));
        let req = HostRequest {
            module: "tools".to_string(),
            method: "callTool".to_string(),
            params: serde_json::json!({ "toolName": "t1", "params": {} }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_session_get_current_with_session_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());
        let key = mgr.current_session_key();
        let _ = mgr.create_session(key, None).unwrap();
        let d = HostApiDispatcher::new(bus).with_session(Arc::new(mgr));
        let req = HostRequest {
            module: "session".to_string(),
            method: "getCurrentSession".to_string(),
            params: serde_json::json!({}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_get_messages_with_session_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());
        let key = mgr.current_session_key();
        let _ = mgr.create_session(key, None).unwrap();
        let d = HostApiDispatcher::new(bus).with_session(Arc::new(mgr));
        let req = HostRequest {
            module: "session".to_string(),
            method: "getMessages".to_string(),
            params: serde_json::json!({ "cap": 5 }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
        assert!(res.data.as_ref().map(|d| d.is_array()).unwrap_or(false));
    }

    #[tokio::test]
    async fn dispatch_send_message_with_session_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());
        let key = mgr.current_session_key();
        let _ = mgr.create_session(key, None).unwrap();
        let d = HostApiDispatcher::new(bus).with_session(Arc::new(mgr));
        let req = HostRequest {
            module: "session".to_string(),
            method: "sendMessage".to_string(),
            params: serde_json::json!({ "message": { "role": "user", "content": { "text": "hi" } } }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_unregister_tool_with_registry_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tools(Arc::new(MockToolRegistry));
        let req = HostRequest {
            module: "tools".to_string(),
            method: "unregisterTool".to_string(),
            params: serde_json::json!({ "toolName": "t1", "pluginId": "p1" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_events_once_returns_listener_id() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "events".to_string(),
            method: "once".to_string(),
            params: serde_json::json!({ "eventName": "test" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
        let id = res
            .data
            .as_ref()
            .and_then(|d| d.get("listenerId"))
            .and_then(|v| v.as_u64());
        assert!(id.is_some());
    }

    #[tokio::test]
    async fn dispatch_events_off_removes_listener() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let on_req = HostRequest {
            module: "events".to_string(),
            method: "on".to_string(),
            params: serde_json::json!({ "eventName": "e1" }),
            call_id: None,
        };
        let on_res = d.dispatch_async("inst-1", on_req).await.unwrap();
        assert!(on_res.ok);
        let listener_id = on_res
            .data
            .as_ref()
            .and_then(|d| d.get("listenerId"))
            .and_then(|v| v.as_u64())
            .expect("listenerId");
        let off_req = HostRequest {
            module: "events".to_string(),
            method: "off".to_string(),
            params: serde_json::json!({ "eventName": "e1", "listenerId": listener_id }),
            call_id: None,
        };
        let off_res = d.dispatch_async("inst-1", off_req).await.unwrap();
        assert!(off_res.ok);
    }

    #[tokio::test]
    async fn dispatch_chat_parses_max_tokens_and_temperature() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_llm(Arc::new(MockLlm));
        let req = HostRequest {
            module: "llm".to_string(),
            method: "createChatCompletion".to_string(),
            params: serde_json::json!({
                "messages": [],
                "model": "m",
                "maxTokens": 100,
                "temperature": 0.7
            }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_register_tool_missing_name_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tools(Arc::new(MockToolRegistry));
        let req = HostRequest {
            module: "tools".to_string(),
            method: "registerTool".to_string(),
            params: serde_json::json!({ "label": "L", "description": "d" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(!res.ok);
        assert!(res
            .error
            .as_ref()
            .map(|e| e.contains("name"))
            .unwrap_or(false));
    }

    #[tokio::test]
    async fn dispatch_get_active_tools_with_registry_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tools(Arc::new(MockToolRegistry));
        let req = HostRequest {
            module: "tools".to_string(),
            method: "getActiveTools".to_string(),
            params: serde_json::json!({}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_set_active_tools_with_registry_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tools(Arc::new(MockToolRegistry));
        let req = HostRequest {
            module: "tools".to_string(),
            method: "setActiveTools".to_string(),
            params: serde_json::json!({ "toolNames": ["tool_a", "tool_b"] }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_register_command_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "tools".to_string(),
            method: "registerCommand".to_string(),
            params: serde_json::json!({ "name": "myCmd", "description": "test command" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_register_command_missing_name_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "tools".to_string(),
            method: "registerCommand".to_string(),
            params: serde_json::json!({ "description": "no name" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(!res.ok);
    }
}
