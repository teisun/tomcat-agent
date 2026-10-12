//! Offline end-to-end coverage for the native `package_install` tool.
//!
//! This test deliberately crosses the LLM tool dispatcher, package backend, resource
//! registry and QuickJS boundary. Unit tests cover individual authorization branches;
//! this file protects the user-visible installation flow from drifting apart.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tomcat::core::agent_loop::{PackageInstallBackend, PackageInstallRequest};
use tomcat::core::permission::{DefaultPermissionGate, GateConfig, PermissionGate, SessionGrants};
use tomcat::core::tools::package_install::{ChatPackageInstallBackend, PackageInstallContext};
use tomcat::{
    AgentLoop, AgentLoopConfig, AllowAllConfirmation, AppConfig, AppError, AuditStore, ChatMessage,
    ChatRequest, ChatResponse, DefaultEventBus, DefaultPrimitiveExecutor, DefaultToolRegistry,
    FunctionRegistry, HostApiDispatcher, LlmProvider, PluginEngine, PluginFunctionInvoker,
    PluginManager, PluginRuntimeManager, PluginToolExecutor, PrimitiveConfig, ResolvedCall,
    SharedPluginRuntimeManager, StreamEvent, ToolRegistry, TracingAuditRecorder,
};

struct RecordingScriptedLlm {
    streams: Mutex<VecDeque<Vec<Result<StreamEvent, AppError>>>>,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
}

impl RecordingScriptedLlm {
    fn new(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> Self {
        Self {
            streams: Mutex::new(streams.into()),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl LlmProvider for RecordingScriptedLlm {
    fn provider_name(&self) -> &str {
        "package-install-test"
    }

    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm("test only uses streaming".to_string()))
    }

    async fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        self.requests.lock().unwrap().push(request);
        let events = self
            .streams
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| AppError::Llm("script exhausted".to_string()))?;
        Ok(Box::new(tokio_stream::iter(events)))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

fn tool_call_stream(
    id: &str,
    name: &str,
    args: serde_json::Value,
) -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some(id.to_string()),
            name: Some(name.to_string()),
            arguments_delta: Some(args.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ]
}

fn text_stream(text: &str) -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::ContentDelta {
            delta: text.to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ]
}

fn test_config(work_dir: &Path) -> AppConfig {
    let mut config = AppConfig::default();
    config.storage.work_dir = Some(work_dir.to_string_lossy().into_owned());
    config.security.enable_audit_log = true;
    config
}

fn test_gate(workspace: &Path) -> Arc<dyn PermissionGate> {
    DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: workspace.join("definition"),
            workspace_roots: vec![workspace.to_path_buf()],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: true,
        },
        SessionGrants::new(),
    )
    .into_arc()
}

fn package_backend(
    config: AppConfig,
    workspace: &Path,
    audit_store: Arc<AuditStore>,
) -> Arc<dyn PackageInstallBackend> {
    Arc::new(ChatPackageInstallBackend {
        ctx: PackageInstallContext::new(
            config,
            workspace.to_path_buf(),
            Some(workspace.to_path_buf()),
            Arc::new(AllowAllConfirmation),
        )
        .with_audit_store(audit_store),
    })
}

fn creator_skill_source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("skills")
        .join("skill-creator")
}

fn function_plugin_source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("function_echo_plugin")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn package_install_dispatcher_installs_creator_skill_and_exposes_result_next_turn() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let work_dir = temp.path().join("tomcat-work");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&work_dir).unwrap();
    let config = test_config(&work_dir);
    let audit_store = Arc::new(AuditStore::open_if_enabled(&config).unwrap().unwrap());
    let backend = package_backend(config.clone(), &workspace, audit_store);
    let source = creator_skill_source();
    assert!(
        source.join("SKILL.md").is_file(),
        "real creator asset missing"
    );

    let llm = Arc::new(RecordingScriptedLlm::new(vec![
        tool_call_stream(
            "install-scope",
            "package_install",
            serde_json::json!({ "source": source, "scope": "scope" }),
        ),
        text_stream("installed the creator skill"),
    ]));
    let primitive = Arc::new(DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        test_gate(&workspace),
    ));
    let mut agent = AgentLoop::new(
        ResolvedCall::from_parts_unchecked(llm.clone(), "test", "test"),
        primitive,
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "package-install-dispatcher".to_string(),
            max_attempts: 1,
            system_prompt: None,
            retry_base_delay_ms: 0,
            ..Default::default()
        },
        CancellationToken::new(),
    )
    .with_package_install_backend(backend);

    let result = agent
        .run(vec![ChatMessage::user("Install the project skill creator")])
        .await
        .unwrap();

    assert!(result.final_text.contains("installed"));
    assert!(
        workspace
            .join(".agents/skills/skill-creator/SKILL.md")
            .is_file(),
        "scope install must put the real creator asset in the project layer"
    );
    let requests = llm.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "tool call must be followed by a second LLM turn"
    );
    assert!(requests[1].messages.iter().any(|message| {
        message.role == tomcat::core::llm::ChatMessageRole::Tool
            && message
                .text_content()
                .is_some_and(|text| text.contains("\"package\":\"skill-creator\""))
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn package_install_supports_all_layers_and_loaded_plugin_executes_in_quickjs() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let work_dir = temp.path().join("tomcat-work");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&work_dir).unwrap();
    let config = test_config(&work_dir);
    let audit_store = Arc::new(AuditStore::open_if_enabled(&config).unwrap().unwrap());
    let backend = package_backend(config, &workspace, audit_store);

    for scope in ["agent", "global"] {
        let installed = backend
            .install(
                PackageInstallRequest::parse(&serde_json::json!({
                    "source": creator_skill_source(),
                    "scope": scope,
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(installed.scope, scope);
        assert!(Path::new(&installed.target_path).is_dir());
        assert_eq!(installed.resources[0].id, "skill-creator");
    }

    let plugin = backend
        .install(
            PackageInstallRequest::parse(&serde_json::json!({
                "source": function_plugin_source(),
                "scope": "scope",
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let plugin_dir = PathBuf::from(plugin.target_path).join("plugins/function-echo-plugin");
    assert!(plugin_dir.join("plugin.json").is_file());

    let event_bus = Arc::new(DefaultEventBus::new());
    let mut manager = Arc::new(PluginManager::new(event_bus.clone()));
    let runtime_manager: SharedPluginRuntimeManager = Arc::new(PluginRuntimeManager::new());
    let manager_mut = Arc::get_mut(&mut manager).unwrap();
    manager_mut.set_plugin_engine(PluginEngine::global(None).unwrap());
    manager_mut.set_plugin_runtime_manager(runtime_manager);
    manager_mut.set_audit_recorder(Arc::new(TracingAuditRecorder));

    let tool_executor = PluginToolExecutor::new(Arc::downgrade(&manager));
    let tool_registry = Arc::new(DefaultToolRegistry::new(
        tool_executor.clone(),
        Arc::new(TracingAuditRecorder),
    ));
    let host_dispatcher = Arc::new(
        HostApiDispatcher::new(event_bus)
            .with_tokio_handle(tokio::runtime::Handle::current())
            .with_tools(tool_registry.clone()),
    );
    let function_registry = Arc::new(FunctionRegistry::new());
    let function_invoker = PluginFunctionInvoker::new(Arc::downgrade(&manager));
    tool_executor.attach_dispatcher(Arc::downgrade(&host_dispatcher));
    function_invoker.attach_dispatcher(Arc::downgrade(&host_dispatcher));
    manager.set_tool_registry(tool_registry as Arc<dyn ToolRegistry>);
    manager.set_function_registry(function_registry.clone());
    manager.set_host_dispatcher(host_dispatcher);

    let manifest =
        tomcat::parse_manifest(&std::fs::read_to_string(plugin_dir.join("plugin.json")).unwrap())
            .unwrap();
    function_registry.register_plugin_functions(&manifest.id, &plugin_dir, &manifest.functions);
    manager
        .register_catalog_plugin(&plugin_dir, manifest)
        .unwrap();
    manager.load_plugin(&plugin_dir).unwrap();

    let target = function_registry
        .functions_for_point("test.echo")
        .into_iter()
        .next()
        .unwrap();
    let result = function_invoker
        .execute(
            &target,
            serde_json::json!({ "text": "installed" }),
            Some("session-1"),
        )
        .await
        .unwrap();
    assert_eq!(result["echoed"], "installed");
    manager.end_session("session-1").await.unwrap();
}
