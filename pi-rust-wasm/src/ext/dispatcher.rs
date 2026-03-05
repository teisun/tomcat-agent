//! # 宿主 API 统一分发器 (HostApiDispatcher)
//!
//! 单入口多路复用：根据 HostRequest 的 module/method 路由到对应 Processor。
//! 与 Architecture 03-host-api-layer 3.3 一致；005/006/004/003 未就绪时以占位返回。

use crate::core::{LlmProvider, PrimitiveExecutor, ToolRegistry};
use crate::infra::error::AppError;
use crate::infra::EventBus;
use std::sync::Arc;

use super::host_binding::{HostRequest, HostResponse};

/// 宿主 API 分发器：无状态、Send + Sync，支持多 Agent 并发。
/// 各 Processor 以 Option 注入，未注入时返回明确错误。
pub struct HostApiDispatcher {
    #[allow(dead_code)] // 008 事件 API 实现时使用
    event_bus: Arc<dyn EventBus>,
    primitive: Option<Arc<dyn PrimitiveExecutor>>,
    tools: Option<Arc<dyn ToolRegistry>>,
    llm: Option<Arc<dyn LlmProvider>>,
}

impl HostApiDispatcher {
    /// 构造分发器；EventBus 必选，其余可选（005/006/004 合并后注入）。
    pub fn new(event_bus: Arc<dyn EventBus>) -> Self {
        Self {
            event_bus,
            primitive: None,
            tools: None,
            llm: None,
        }
    }

    /// 注入 4 原语执行器（005 就绪后调用）。
    pub fn with_primitive(mut self, p: Arc<dyn PrimitiveExecutor>) -> Self {
        self.primitive = Some(p);
        self
    }

    /// 注入工具注册中心（006 就绪后调用）。
    pub fn with_tools(mut self, t: Arc<dyn ToolRegistry>) -> Self {
        self.tools = Some(t);
        self
    }

    /// 注入 LLM Provider（004 就绪后调用）。
    pub fn with_llm(mut self, l: Arc<dyn LlmProvider>) -> Self {
        self.llm = Some(l);
        self
    }

    /// 同步分发入口：解析 request，按 module/method 路由，返回 HostResponse。
    /// 耗时操作（LLM 等）在 008 异步扩展中可改为异步等待，此处先返回“未实现”或占位。
    pub fn dispatch(
        &self,
        _instance_id: &str,
        request: HostRequest,
    ) -> Result<HostResponse, AppError> {
        let module = request.module.as_str();
        let method = request.method.as_str();
        let params = &request.params;

        match (module, method) {
            ("log" | "agent", "log") | ("agent", "info") | ("agent", "warn") | ("agent", "error") | ("agent", "debug") => {
                self.do_log(method, params)
            }
            ("fs" | "primitive", "readFile") => self.do_read_file(params),
            ("fs" | "primitive", "writeFile") => self.do_write_file(params),
            ("fs" | "primitive", "editFile") => self.do_edit_file(params),
            ("fs" | "primitive", "executeBash") => self.do_execute_bash(params),
            ("llm", "createChatCompletion") => self.do_chat(params),
            ("llm", "createChatCompletionStream") => self.do_chat_stream(params),
            ("tools", "registerTool") => self.do_register_tool(params),
            ("tools", "unregisterTool") => self.do_unregister_tool(params),
            ("tools", "getToolList") => self.do_list_tools(params),
            ("tools", "callTool") => self.do_call_tool(params),
            ("events", "on") | ("events", "once") | ("events", "off") | ("events", "emit") => {
                self.do_events(method, params)
            }
            _ => Ok(HostResponse::err(format!(
                "unknown API: {}.{}",
                module, method
            ))),
        }
    }

    fn do_log(&self, _method: &str, params: &serde_json::Value) -> Result<HostResponse, AppError> {
        let msg = params.get("message").and_then(|v| v.as_str()).unwrap_or("");
        tracing::info!("[plugin log] {}", msg);
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    fn do_read_file(&self, params: &serde_json::Value) -> Result<HostResponse, AppError> {
        let _path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let _plugin_id = params.get("pluginId").and_then(|v| v.as_str()).unwrap_or("");
        match &self.primitive {
            None => Ok(HostResponse::err("PrimitiveExecutor not configured (005)")),
            Some(_) => {
                // 异步原语在此处需 block_on 或由上层异步调用；008 异步扩展中再接入
                Ok(HostResponse::err(
                    "readFile: async hostcall not wired yet (use 008 async)",
                ))
            }
        }
    }

    fn do_write_file(&self, _params: &serde_json::Value) -> Result<HostResponse, AppError> {
        match &self.primitive {
            None => Ok(HostResponse::err("PrimitiveExecutor not configured (005)")),
            Some(_) => Ok(HostResponse::err(
                "writeFile: async hostcall not wired yet (008 async)",
            )),
        }
    }

    fn do_edit_file(&self, _params: &serde_json::Value) -> Result<HostResponse, AppError> {
        match &self.primitive {
            None => Ok(HostResponse::err("PrimitiveExecutor not configured (005)")),
            Some(_) => Ok(HostResponse::err(
                "editFile: async hostcall not wired yet (008 async)",
            )),
        }
    }

    fn do_execute_bash(&self, _params: &serde_json::Value) -> Result<HostResponse, AppError> {
        match &self.primitive {
            None => Ok(HostResponse::err("PrimitiveExecutor not configured (005)")),
            Some(_) => Ok(HostResponse::err(
                "executeBash: async hostcall not wired yet (008 async)",
            )),
        }
    }

    fn do_chat(&self, _params: &serde_json::Value) -> Result<HostResponse, AppError> {
        match &self.llm {
            None => Ok(HostResponse::err("LlmProvider not configured (004)")),
            Some(_) => Ok(HostResponse::err(
                "createChatCompletion: async hostcall not wired yet (008 async)",
            )),
        }
    }

    fn do_chat_stream(&self, _params: &serde_json::Value) -> Result<HostResponse, AppError> {
        match &self.llm {
            None => Ok(HostResponse::err("LlmProvider not configured (004)")),
            Some(_) => Ok(HostResponse::err(
                "createChatCompletionStream: async hostcall not wired yet (008 async)",
            )),
        }
    }

    fn do_register_tool(&self, _params: &serde_json::Value) -> Result<HostResponse, AppError> {
        match &self.tools {
            None => Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(_) => Ok(HostResponse::err(
                "registerTool: async hostcall not wired yet (008 async)",
            )),
        }
    }

    fn do_unregister_tool(&self, _params: &serde_json::Value) -> Result<HostResponse, AppError> {
        match &self.tools {
            None => Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(_) => Ok(HostResponse::err(
                "unregisterTool: async hostcall not wired yet (008 async)",
            )),
        }
    }

    fn do_list_tools(&self, _params: &serde_json::Value) -> Result<HostResponse, AppError> {
        match &self.tools {
            None => Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(_) => Ok(HostResponse::err(
                "getToolList: async hostcall not wired yet (008 async)",
            )),
        }
    }

    fn do_call_tool(&self, _params: &serde_json::Value) -> Result<HostResponse, AppError> {
        match &self.tools {
            None => Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(_) => Ok(HostResponse::err(
                "callTool: async hostcall not wired yet (008 async)",
            )),
        }
    }

    fn do_events(&self, method: &str, params: &serde_json::Value) -> Result<HostResponse, AppError> {
        let _ = (method, params);
        // EventBus 已有；008 可在此注册 on/once/off/emit 的 JSON 参数解析与调用
        Ok(HostResponse::err(
            "events.on/once/off/emit: not implemented yet (008)",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::DefaultEventBus;

    #[test]
    fn dispatch_unknown_api_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "unknown".to_string(),
            method: "foo".to_string(),
            params: serde_json::json!({}),
            call_id: None,
        };
        let res = d.dispatch("inst-1", req).unwrap();
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("unknown API"));
    }

    #[test]
    fn dispatch_log_succeeds() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "agent".to_string(),
            method: "log".to_string(),
            params: serde_json::json!({ "message": "hello" }),
            call_id: None,
        };
        let res = d.dispatch("inst-1", req).unwrap();
        assert!(res.ok);
    }

    #[test]
    fn dispatch_read_file_without_primitive_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "fs".to_string(),
            method: "readFile".to_string(),
            params: serde_json::json!({ "path": "/tmp/x", "pluginId": "p1" }),
            call_id: None,
        };
        let res = d.dispatch("inst-1", req).unwrap();
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("005"));
    }
}
