use super::helpers::normalize_tool_parameters;
use super::{AsyncCallStatus, HostApiDispatcher};
use crate::core::{
    BashResult, ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice, DirEntry,
    EditFileResult, EditOperation, LlmProvider, PrimitiveExecutor, PrimitiveOperation,
    SessionManager, StreamEvent, Tool, ToolRegistry, WriteFileResult,
};
use crate::ext::host_binding::{HostRequest, HostResponse};
use crate::ext::vm_actor::EventEnvelope;
use crate::infra::error::AppError;
use crate::infra::wire;
use crate::infra::{AuditRecorder, DefaultEventBus};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::runtime::Handle;

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
        fn record_plugin_lifecycle(&self, _: crate::infra::PluginLifecycleAuditEntry) {}
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
        _argv: Option<&[String]>,
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
async fn dispatch_execute_bash_with_argv_calls_primitive() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let ran = Arc::new(AtomicBool::new(false));
    let ran2 = Arc::clone(&ran);
    #[derive(Clone)]
    struct ArgvPrimitive(Arc<AtomicBool>);
    #[async_trait::async_trait]
    impl PrimitiveExecutor for ArgvPrimitive {
        async fn read_file(&self, _p: &str, _id: &str) -> Result<String, AppError> {
            Ok(String::new())
        }
        async fn list_dir(&self, _p: &str, _id: &str) -> Result<Vec<DirEntry>, AppError> {
            Ok(vec![])
        }
        async fn write_file(
            &self,
            _p: &str,
            _c: &str,
            _o: bool,
            _id: &str,
        ) -> Result<WriteFileResult, AppError> {
            Ok(WriteFileResult {
                path: String::new(),
                written: false,
            })
        }
        async fn edit_file(
            &self,
            _p: &str,
            _e: Vec<EditOperation>,
            _id: &str,
        ) -> Result<EditFileResult, AppError> {
            Ok(EditFileResult {
                path: String::new(),
                applied: false,
            })
        }
        async fn execute_bash(
            &self,
            cmd: &str,
            _cwd: Option<&str>,
            _id: &str,
            argv: Option<&[String]>,
        ) -> Result<BashResult, AppError> {
            if cmd == "echo" {
                if let Some(a) = argv {
                    if a.len() == 2 && a[0] == "a" && a[1] == "b" {
                        self.0.store(true, Ordering::SeqCst);
                    }
                }
            }
            Ok(BashResult {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            })
        }
        async fn require_user_confirmation(
            &self,
            _op: PrimitiveOperation,
            _prev: &str,
            _id: &str,
        ) -> Result<bool, AppError> {
            Ok(true)
        }
    }
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus).with_primitive(Arc::new(ArgvPrimitive(ran2)));
    let req = HostRequest {
        module: "fs".to_string(),
        method: "executeBash".to_string(),
        params: serde_json::json!({
            "command": "echo",
            "args": ["a", "b"],
            "pluginId": "p1"
        }),
        call_id: None,
    };
    let res = d.dispatch_async("inst-argv", req).await.unwrap();
    assert!(res.ok);
    assert!(
        ran.load(Ordering::SeqCst),
        "execute_bash 应收到 argv 模式参数"
    );
}

#[tokio::test]
async fn dispatch_register_command_records_metadata() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    let req = HostRequest {
        module: "tools".to_string(),
        method: "registerCommand".to_string(),
        params: serde_json::json!({ "name": "my-cmd", "description": "desc" }),
        call_id: None,
    };
    let res = d.dispatch_async("inst-rc", req).await.unwrap();
    assert!(res.ok);
    let cmds = d.registered_plugin_commands("inst-rc");
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].0, "my-cmd");
    assert_eq!(cmds[0].1, "desc");
}

#[test]
fn normalize_tool_parameters_unwraps_schema() {
    let raw = serde_json::json!({
        "schema": {
            "type": "object",
            "properties": { "q": { "type": "string" } }
        }
    });
    let n = normalize_tool_parameters(&raw);
    assert_eq!(n.get("type").and_then(|v| v.as_str()), Some("object"));
    assert!(n.get("properties").is_some());
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

// ========== Async Hostcall Tests (8.4.8) ==========

fn make_dispatcher_with_primitive() -> HostApiDispatcher {
    let bus = Arc::new(DefaultEventBus::new());
    HostApiDispatcher::new(bus)
        .with_tokio_handle(Handle::current())
        .with_primitive(Arc::new(MockPrimitive))
}

#[tokio::test]
async fn async_submit_poll_full_roundtrip() {
    let d = make_dispatcher_with_primitive();
    let req = HostRequest {
        module: "fs".to_string(),
        method: "executeBash".to_string(),
        params: serde_json::json!({"command": "echo hi"}),
        call_id: Some("req-1".to_string()),
    };
    let submit = d.dispatch("inst-a", req).unwrap();
    assert!(submit.ok);
    assert_eq!(submit.call_id.as_deref(), Some("req-1"));
    assert!(submit
        .data
        .as_ref()
        .unwrap()
        .get("pending")
        .unwrap()
        .as_bool()
        .unwrap());

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let poll_req = HostRequest {
        module: "__async".to_string(),
        method: "poll".to_string(),
        params: serde_json::json!({"callId": "req-1"}),
        call_id: None,
    };
    let poll_res = d.dispatch_async("inst-a", poll_req).await.unwrap();
    assert!(poll_res.ok);
    let data = poll_res.data.unwrap();
    assert!(data.get("ready").unwrap().as_bool().unwrap());
    assert!(data.get("result").is_some());
}

#[tokio::test]
async fn sync_path_unchanged_without_call_id() {
    let d = make_dispatcher_with_primitive();
    let res = tokio::task::spawn_blocking(move || {
        let req = HostRequest {
            module: "fs".to_string(),
            method: "executeBash".to_string(),
            params: serde_json::json!({"command": "echo hi"}),
            call_id: None,
        };
        d.dispatch("inst-a", req)
    })
    .await
    .unwrap()
    .unwrap();
    assert!(res.ok);
    assert!(res.data.as_ref().unwrap().get("stdout").is_some());
}

#[tokio::test]
async fn async_poll_not_ready_immediately() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
    d.async_results
        .insert("pending-1".to_string(), AsyncCallStatus::Pending);
    let poll_req = HostRequest {
        module: "__async".to_string(),
        method: "poll".to_string(),
        params: serde_json::json!({"callId": "pending-1"}),
        call_id: None,
    };
    let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
    assert!(res.ok);
    assert!(!res.data.unwrap().get("ready").unwrap().as_bool().unwrap());
}

#[tokio::test]
async fn async_poll_ready_returns_result() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
    d.async_results.insert(
        "done-1".to_string(),
        AsyncCallStatus::Done(HostResponse::ok(serde_json::json!({"stdout": "hello"}))),
    );
    let poll_req = HostRequest {
        module: "__async".to_string(),
        method: "poll".to_string(),
        params: serde_json::json!({"callId": "done-1"}),
        call_id: None,
    };
    let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
    assert!(res.ok);
    let data = res.data.unwrap();
    assert!(data.get("ready").unwrap().as_bool().unwrap());
    let result = data.get("result").unwrap();
    assert_eq!(result.get("stdout").unwrap().as_str().unwrap(), "hello");
}

#[tokio::test]
async fn async_poll_error_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
    d.async_results.insert(
        "err-1".to_string(),
        AsyncCallStatus::Error("something broke".to_string()),
    );
    let poll_req = HostRequest {
        module: "__async".to_string(),
        method: "poll".to_string(),
        params: serde_json::json!({"callId": "err-1"}),
        call_id: None,
    };
    let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
    assert!(!res.ok);
    assert!(res.error.unwrap().contains("something broke"));
}

#[tokio::test]
async fn async_timeout_produces_error() {
    let bus = Arc::new(DefaultEventBus::new());
    // Slow mock: sleeps longer than timeout
    struct SlowPrimitive;
    #[async_trait::async_trait]
    impl PrimitiveExecutor for SlowPrimitive {
        async fn read_file(&self, _: &str, _: &str) -> Result<String, AppError> {
            Ok(String::new())
        }
        async fn list_dir(&self, _: &str, _: &str) -> Result<Vec<DirEntry>, AppError> {
            Ok(vec![])
        }
        async fn write_file(
            &self,
            _: &str,
            _: &str,
            _: bool,
            _: &str,
        ) -> Result<WriteFileResult, AppError> {
            Ok(WriteFileResult {
                path: String::new(),
                written: false,
            })
        }
        async fn edit_file(
            &self,
            _: &str,
            _: Vec<EditOperation>,
            _: &str,
        ) -> Result<EditFileResult, AppError> {
            Ok(EditFileResult {
                path: String::new(),
                applied: false,
            })
        }
        async fn execute_bash(
            &self,
            _: &str,
            _: Option<&str>,
            _: &str,
            _: Option<&[String]>,
        ) -> Result<BashResult, AppError> {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            Ok(BashResult {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            })
        }
        async fn require_user_confirmation(
            &self,
            _: PrimitiveOperation,
            _: &str,
            _: &str,
        ) -> Result<bool, AppError> {
            Ok(true)
        }
    }
    let d = HostApiDispatcher::new(bus)
        .with_tokio_handle(Handle::current())
        .with_primitive(Arc::new(SlowPrimitive))
        .with_async_timeout(std::time::Duration::from_millis(100));
    let req = HostRequest {
        module: "fs".to_string(),
        method: "executeBash".to_string(),
        params: serde_json::json!({"command": "slow"}),
        call_id: Some("timeout-1".to_string()),
    };
    d.dispatch("inst-a", req).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let poll_req = HostRequest {
        module: "__async".to_string(),
        method: "poll".to_string(),
        params: serde_json::json!({"callId": "timeout-1"}),
        call_id: None,
    };
    let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
    assert!(!res.ok);
    assert!(res.error.unwrap().contains("timeout"));
}

#[tokio::test]
async fn async_multiple_call_ids_concurrent() {
    let d = make_dispatcher_with_primitive();
    for i in 0..5 {
        let req = HostRequest {
            module: "fs".to_string(),
            method: "executeBash".to_string(),
            params: serde_json::json!({"command": format!("echo {i}")}),
            call_id: Some(format!("multi-{i}")),
        };
        let submit = d.dispatch("inst-a", req).unwrap();
        assert!(submit.ok);
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    for i in 0..5 {
        let poll_req = HostRequest {
            module: "__async".to_string(),
            method: "poll".to_string(),
            params: serde_json::json!({"callId": format!("multi-{i}")}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
        assert!(res.ok);
        assert!(res.data.unwrap().get("ready").unwrap().as_bool().unwrap());
    }
}

#[tokio::test]
async fn async_cleanup_instance_removes_pending() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
    d.async_results
        .insert("ci-1".to_string(), AsyncCallStatus::Pending);
    d.async_results
        .insert("ci-2".to_string(), AsyncCallStatus::Pending);
    d.instance_calls
        .entry("inst-x".to_string())
        .or_default()
        .extend(["ci-1".to_string(), "ci-2".to_string()]);
    // Also add one for a different instance to ensure it's not removed
    d.async_results
        .insert("other-1".to_string(), AsyncCallStatus::Pending);
    d.instance_calls
        .entry("inst-y".to_string())
        .or_default()
        .push("other-1".to_string());

    d.cleanup_instance("inst-x");

    assert!(d.async_results.get("ci-1").is_none());
    assert!(d.async_results.get("ci-2").is_none());
    assert!(d.async_results.get("other-1").is_some());
}

#[tokio::test]
async fn async_poll_cleans_up_after_ready() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
    d.async_results.insert(
        "once-1".to_string(),
        AsyncCallStatus::Done(HostResponse::ok(serde_json::json!({"v": 42}))),
    );
    let poll_req = || HostRequest {
        module: "__async".to_string(),
        method: "poll".to_string(),
        params: serde_json::json!({"callId": "once-1"}),
        call_id: None,
    };
    let res1 = d.dispatch_async("inst-a", poll_req()).await.unwrap();
    assert!(res1.ok);
    assert!(res1.data.unwrap().get("ready").unwrap().as_bool().unwrap());

    let res2 = d.dispatch_async("inst-a", poll_req()).await.unwrap();
    assert!(!res2.ok);
    assert!(res2.error.unwrap().contains("unknown callId"));
}

#[tokio::test]
async fn async_poll_missing_call_id_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
    let poll_req = HostRequest {
        module: "__async".to_string(),
        method: "poll".to_string(),
        params: serde_json::json!({}),
        call_id: None,
    };
    let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
    assert!(!res.ok);
    assert!(res.error.unwrap().contains("missing callId"));
}

#[test]
fn register_event_channel_and_deliver() {
    let bus = Arc::new(crate::infra::DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    d.register_event_channel("s1/p1", 4);

    let envelope = EventEnvelope {
        event_type: "test_event".into(),
        data: serde_json::json!({"key": "val"}),
        context: serde_json::json!({}),
    };
    d.deliver_event("s1/p1", envelope).unwrap();
}

#[test]
fn deliver_event_backpressure() {
    let bus = Arc::new(crate::infra::DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    d.register_event_channel("s1/p1", 2);

    for _ in 0..2 {
        d.deliver_event(
            "s1/p1",
            EventEnvelope {
                event_type: "x".into(),
                data: serde_json::json!(null),
                context: serde_json::json!(null),
            },
        )
        .unwrap();
    }

    let r = d.deliver_event(
        "s1/p1",
        EventEnvelope {
            event_type: "overflow".into(),
            data: serde_json::json!(null),
            context: serde_json::json!(null),
        },
    );
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("backpressure"));
}

#[test]
fn wait_for_event_receives_delivered_event() {
    let bus = Arc::new(crate::infra::DefaultEventBus::new());
    let d = Arc::new(HostApiDispatcher::new(bus));
    d.register_event_channel("s1/p1", 4);

    d.deliver_event(
        "s1/p1",
        EventEnvelope {
            event_type: wire::vm::WIRE_SESSION_START.into(),
            data: serde_json::json!({"sid": "s1"}),
            context: serde_json::json!({}),
        },
    )
    .unwrap();

    let resp = d
        .do_wait_for_event("s1/p1", &serde_json::json!({}))
        .unwrap();
    assert!(resp.ok);
    assert_eq!(
        resp.data.as_ref().unwrap()["type"].as_str().unwrap(),
        wire::vm::WIRE_SESSION_START
    );
}

#[test]
fn wait_for_event_channel_closed_returns_shutdown() {
    let bus = Arc::new(crate::infra::DefaultEventBus::new());
    let d = Arc::new(HostApiDispatcher::new(bus));
    d.register_event_channel("s1/p1", 4);

    // Drop the sender side to close the channel
    d.event_senders.remove("s1/p1");

    let resp = d
        .do_wait_for_event("s1/p1", &serde_json::json!({}))
        .unwrap();
    assert!(resp.ok);
    assert_eq!(resp.data.as_ref().unwrap()["type"], "__shutdown");
}

#[test]
fn cleanup_instance_removes_event_channels() {
    let bus = Arc::new(crate::infra::DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    d.register_event_channel("s1/p1", 4);
    assert!(d.get_event_sender("s1/p1").is_some());

    d.cleanup_instance("s1/p1");
    assert!(d.get_event_sender("s1/p1").is_none());
}

#[test]
fn deliver_event_no_channel_returns_err() {
    let bus = Arc::new(crate::infra::DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    let r = d.deliver_event(
        "nonexistent",
        EventEnvelope {
            event_type: "x".into(),
            data: serde_json::json!(null),
            context: serde_json::json!(null),
        },
    );
    assert!(r.is_err());
}
