//! Black-box integration coverage for the stdio MCP connector.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serial_test::serial;
use tokio_util::sync::CancellationToken;
use tomcat::api::chat::ChatContextOverrides;
use tomcat::{
    init_context_state, run_chat_turn, AppConfig, AppError, Capabilities, ChatContext, ChatMessage,
    ChatRequest, ChatResponse, LlmProvider, LlmResolver, LlmScene, ResolvedCall, StreamEvent,
};

struct EnvGuard {
    key: &'static str,
    old: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let old = std::env::var_os(key);
        unsafe { std::env::set_var(key, value) };
        Self { key, old }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.old {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

fn fixture_path() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mcp/fake_stdio_server.mjs")
        .to_string_lossy()
        .into_owned()
}

fn write_mcp_config(path: &std::path::Path, source: serde_json::Value) {
    std::fs::create_dir_all(path.parent().expect("MCP config parent")).expect("MCP config parent");
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&source).expect("serialize MCP config"),
    )
    .expect("write MCP config");
}

fn fake_mcp_config(name: &str) -> serde_json::Value {
    fake_mcp_config_with_args(name, &[])
}

fn fake_mcp_config_with_args(name: &str, extra_args: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "mcpServers": {
            name: {
                "command": "node",
                "args": std::iter::once(fixture_path())
                    .chain(extra_args.iter().map(|arg| (*arg).to_string()))
                    .collect::<Vec<_>>(),
            }
        }
    })
}

fn connector_context(
    temp: &tempfile::TempDir,
    workspace: std::path::PathBuf,
    api_key_env: &str,
) -> ChatContext {
    let work_dir = temp.path().join("work");
    std::fs::create_dir_all(&work_dir).expect("create test work directory");
    std::fs::write(
        work_dir.join("models.toml"),
        format!(
            r#"[[models]]
id = "connector-test"
model_name = "connector-test"
api = "openai-responses"
provider = "connector-test"
api_key_env = "{api_key_env}"
base_url = "http://127.0.0.1:9"
capabilities = {{ vision = true, files = true, tools = true, reasoning = true, web_search = false }}
"#
        ),
    )
    .expect("write test models.toml");

    let mut cfg = AppConfig::default();
    cfg.connector.enabled = true;
    cfg.skills.enabled = false;
    cfg.llm.default_model = "connector-test".to_string();
    cfg.storage.work_dir = Some(work_dir.to_string_lossy().into_owned());
    ChatContext::from_config_with_overrides(
        cfg,
        ChatContextOverrides::default().with_session_cwd_override(workspace),
    )
    .expect("chat context")
}

async fn start_connectors(ctx: &ChatContext) {
    let connectors = ctx
        .global_services
        .connector_registry
        .as_ref()
        .expect("connector module enabled")
        .clone();
    connectors.spawn_connect_all().await;
}

async fn wait_for_deferred_tool(ctx: &ChatContext, name: &str, present: bool) {
    let manager = ctx
        .global_services
        .connector_registry
        .as_ref()
        .expect("connector module enabled")
        .mcp_manager();
    for _ in 0..50 {
        let found = manager
            .list_servers()
            .iter()
            .flat_map(|source| manager.tool_defs(&source.name))
            .any(|tool| tool.model_name == name);
        if found == present {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!(
        "deferred MCP tool '{name}' did not become {}",
        if present { "available" } else { "unavailable" }
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn pending_project_server_is_absent_until_project_trusted() {
    const API_KEY_ENV: &str = "TOMCAT_CONNECTOR_PROJECT_TEST_KEY";

    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    let _api_key = EnvGuard::set(API_KEY_ENV, "test-key");
    write_mcp_config(
        &workspace.join(".agents").join("mcp.json"),
        fake_mcp_config("project-fake"),
    );
    let ctx = connector_context(&temp, workspace, API_KEY_ENV);
    start_connectors(&ctx).await;

    wait_for_deferred_tool(&ctx, "mcp__project-fake__capture", false).await;
    let connectors = ctx
        .global_services
        .connector_registry
        .as_ref()
        .expect("connector module enabled");
    let project_root = connectors
        .mcp_manager()
        .project_root()
        .unwrap()
        .to_path_buf();
    tomcat::core::security::project_trust::ProjectTrustStore::open(&ctx.config)
        .unwrap()
        .trust(&project_root)
        .unwrap();
    tomcat::core::connector::ConnectorRegistry::project_trusted(&ctx.config, &project_root)
        .expect("wake project MCP sources");
    wait_for_deferred_tool(&ctx, "mcp__project-fake__capture", true).await;
    assert!(
        ctx.global_services
            .tool_registry
            .get_tool("mcp__project-fake__capture")
            .await
            .is_err(),
        "ready MCP tools must remain deferred and outside ToolRegistry"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn pasted_cursor_style_mcp_snippet_connects_without_translation() {
    const API_KEY_ENV: &str = "TOMCAT_CONNECTOR_GLOBAL_TEST_KEY";

    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    let _api_key = EnvGuard::set(API_KEY_ENV, "test-key");
    write_mcp_config(
        &temp.path().join("work").join("mcp.json"),
        fake_mcp_config("pasted"),
    );
    let ctx = connector_context(&temp, workspace, API_KEY_ENV);
    start_connectors(&ctx).await;

    wait_for_deferred_tool(&ctx, "mcp__pasted__capture", true).await;
}

#[tokio::test]
async fn sdk_m0_stdio_fixture_accepts_overlapping_calls_and_isolated_cancel() {
    use rmcp::{service::PeerRequestOptions, ServiceExt};
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    let gate = temp.path().join("release-A");
    let events = temp.path().join("events.jsonl");
    let mut command = tokio::process::Command::new("node");
    command.arg(fixture_path()).arg("--events").arg(&events);
    let transport = rmcp::transport::TokioChildProcess::new(command).unwrap();
    let client = ().serve(transport).await.unwrap();
    let mut calls = Vec::new();
    for index in 0..16 {
        let args = serde_json::json!({"label":format!("A-{index}"), "gateFile":gate});
        calls.push(
            client
                .peer()
                .send_request_with_option(
                    rmcp::model::ClientRequest::CallToolRequest(rmcp::model::Request::new(
                        rmcp::model::CallToolRequestParams::new("capture")
                            .with_arguments(args.as_object().unwrap().clone()),
                    )),
                    PeerRequestOptions::with_timeout(Duration::from_secs(5))
                        .reset_timeout_on_progress(),
                )
                .await
                .unwrap(),
        );
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let text = tokio::fs::read_to_string(&events).await.unwrap_or_default();
            let started = text
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .filter(|event| event["kind"] == "started")
                .count();
            if started == 16 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("all 16 requests reach stdio before any gate opens");
    calls
        .remove(0)
        .cancel(Some("cancel only A-0".into()))
        .await
        .unwrap();
    let b = tokio::time::timeout(
        Duration::from_secs(2),
        client.peer().call_tool(
            rmcp::model::CallToolRequestParams::new("status").with_arguments(
                serde_json::json!({"label":"B"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(serde_json::to_string(&b)
        .unwrap()
        .contains("fixture result: B"));
    tokio::fs::write(&gate, "release").await.unwrap();
    for request in calls {
        request.await_response().await.unwrap();
    }
    wait_stdio_exits(&events, 17).await;
    let text = tokio::fs::read_to_string(&events).await.unwrap();
    assert_eq!(
        text.lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|event| event["kind"] == "cancelled")
            .count(),
        1
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn sdk_m0_stdio_unit_handler_progress_extends_wait() {
    use rmcp::{service::PeerRequestOptions, ServiceExt};
    use std::time::Duration;
    let mut command = tokio::process::Command::new("node");
    command.arg(fixture_path());
    let client = ().serve(rmcp::transport::TokioChildProcess::new(command).unwrap()).await.unwrap();
    let args = serde_json::json!({"label":"progress", "delayMs":900, "progressMs":50});
    let request = client
        .peer()
        .send_request_with_option(
            rmcp::model::ClientRequest::CallToolRequest(rmcp::model::Request::new(
                rmcp::model::CallToolRequestParams::new("capture")
                    .with_arguments(args.as_object().unwrap().clone()),
            )),
            PeerRequestOptions::with_timeout(Duration::from_millis(300))
                .reset_timeout_on_progress(),
        )
        .await
        .unwrap();
    let start = tokio::time::Instant::now();
    request
        .await_response()
        .await
        .expect("stdio progress extends I");
    assert!(start.elapsed() >= Duration::from_millis(800));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn manager_stdio_default_quota_and_single_call_cancel_are_shared_across_sessions() {
    use std::time::Duration;
    use tomcat::core::connector::mcp::manager::{McpCallContext, McpManager};
    for limit in [None, Some(1)] {
        let temp = tempfile::tempdir().unwrap();
        let events = temp.path().join("events.jsonl");
        let methods = temp.path().join("methods.log");
        let gate = temp.path().join("release");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        if let Some(n) = limit {
            cfg.connector.mcp.max_concurrent_calls = n;
        }
        let config = fake_mcp_config_with_args(
            "stdio",
            &[
                "--events",
                events.to_str().unwrap(),
                "--record",
                methods.to_str().unwrap(),
            ],
        );
        write_mcp_config(&temp.path().join("work/mcp.json"), config);
        let manager = McpManager::new(&cfg, None).unwrap();
        manager.connect_server("stdio").await.unwrap();
        let n = cfg.connector.mcp.max_concurrent_calls;
        assert_eq!(n, limit.unwrap_or(16));
        let mut active = Vec::new();
        let cancel_a = CancellationToken::new();
        for index in 0..n {
            let manager = manager.clone();
            let parent = if index == 0 {
                cancel_a.clone()
            } else {
                CancellationToken::new()
            };
            let context =
                McpCallContext::new(format!("agent-session-{index}"), "same-outer-id", &parent);
            let args = serde_json::json!({"label":format!("A-{index}"), "gateFile":gate});
            active.push(tokio::spawn(async move {
                manager
                    .call_model_tool_with_context("mcp__stdio__capture", args, context)
                    .await
            }));
        }
        let started = || async {
            tokio::fs::read_to_string(&events)
                .await
                .unwrap_or_default()
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .filter(|event| event["kind"] == "started")
                .count()
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while started().await != n {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("all default 16 (or explicit 1) calls reach the stdio server");
        let queued = {
            let manager = manager.clone();
            tokio::spawn(async move {
                manager
                    .call_model_tool(
                        "mcp__stdio__capture",
                        serde_json::json!({"label":"queued-B"}),
                    )
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(started().await, n);
        assert!(!queued.is_finished());
        cancel_a.cancel();
        assert!(active.remove(0).await.unwrap().is_err());
        let b = tokio::time::timeout(Duration::from_secs(5), queued)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(b["content"][0]["text"], "fixture result: queued-B");
        assert!(active.iter().all(|call| !call.is_finished()));
        tokio::fs::write(&gate, "release").await.unwrap();
        for (index, call) in active.into_iter().enumerate() {
            assert_eq!(
                call.await.unwrap().unwrap()["content"][0]["text"],
                format!("fixture result: A-{}", index + 1)
            );
        }
        wait_stdio_exits(&events, n + 1).await;
        let log = tokio::fs::read_to_string(&methods).await.unwrap();
        assert_eq!(
            log.lines().filter(|method| *method == "tools/list").count(),
            1
        );
        assert_eq!(
            log.lines().filter(|method| *method == "tools/call").count(),
            n + 1
        );
        manager.remove_configured_server("stdio", &cfg).unwrap();
    }
}

#[tokio::test]
async fn manager_stdio_progress_exceeds_window_without_renewing_a_sibling() {
    use tomcat::core::connector::mcp::manager::McpManager;
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    cfg.connector.mcp.call_timeout_ms = 300;
    let config = fake_mcp_config("progress");
    write_mcp_config(&temp.path().join("work/mcp.json"), config);
    let manager = McpManager::new(&cfg, None).unwrap();
    manager.connect_server("progress").await.unwrap();
    let a = {
        let manager = manager.clone();
        tokio::spawn(async move {
            manager
                .call_model_tool(
                    "mcp__progress__capture",
                    serde_json::json!({"label":"A", "delayMs":900, "progressMs":40}),
                )
                .await
        })
    };
    let b = manager
        .call_model_tool(
            "mcp__progress__status",
            serde_json::json!({"label":"B", "delayMs":900, "progressMs":40, "wrongToken":true}),
        )
        .await;
    assert!(b.unwrap_err().to_string().contains("idle wait timed out"));
    assert!(!a.is_finished());
    assert_eq!(
        a.await.unwrap().unwrap()["content"][0]["text"],
        "fixture result: A"
    );
    manager.remove_configured_server("progress", &cfg).unwrap();
}

async fn wait_stdio_exits(events: &std::path::Path, expected: usize) {
    use std::time::Duration;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let log = tokio::fs::read_to_string(events).await.unwrap_or_default();
            let exited = log
                .lines()
                .filter_map(|row| serde_json::from_str::<serde_json::Value>(row).ok())
                .filter(|event| event["kind"] == "exited")
                .count();
            if exited == expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("stdio fixture tasks and event writes drained before removing temporary files");
}

struct RecordingMockLlm {
    streams: Mutex<VecDeque<Vec<Result<StreamEvent, AppError>>>>,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
}

impl RecordingMockLlm {
    fn new(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> Self {
        Self {
            streams: Mutex::new(streams.into()),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl LlmProvider for RecordingMockLlm {
    fn provider_name(&self) -> &str {
        "connector-mock"
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm("mock chat is not used".to_string()))
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        self.requests.lock().expect("requests lock").push(req);
        let stream = self
            .streams
            .lock()
            .expect("streams lock")
            .pop_front()
            .ok_or_else(|| AppError::Llm("unexpected mock request".to_string()))?;
        Ok(Box::new(tokio_stream::iter(stream)))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

struct FixedResolver {
    provider: Arc<dyn LlmProvider>,
}

impl LlmResolver for FixedResolver {
    fn resolve(
        &self,
        _scene: LlmScene,
        _session_override: Option<&str>,
    ) -> Result<ResolvedCall, AppError> {
        let mut call =
            ResolvedCall::from_parts_unchecked(self.provider.clone(), "gpt-5.4", "gpt-5.4");
        call.api = "openai-responses".to_string();
        call.provider = "openai".to_string();
        call.capabilities = Capabilities {
            vision: true,
            files: true,
            tools: true,
            reasoning: true,
            web_search: false,
        };
        Ok(call)
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn fake_stdio_server_end_to_end_image_reflow() {
    const API_KEY_ENV: &str = "TOMCAT_CONNECTOR_IMAGE_TEST_KEY";

    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    let _api_key = EnvGuard::set(API_KEY_ENV, "test-key");
    write_mcp_config(
        &temp.path().join("work").join("mcp.json"),
        fake_mcp_config("fake"),
    );
    let mut ctx = connector_context(&temp, workspace, API_KEY_ENV);
    start_connectors(&ctx).await;
    wait_for_deferred_tool(&ctx, "mcp__fake__capture", true).await;

    let provider = Arc::new(RecordingMockLlm::new(vec![
        vec![
            Ok(StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("search-1".to_string()),
                name: Some("tool_search".to_string()),
                arguments_delta: Some("{}".to_string()),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "tool_calls".to_string(),
            }),
        ],
        vec![
            Ok(StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("describe-1".to_string()),
                name: Some("tool_describe".to_string()),
                arguments_delta: Some(
                    serde_json::json!({
                        "names": ["mcp__fake__capture"]
                    })
                    .to_string(),
                ),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "tool_calls".to_string(),
            }),
        ],
        vec![
            Ok(StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("deferred-capture-1".to_string()),
                name: Some("tool_call".to_string()),
                arguments_delta: Some(
                    serde_json::json!({
                        "name": "mcp__fake__capture",
                        "arguments": {}
                    })
                    .to_string(),
                ),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "tool_calls".to_string(),
            }),
        ],
        vec![
            Ok(StreamEvent::ContentDelta {
                delta: "capture accepted".to_string(),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "stop".to_string(),
            }),
        ],
    ]));
    let requests = provider.requests.clone();
    ctx.global_services.llm_resolver = Arc::new(FixedResolver {
        provider: provider.clone(),
    });

    let system_prompt = "test system prompt";
    let mut context_state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        system_prompt,
    )
    .expect("context state");
    let outcome = run_chat_turn(
        &ctx,
        "capture an image",
        system_prompt,
        &mut context_state,
        CancellationToken::new(),
    )
    .await
    .expect("tool turn");
    assert!(!outcome.is_interrupted());

    let reflowed_image = requests
        .lock()
        .expect("requests lock")
        .iter()
        .flat_map(|request| request.messages.iter())
        .any(|message| {
            serde_json::to_string(message)
                .expect("serialize message")
                .contains(r#""type":"input_image""#)
        });
    assert!(
        reflowed_image,
        "deferred tool_call image result must become an InputImage in the next model request"
    );
    let requests = requests.lock().expect("requests lock");
    assert!(
        requests.iter().all(|request| {
            request.tools.as_ref().into_iter().flatten().all(|tool| {
                !tool["function"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with("mcp__")
            })
        }),
        "the changing MCP catalog may appear in tool results but never in prompt-facing tools"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn deferred_tool_call_error_preserves_mcp_is_error() {
    const API_KEY_ENV: &str = "TOMCAT_CONNECTOR_ERROR_TEST_KEY";

    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    let _api_key = EnvGuard::set(API_KEY_ENV, "test-key");
    write_mcp_config(
        &temp.path().join("work").join("mcp.json"),
        fake_mcp_config_with_args("fake", &["--error"]),
    );
    let mut ctx = connector_context(&temp, workspace, API_KEY_ENV);
    start_connectors(&ctx).await;
    wait_for_deferred_tool(&ctx, "mcp__fake__capture", true).await;

    let tool_execution_events = Arc::new(Mutex::new(Vec::new()));
    let recorded_events = tool_execution_events.clone();
    ctx.global_services.event_bus.on(
        "tool_execution_end",
        Box::new(move |event| {
            if event.payload["toolCallId"] == "deferred-error-1" {
                recorded_events
                    .lock()
                    .expect("recorded events lock")
                    .push(event.payload);
            }
            Ok(())
        }),
    );

    let provider = Arc::new(RecordingMockLlm::new(vec![
        vec![
            Ok(StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("deferred-error-1".to_string()),
                name: Some("tool_call".to_string()),
                arguments_delta: Some(
                    serde_json::json!({
                        "name": "mcp__fake__capture",
                        "arguments": {},
                    })
                    .to_string(),
                ),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "tool_calls".to_string(),
            }),
        ],
        vec![
            Ok(StreamEvent::ContentDelta {
                delta: "error received".to_string(),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "stop".to_string(),
            }),
        ],
    ]));
    ctx.global_services.llm_resolver = Arc::new(FixedResolver { provider });

    let system_prompt = "test system prompt";
    let mut context_state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        system_prompt,
    )
    .expect("context state");
    run_chat_turn(
        &ctx,
        "trigger a deferred MCP tool error",
        system_prompt,
        &mut context_state,
        CancellationToken::new(),
    )
    .await
    .expect("tool turn");

    let events = tool_execution_events.lock().expect("recorded events lock");
    assert_eq!(
        events.len(),
        1,
        "the deferred tool call must emit one end event"
    );
    assert_eq!(events[0]["toolName"], "tool_call");
    assert_eq!(events[0]["isError"], true);
    assert_eq!(events[0]["result"], "fake tool error");
}
