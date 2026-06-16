//! # `HostApiDispatcher` 注入扩展后的成功路径
//!
//! 覆盖：
//!
//! - `with_primitive`：read/write/edit/executeBash 都通过 `MockPrimitive`，
//!   并验证 `executeBash` 的 argv 形参可以原样下发到自定义实现。
//! - `with_llm`：chat / chat_stream / chat_parses_max_tokens_and_temperature。
//! - `with_tools`：register_tool / list_tools / call_tool / unregister_tool /
//!   get_active_tools / set_active_tools / register_command 系列；以及
//!   `registerTool` 缺 name 的负向断言。
//! - `with_session`：getCurrentSession / getMessages / sendMessage 走通。
//! - `register_command_records_metadata`：插件命令注册后能从 dispatcher 取回。
//! - `normalize_tool_parameters_unwraps_schema`：辅助函数解包 `schema` 包装。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::super::helpers::{normalize_tool_parameters, parse_chat_request};
use super::super::HostApiDispatcher;
use super::mocks::{MockLlm, MockPrimitive, MockToolRegistry};
use crate::core::llm::thinking_policy::ThinkingFormat;
use crate::core::{
    BashResult, Capabilities, ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice, DirEntry,
    EditFileResult, EditOperation, LlmProvider, LlmResolver, LlmScene, PrimitiveExecutor,
    PrimitiveOperation, ResolvedCall, SessionManager, StreamEvent, WriteFileResult,
};
use crate::ext::host_binding::HostRequest;
use crate::infra::error::AppError;
use crate::infra::DefaultEventBus;

#[derive(Clone)]
struct RecordingLlm {
    reply: &'static str,
    calls: Arc<AtomicUsize>,
    seen_models: Arc<Mutex<Vec<String>>>,
}

impl RecordingLlm {
    fn new(reply: &'static str) -> (Self, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen_models = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                reply,
                calls: Arc::clone(&calls),
                seen_models: Arc::clone(&seen_models),
            },
            calls,
            seen_models,
        )
    }
}

#[async_trait::async_trait]
impl LlmProvider for RecordingLlm {
    fn provider_name(&self) -> &str {
        self.reply
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen_models.lock().unwrap().push(req.model.clone());
        Ok(ChatResponse {
            id: Some(format!("{}-id", self.reply)),
            choices: vec![ChatResponseChoice {
                index: 0,
                message: ChatMessage::assistant(self.reply),
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<
        Box<dyn futures_util::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen_models.lock().unwrap().push(req.model.clone());
        use futures_util::stream;
        Ok(Box::new(stream::iter(vec![Ok(
            StreamEvent::ContentDelta {
                delta: self.reply.to_string(),
            },
        )])))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

struct StaticResolver {
    provider: Arc<dyn LlmProvider>,
    seen_models: Arc<Mutex<Vec<String>>>,
}

impl LlmResolver for StaticResolver {
    fn resolve(
        &self,
        scene: LlmScene,
        session_override: Option<&str>,
    ) -> Result<ResolvedCall, AppError> {
        assert_eq!(scene, LlmScene::Main);
        let model = session_override.unwrap_or_default().to_string();
        self.seen_models.lock().unwrap().push(model.clone());
        Ok(ResolvedCall {
            provider_impl: Arc::clone(&self.provider),
            model,
            api: "openai".to_string(),
            provider: "mimo".to_string(),
            base_url: None,
            key_source: "TEST_KEY".to_string(),
            thinking_format: ThinkingFormat::Openai,
            capabilities: Capabilities::default(),
        })
    }
}

#[derive(Clone)]
struct CapturingLlm {
    seen_max_tokens: Arc<Mutex<Vec<Option<u32>>>>,
    seen_temperatures: Arc<Mutex<Vec<Option<f32>>>>,
}

type SeenMaxTokens = Arc<Mutex<Vec<Option<u32>>>>;
type SeenTemperatures = Arc<Mutex<Vec<Option<f32>>>>;

impl CapturingLlm {
    fn new() -> (Self, SeenMaxTokens, SeenTemperatures) {
        let seen_max_tokens = Arc::new(Mutex::new(Vec::new()));
        let seen_temperatures = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                seen_max_tokens: Arc::clone(&seen_max_tokens),
                seen_temperatures: Arc::clone(&seen_temperatures),
            },
            seen_max_tokens,
            seen_temperatures,
        )
    }
}

#[async_trait::async_trait]
impl LlmProvider for CapturingLlm {
    fn provider_name(&self) -> &str {
        "capturing"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, AppError> {
        self.seen_max_tokens.lock().unwrap().push(req.max_tokens);
        self.seen_temperatures.lock().unwrap().push(req.temperature);
        Ok(ChatResponse {
            id: Some("capturing-id".to_string()),
            choices: vec![ChatResponseChoice {
                index: 0,
                message: ChatMessage::assistant("captured"),
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<
        Box<dyn futures_util::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        self.seen_max_tokens.lock().unwrap().push(req.max_tokens);
        self.seen_temperatures.lock().unwrap().push(req.temperature);
        use futures_util::stream;
        Ok(Box::new(stream::iter(vec![Ok(
            StreamEvent::ContentDelta {
                delta: "captured".to_string(),
            },
        )])))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
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
                bytes_written: 0,
                diff_hint: None,
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
            _timeout_ms: Option<u64>,
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
                ..Default::default()
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

#[test]
fn parse_chat_request_forwards_tools() {
    let req = parse_chat_request(&serde_json::json!({
        "messages": [{"role": "user", "content": "hi"}],
        "model": "mimo-mini",
        "tools": [{
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "search the web",
                "parameters": {"type": "object", "properties": {}}
            }
        }]
    }))
    .expect("chat request");

    let tools = req.tools.expect("tools should be forwarded");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"], serde_json::json!("function"));
    assert_eq!(
        tools[0]["function"]["name"],
        serde_json::json!("web_search")
    );
}

#[tokio::test]
async fn dispatch_chat_with_llm_returns_ok() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus).with_llm(Arc::new(MockLlm));
    let req = HostRequest {
        module: "llm".to_string(),
        method: "createChatCompletion".to_string(),
        params: serde_json::json!({ "messages": [], "model": "default" }),
        call_id: None,
    };
    let res = d.dispatch_async("inst-1", req).await.unwrap();
    assert!(res.ok);
}

#[tokio::test]
async fn dispatch_chat_routes_named_model_via_resolver() {
    let bus = Arc::new(DefaultEventBus::new());
    let (global_llm, global_calls, _) = RecordingLlm::new("global");
    let (resolved_llm, resolved_calls, resolved_models) = RecordingLlm::new("resolved");
    let resolver_models = Arc::new(Mutex::new(Vec::new()));
    let resolver = StaticResolver {
        provider: Arc::new(resolved_llm),
        seen_models: Arc::clone(&resolver_models),
    };
    let d = HostApiDispatcher::new(bus)
        .with_llm(Arc::new(global_llm))
        .with_llm_resolver(Arc::new(resolver));
    let req = HostRequest {
        module: "llm".to_string(),
        method: "createChatCompletion".to_string(),
        params: serde_json::json!({ "messages": [], "model": "mimo-v2.5-pro" }),
        call_id: None,
    };

    let res = d.dispatch_async("inst-1", req).await.unwrap();
    assert!(res.ok);
    assert_eq!(global_calls.load(Ordering::SeqCst), 0);
    assert_eq!(resolved_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        resolver_models.lock().unwrap().clone(),
        vec!["mimo-v2.5-pro".to_string()]
    );
    assert_eq!(
        resolved_models.lock().unwrap().clone(),
        vec!["mimo-v2.5-pro".to_string()]
    );
    assert_eq!(
        res.data
            .as_ref()
            .and_then(|d| d["choices"][0]["message"]["content"].as_str()),
        Some("resolved")
    );
}

#[tokio::test]
async fn dispatch_chat_default_model_falls_back_to_global_llm() {
    let bus = Arc::new(DefaultEventBus::new());
    let (global_llm, global_calls, global_models) = RecordingLlm::new("global");
    let (resolved_llm, resolved_calls, _) = RecordingLlm::new("resolved");
    let resolver_models = Arc::new(Mutex::new(Vec::new()));
    let resolver = StaticResolver {
        provider: Arc::new(resolved_llm),
        seen_models: Arc::clone(&resolver_models),
    };
    let d = HostApiDispatcher::new(bus)
        .with_llm(Arc::new(global_llm))
        .with_llm_resolver(Arc::new(resolver));
    let req = HostRequest {
        module: "llm".to_string(),
        method: "createChatCompletion".to_string(),
        params: serde_json::json!({ "messages": [], "model": "default" }),
        call_id: None,
    };

    let res = d.dispatch_async("inst-1", req).await.unwrap();
    assert!(res.ok);
    assert_eq!(global_calls.load(Ordering::SeqCst), 1);
    assert_eq!(resolved_calls.load(Ordering::SeqCst), 0);
    assert!(resolver_models.lock().unwrap().is_empty());
    assert_eq!(
        global_models.lock().unwrap().clone(),
        vec!["default".to_string()]
    );
}

#[tokio::test]
async fn dispatch_chat_stream_with_llm_returns_ok() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus).with_llm(Arc::new(MockLlm));
    let req = HostRequest {
        module: "llm".to_string(),
        method: "createChatCompletionStream".to_string(),
        params: serde_json::json!({ "messages": [], "model": "default" }),
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
async fn session_scoped_hostcall_isolation() {
    let bus = Arc::new(DefaultEventBus::new());
    let dir = tempfile::tempdir().unwrap();
    let mgr = Arc::new(SessionManager::new(dir.path().to_path_buf()));
    let key = mgr.current_session_key().to_string();
    let first = mgr
        .create_session(&key, Some("/workspace/alpha".to_string()))
        .unwrap();
    let second = mgr
        .create_session(&key, Some("/workspace/beta".to_string()))
        .unwrap();
    let d = HostApiDispatcher::new(bus).with_session(Arc::clone(&mgr));

    let current = d
        .dispatch_async(
            &format!("{}/demo-plugin", first.session_id),
            HostRequest {
                module: "session".to_string(),
                method: "getCurrentSession".to_string(),
                params: serde_json::json!({}),
                call_id: None,
            },
        )
        .await
        .unwrap();
    assert!(current.ok);
    assert_eq!(
        current
            .data
            .as_ref()
            .and_then(|data| data.get("sessionId"))
            .and_then(|value| value.as_str()),
        Some(first.session_id.as_str()),
        "dispatcher should resolve session from instance_id instead of current session pointer"
    );

    let cwd = d
        .dispatch_async(
            &format!("{}/demo-plugin", first.session_id),
            HostRequest {
                module: "context".to_string(),
                method: "getCwd".to_string(),
                params: serde_json::json!({}),
                call_id: None,
            },
        )
        .await
        .unwrap();
    assert!(cwd.ok);
    assert_eq!(
        cwd.data
            .as_ref()
            .and_then(|data| data.get("cwd"))
            .and_then(|value| value.as_str()),
        Some("/workspace/alpha")
    );

    let send = d
        .dispatch_async(
            &format!("{}/demo-plugin", first.session_id),
            HostRequest {
                module: "session".to_string(),
                method: "sendMessage".to_string(),
                params: serde_json::json!({
                    "message": { "role": "user", "content": "hello from alpha" }
                }),
                call_id: None,
            },
        )
        .await
        .unwrap();
    assert!(send.ok);

    let first_messages = d
        .dispatch_async(
            &format!("{}/demo-plugin", first.session_id),
            HostRequest {
                module: "session".to_string(),
                method: "getMessages".to_string(),
                params: serde_json::json!({ "cap": 10 }),
                call_id: None,
            },
        )
        .await
        .unwrap();
    assert!(first_messages.ok);
    assert!(
        first_messages
            .data
            .as_ref()
            .and_then(|data| data.as_array())
            .map(|messages| !messages.is_empty())
            .unwrap_or(false),
        "routed session should expose its own appended messages"
    );

    let second_messages = d
        .dispatch_async(
            &format!("{}/demo-plugin", second.session_id),
            HostRequest {
                module: "session".to_string(),
                method: "getMessages".to_string(),
                params: serde_json::json!({ "cap": 10 }),
                call_id: None,
            },
        )
        .await
        .unwrap();
    assert!(second_messages.ok);
    assert!(
        second_messages
            .data
            .as_ref()
            .and_then(|data| data.as_array())
            .map(|messages| messages.is_empty())
            .unwrap_or(false),
        "neighbor session should not observe routed messages"
    );

    let first_entries = mgr.get_entries_for_session(&first.session_id, 10).unwrap();
    let second_entries = mgr.get_entries_for_session(&second.session_id, 10).unwrap();
    assert!(
        first_entries.iter().any(|entry| serde_json::to_value(entry)
            .ok()
            .and_then(|value| value.get("message").cloned())
            .is_some()),
        "message should be appended to the routed session transcript"
    );
    assert!(
        second_entries
            .iter()
            .all(|entry| serde_json::to_value(entry)
                .ok()
                .and_then(|value| value.get("message").cloned())
                .is_none()),
        "neighbor session transcript should remain untouched"
    );
}

#[tokio::test]
async fn dispatch_session_hostcalls_can_route_across_bound_session_managers() {
    let bus = Arc::new(DefaultEventBus::new());
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let mgr_a = Arc::new(SessionManager::new(dir_a.path().to_path_buf()));
    let mgr_b = Arc::new(SessionManager::new(dir_b.path().to_path_buf()));
    let key_a = mgr_a.current_session_key().to_string();
    let key_b = mgr_b.current_session_key().to_string();
    let session_a = mgr_a
        .create_session(&key_a, Some("/workspace/alpha".to_string()))
        .unwrap();
    let session_b = mgr_b
        .create_session(&key_b, Some("/workspace/beta".to_string()))
        .unwrap();
    let d = HostApiDispatcher::new(bus).with_session(Arc::clone(&mgr_a));
    d.bind_session(&session_a.session_id, Arc::downgrade(&mgr_a));
    d.bind_session(&session_b.session_id, Arc::downgrade(&mgr_b));

    let cwd = d
        .dispatch_async(
            &format!("{}/demo-plugin", session_b.session_id),
            HostRequest {
                module: "context".to_string(),
                method: "getCwd".to_string(),
                params: serde_json::json!({}),
                call_id: None,
            },
        )
        .await
        .unwrap();
    assert!(cwd.ok);
    assert_eq!(
        cwd.data
            .as_ref()
            .and_then(|data| data.get("cwd"))
            .and_then(|value| value.as_str()),
        Some("/workspace/beta"),
        "dispatcher should use the bound session manager for the target session_id"
    );
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
    let (llm, seen_max_tokens, seen_temperatures) = CapturingLlm::new();
    let d = HostApiDispatcher::new(bus).with_llm(Arc::new(llm));
    let req = HostRequest {
        module: "llm".to_string(),
        method: "createChatCompletion".to_string(),
        params: serde_json::json!({
            "messages": [],
            "model": "default",
            "maxTokens": 100,
            "temperature": 0.7
        }),
        call_id: None,
    };
    let res = d.dispatch_async("inst-1", req).await.unwrap();
    assert!(res.ok);
    assert_eq!(seen_max_tokens.lock().unwrap().as_slice(), &[Some(100)]);
    let temperatures = seen_temperatures.lock().unwrap();
    assert_eq!(temperatures.len(), 1);
    let actual = temperatures[0].expect("temperature should be parsed");
    assert!(
        (actual - 0.7).abs() < f32::EPSILON,
        "expected 0.7, got {actual}"
    );
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
