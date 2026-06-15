mod common;

use async_trait::async_trait;
use futures_util::stream;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tomcat::{
    parse_manifest, BashResult, ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice,
    DefaultEventBus, DefaultToolRegistry, DirEntry, EditFileResult, EditOperation,
    FunctionRegistry, HostApiDispatcher, LlmProvider, PluginEngine, PluginEngineConfig,
    PluginFunctionInvoker, PluginInstance, PluginManager, PluginRuntimeManager, PluginStatus,
    PluginToolExecutor, PrimitiveExecutor, PrimitiveOperation, SharedPluginRuntimeManager,
    StreamEvent, Tool, ToolExecutor, ToolRegistry, TracingAuditRecorder, VmActorHandle,
    VmActorState, WriteFileResult,
};

type FunctionManagerHarness = (
    Arc<PluginFunctionInvoker>,
    Arc<FunctionRegistry>,
    Arc<DefaultToolRegistry>,
    Arc<PluginManager>,
    Arc<HostApiDispatcher>,
    SharedPluginRuntimeManager,
);

fn create_plugin_dir(id: &str, script: &str) -> tempfile::TempDir {
    create_plugin_dir_with_manifest(
        json!({
            "id": id,
            "name": id,
            "version": "0.1.0",
            "description": "quickjs test plugin",
            "author": "tests",
            "main": "main.js",
            "requiredPermissions": [],
            "requiredApiVersion": "1.0",
            "tags": [],
            "tools": [],
            "events": [],
            "activation": "lazy"
        }),
        script,
    )
}

fn create_plugin_dir_with_manifest(manifest: serde_json::Value, script: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("create temp plugin dir");
    std::fs::write(
        tmp.path().join("plugin.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::write(tmp.path().join("main.js"), script).unwrap();
    tmp
}

fn register_plugin(manager: &PluginManager, plugin_dir: &std::path::Path, plugin_id: &str) {
    let manifest_json = std::fs::read_to_string(plugin_dir.join("plugin.json")).unwrap();
    let manifest = parse_manifest(&manifest_json).unwrap();
    let manifest_tool_names = manifest
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();
    let manifest_functions = manifest.functions.clone();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    manager
        .register_plugin(PluginInstance {
            id: plugin_id.to_string(),
            manifest,
            plugin_vm_instance: None,
            status: PluginStatus::Loaded,
            registered_tools: manifest_tool_names,
            registered_functions: manifest_functions,
            registered_commands: vec![],
            event_listener_ids: vec![],
            config: serde_json::json!({}),
            created_at: now,
            loaded_at: now,
            plugin_root: plugin_dir.to_path_buf(),
        })
        .unwrap();
}

fn make_tool(tool_name: &str, plugin_id: &str) -> Tool {
    Tool {
        name: tool_name.to_string(),
        label: tool_name.to_string(),
        description: format!("{plugin_id}::{tool_name}"),
        parameters: json!({
            "type": "object",
            "properties": {},
        }),
        plugin_id: plugin_id.to_string(),
        is_enabled: true,
        created_at: 0,
    }
}

fn make_tool_manager(
    plugin_dir: &Path,
) -> (
    Arc<PluginToolExecutor>,
    Arc<PluginManager>,
    Arc<HostApiDispatcher>,
    SharedPluginRuntimeManager,
) {
    let event_bus = Arc::new(DefaultEventBus::new());
    let mut manager = Arc::new(PluginManager::new(event_bus.clone()));
    let runtime_manager: SharedPluginRuntimeManager = Arc::new(PluginRuntimeManager::new());

    let inner = Arc::get_mut(&mut manager).expect("plugin manager should be uniquely owned");
    inner.set_plugin_engine(PluginEngine::global(None).expect("create quickjs engine"));
    inner.set_plugin_runtime_manager(runtime_manager.clone());
    inner.set_audit_recorder(Arc::new(TracingAuditRecorder));

    let executor = PluginToolExecutor::new(Arc::downgrade(&manager));
    let registry_impl = Arc::new(DefaultToolRegistry::new(
        executor.clone(),
        Arc::new(TracingAuditRecorder),
    ));
    let registry: Arc<dyn ToolRegistry> = registry_impl.clone();
    let dispatcher = Arc::new(
        HostApiDispatcher::new(event_bus.clone())
            .with_tokio_handle(tokio::runtime::Handle::current())
            .with_tools(registry.clone()),
    );
    executor.attach_dispatcher(Arc::downgrade(&dispatcher));
    manager.set_tool_registry(registry);
    manager.set_host_dispatcher(dispatcher.clone());
    manager
        .load_plugin(plugin_dir)
        .expect("load real plugin fixture");

    (executor, manager, dispatcher, runtime_manager)
}

fn real_function_fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("function_echo_plugin")
}

fn install_real_function_fixture(work_dir: &Path) -> PathBuf {
    let src = real_function_fixture_dir();
    let dst = work_dir.join("function-echo-plugin");
    std::fs::create_dir_all(&dst).expect("create function fixture dir");
    for name in ["plugin.json", "main.js"] {
        std::fs::copy(src.join(name), dst.join(name)).expect("copy function fixture file");
    }
    dst
}

fn make_function_manager(plugin_dir: &Path) -> FunctionManagerHarness {
    let event_bus = Arc::new(DefaultEventBus::new());
    let mut manager = Arc::new(PluginManager::new(event_bus.clone()));
    let runtime_manager: SharedPluginRuntimeManager = Arc::new(PluginRuntimeManager::new());

    let inner = Arc::get_mut(&mut manager).expect("plugin manager should be uniquely owned");
    inner.set_plugin_engine(PluginEngine::global(None).expect("create quickjs engine"));
    inner.set_plugin_runtime_manager(runtime_manager.clone());
    inner.set_audit_recorder(Arc::new(TracingAuditRecorder));

    let tool_executor = PluginToolExecutor::new(Arc::downgrade(&manager));
    let tool_registry = Arc::new(DefaultToolRegistry::new(
        tool_executor.clone(),
        Arc::new(TracingAuditRecorder),
    ));
    let dispatcher = Arc::new(
        HostApiDispatcher::new(event_bus.clone())
            .with_tokio_handle(tokio::runtime::Handle::current())
            .with_tools(tool_registry.clone()),
    );
    let function_registry = Arc::new(FunctionRegistry::new());
    let function_invoker = PluginFunctionInvoker::new(Arc::downgrade(&manager));

    tool_executor.attach_dispatcher(Arc::downgrade(&dispatcher));
    function_invoker.attach_dispatcher(Arc::downgrade(&dispatcher));
    manager.set_tool_registry(tool_registry.clone());
    manager.set_function_registry(function_registry.clone());
    manager.set_host_dispatcher(dispatcher.clone());

    let manifest_json =
        std::fs::read_to_string(plugin_dir.join("plugin.json")).expect("read function manifest");
    let manifest = parse_manifest(&manifest_json).expect("parse function manifest");
    function_registry.register_plugin_functions(&manifest.id, plugin_dir, &manifest.functions);
    manager
        .register_catalog_plugin(plugin_dir, manifest)
        .expect("register catalog stub");
    manager
        .load_plugin(plugin_dir)
        .expect("load real function plugin fixture");

    (
        function_invoker,
        function_registry,
        tool_registry,
        manager,
        dispatcher,
        runtime_manager,
    )
}

fn make_manager() -> (PluginManager, SharedPluginRuntimeManager) {
    let bus = Arc::new(DefaultEventBus::new());
    let dispatcher = Arc::new(
        HostApiDispatcher::new(bus.clone()).with_tokio_handle(tokio::runtime::Handle::current()),
    );
    make_manager_with_dispatcher_and_config(
        bus,
        dispatcher,
        PluginEngineConfig {
            call_timeout_ms: 500,
            interrupt_budget: 50_000,
            ..Default::default()
        },
    )
}

fn make_manager_with_dispatcher(
    bus: Arc<DefaultEventBus>,
    dispatcher: Arc<HostApiDispatcher>,
) -> (PluginManager, SharedPluginRuntimeManager) {
    make_manager_with_dispatcher_and_config(
        bus,
        dispatcher,
        PluginEngineConfig {
            call_timeout_ms: 500,
            interrupt_budget: 50_000,
            ..Default::default()
        },
    )
}

fn make_manager_with_dispatcher_and_config(
    bus: Arc<DefaultEventBus>,
    dispatcher: Arc<HostApiDispatcher>,
    engine_config: PluginEngineConfig,
) -> (PluginManager, SharedPluginRuntimeManager) {
    let rm: SharedPluginRuntimeManager = Arc::new(PluginRuntimeManager::new());
    let engine = PluginEngine::global(Some(engine_config)).expect("create quickjs engine");

    let mut manager = PluginManager::new(bus);
    manager.set_plugin_engine(engine);
    manager.set_host_dispatcher(dispatcher);
    manager.set_plugin_runtime_manager(rm.clone());
    manager.set_event_channel_capacity(16);
    (manager, rm)
}

struct MockPrimitive;

#[async_trait]
impl PrimitiveExecutor for MockPrimitive {
    async fn read_file(&self, _path: &str, _plugin_id: &str) -> Result<String, tomcat::AppError> {
        Ok("mock_content".to_string())
    }

    async fn list_dir(
        &self,
        _path: &str,
        _plugin_id: &str,
    ) -> Result<Vec<DirEntry>, tomcat::AppError> {
        Ok(vec![])
    }

    async fn write_file(
        &self,
        path: &str,
        _content: &str,
        _overwrite: bool,
        _plugin_id: &str,
    ) -> Result<WriteFileResult, tomcat::AppError> {
        Ok(WriteFileResult {
            path: path.to_string(),
            written: true,
            bytes_written: 0,
            diff_hint: None,
        })
    }

    async fn edit_file(
        &self,
        path: &str,
        _edits: Vec<EditOperation>,
        _plugin_id: &str,
    ) -> Result<EditFileResult, tomcat::AppError> {
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
        _timeout_ms: Option<u64>,
    ) -> Result<BashResult, tomcat::AppError> {
        Ok(BashResult {
            stdout: "ok".to_string(),
            stderr: String::new(),
            exit_code: 0,
            ..Default::default()
        })
    }

    async fn require_user_confirmation(
        &self,
        _op: PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, tomcat::AppError> {
        Ok(true)
    }
}

struct MockLlm;

#[async_trait]
impl LlmProvider for MockLlm {
    fn provider_name(&self) -> &str {
        "mock"
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, tomcat::AppError> {
        Ok(ChatResponse {
            id: Some("quickjs-e2e".to_string()),
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
        Box<dyn futures_util::Stream<Item = Result<StreamEvent, tomcat::AppError>> + Send + Unpin>,
        tomcat::AppError,
    > {
        Ok(Box::new(stream::iter(vec![Ok(
            StreamEvent::ContentDelta {
                delta: "hi".to_string(),
            },
        )])))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, tomcat::AppError> {
        Ok(0)
    }
}

async fn wait_for_state(handle: &VmActorHandle, expected: VmActorState) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if handle.current_state() == expected {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_counter(counter: &AtomicU32, expected: u32) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if counter.load(Ordering::SeqCst) >= expected {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[test]
fn quickjs_engine_runs_bridge_and_hostcall() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let engine = PluginEngine::global(None)?;
    let mut instance = engine.create_instance("quickjs-smoke")?;
    let call_count = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&call_count);

    instance.register_host_binding(move |request_json: &str| {
        let req: serde_json::Value = serde_json::from_str(request_json).unwrap();
        if req.get("method").and_then(|m| m.as_str()) == Some("log") {
            counter.fetch_add(1, Ordering::SeqCst);
        }
        Ok(serde_json::json!({"ok": true, "data": null}).to_string())
    })?;

    instance.run_script("pi.log('hello from quickjs');")?;
    assert!(
        call_count.load(Ordering::SeqCst) >= 1,
        "pi.log should reach host binding at least once"
    );
    Ok(())
}

#[test]
fn run_script_console() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let engine = PluginEngine::global(None)?;
    let mut instance = engine.create_instance("quickjs-console")?;
    let logs = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&logs);

    instance.register_host_binding(move |request_json: &str| {
        let req: serde_json::Value = serde_json::from_str(request_json).unwrap();
        if req.get("method").and_then(|m| m.as_str()) == Some("log") {
            if let Some(message) = req
                .get("params")
                .and_then(|params| params.get("message"))
                .and_then(|value| value.as_str())
            {
                sink.lock().unwrap().push(message.to_string());
            }
        }
        Ok(serde_json::json!({"ok": true, "data": null}).to_string())
    })?;

    instance.run_script(
        r#"
console.log("hello", { value: 2 });
console.error("boom");
Promise.resolve().then(function () { console.warn("microtask-fired"); });
setTimeout(function () { console.info("timer-fired"); }, 5);
"#,
    )?;

    let logs = logs.lock().unwrap();
    assert!(logs.iter().any(|line| line.contains("[log] hello")));
    assert!(logs.iter().any(|line| line.contains("[error] boom")));
    assert!(logs.iter().any(|line| line.contains("microtask-fired")));
    assert!(logs.iter().any(|line| line.contains("timer-fired")));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shims_and_crypto_work_in_session_vm() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let plugin_dir = create_plugin_dir(
        "shim-session-plugin",
        r#"
pi.on("session_start", function () {
  if (path.join("/tmp", "demo", "..", "ok.txt") !== "/tmp/ok.txt") {
    throw new Error("path.join mismatch");
  }
  if (util.format("%s:%d", "ok", 2) !== "ok:2") {
    throw new Error("util.format mismatch");
  }
  const emitter = new events.EventEmitter();
  let seen = 0;
  emitter.on("ping", function (value) { seen = value; });
  emitter.emit("ping", 9);
  if (seen !== 9) {
    throw new Error("events mismatch");
  }
  const digest = crypto.createHash("sha256").update("abc").digest("hex");
  if (digest !== "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad") {
    throw new Error("crypto mismatch");
  }
  const mac = crypto.createHmac("sha256", "key")
    .update("The quick brown fox jumps over the lazy dog")
    .digest("hex");
  if (mac !== "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8") {
    throw new Error("hmac mismatch");
  }
  const bytes = crypto.randomBytes(8);
  if (!Buffer.isBuffer(bytes) || bytes.length !== 8) {
    throw new Error("randomBytes mismatch");
  }

  const aesKey = Buffer.from("00000000000000000000000000000000", "hex");
  const aesIv = Buffer.from("000000000000000000000000", "hex");
  const aesPlaintext = Buffer.from("00000000000000000000000000000000", "hex");
  const aesSealed = crypto.aesGcmEncrypt(aesKey, aesIv, aesPlaintext);
  if (aesSealed.toString("hex") !== "0388dace60b6a392f328c2b971b2fe78ab6e47d42cec13bdf53a67b21257bddf") {
    throw new Error("aes-gcm mismatch");
  }
  if (crypto.aesGcmDecrypt(aesKey, aesIv, aesSealed).toString("hex") !== aesPlaintext.toString("hex")) {
    throw new Error("aes-gcm decrypt mismatch");
  }

  const edSeed = Buffer.from(
    "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
    "hex"
  );
  const edPair = crypto.ed25519GenerateKeyPair(edSeed);
  const edSignature = crypto.ed25519Sign(edPair.secretKey, Buffer.alloc(0));
  if (!crypto.ed25519Verify(edPair.publicKey, Buffer.alloc(0), edSignature)) {
    throw new Error("ed25519 verify mismatch");
  }
});
__pi_start_event_loop();
"#,
    );

    let (manager, rm) = make_manager();
    register_plugin(&manager, plugin_dir.path(), "shim-session-plugin");

    let handle = manager
        .start_session_vm("s1", "shim-session-plugin")
        .await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    manager.dispatch_session_event(
        "s1",
        "shim-session-plugin",
        "session_start",
        serde_json::json!({}),
        serde_json::json!({}),
    )?;
    tokio::time::sleep(Duration::from_millis(250)).await;

    assert!(
        matches!(
            handle.current_state(),
            VmActorState::Created | VmActorState::Running | VmActorState::Idle
        ),
        "Tier-A shims and crypto should keep the session VM healthy"
    );

    manager.end_session("s1").await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rm.is_empty(), "end_session should clear RuntimeManager");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pi_readfile_llm() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let plugin_dir = create_plugin_dir(
        "readfile-llm-plugin",
        r#"
pi.on("session_start", async function () {
  const text = await pi.readFile("/tmp/demo.txt");
  if (text !== "mock_content") {
    throw new Error("readFile mismatch: " + text);
  }
  const reply = await pi.complete("say hi");
  if (reply !== "hi") {
    throw new Error("llm mismatch: " + reply);
  }
  console.log("readfile-llm-ok");
});
__pi_start_event_loop();
"#,
    );

    let bus = Arc::new(DefaultEventBus::new());
    let dispatcher = Arc::new(
        HostApiDispatcher::new(bus.clone())
            .with_tokio_handle(tokio::runtime::Handle::current())
            .with_primitive(Arc::new(MockPrimitive))
            .with_llm(Arc::new(MockLlm)),
    );
    let (manager, rm) = make_manager_with_dispatcher(bus, dispatcher);
    register_plugin(&manager, plugin_dir.path(), "readfile-llm-plugin");

    let handle = manager
        .start_session_vm("s1", "readfile-llm-plugin")
        .await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    manager.dispatch_session_event(
        "s1",
        "readfile-llm-plugin",
        "session_start",
        serde_json::json!({}),
        serde_json::json!({}),
    )?;
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(
        matches!(
            handle.current_state(),
            VmActorState::Created | VmActorState::Running | VmActorState::Idle
        ),
        "readFile + llm hostcalls should keep the session VM healthy"
    );

    manager.end_session("s1").await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rm.is_empty(), "end_session should clear RuntimeManager");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pure_tool_plugin_executes_via_real_tool_harness() -> Result<(), Box<dyn std::error::Error>>
{
    common::setup_logging();
    let plugin_dir = create_plugin_dir_with_manifest(
        json!({
            "id": "pure-tool-plugin",
            "name": "pure-tool-plugin",
            "version": "0.1.0",
            "description": "pure tool fixture",
            "author": "tests",
            "main": "main.js",
            "requiredPermissions": [],
            "requiredApiVersion": "1.0",
            "tags": [],
            "tools": [{
                "name": "plugin_add",
                "description": "add two numbers",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "a": { "type": "number" },
                        "b": { "type": "number" }
                    },
                    "required": ["a", "b"]
                }
            }],
            "events": [],
            "activation": "lazy"
        }),
        r#"
pi.registerTool({
  name: "plugin_add",
  description: "add two numbers",
  parameters: {
    type: "object",
    properties: {
      a: { type: "number" },
      b: { type: "number" }
    },
    required: ["a", "b"]
  },
  execute: function (_callId, params) {
    return params.a + params.b;
  }
});
"#,
    );

    let (executor, manager, dispatcher, rm) = make_tool_manager(plugin_dir.path());
    let info = manager
        .get_plugin("pure-tool-plugin")
        .expect("pure tool plugin info");
    assert!(
        info.registered_tools
            .iter()
            .any(|tool| tool == "plugin_add"),
        "manifest-declared tool should be visible after loading"
    );
    assert!(
        !manager.has_session_vm("s1", "pure-tool-plugin"),
        "lazy pure-tool plugin should not prestart a session VM"
    );

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        executor.execute(
            &make_tool("plugin_add", "pure-tool-plugin"),
            json!({ "a": 2, "b": 3 }),
            "__test__",
            Some("s1"),
        ),
    )
    .await?
    .expect("execute pure tool");

    assert_eq!(result, json!(5));
    assert!(
        dispatcher.command_completed_count() >= 1,
        "commandCompleted should be emitted for successful tool execution"
    );

    manager.end_session("s1").await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rm.is_empty(), "end_session should clear RuntimeManager");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_vm_preserves_state_across_custom_events() -> Result<(), Box<dyn std::error::Error>>
{
    common::setup_logging();
    let plugin_dir = create_plugin_dir_with_manifest(
        json!({
            "id": "phase-session-plugin",
            "name": "phase-session-plugin",
            "version": "0.1.0",
            "description": "session lifecycle fixture",
            "author": "tests",
            "main": "main.js",
            "requiredPermissions": [],
            "requiredApiVersion": "1.0",
            "tags": [],
            "tools": [],
            "events": ["phase_a", "phase_b"],
            "activation": "session"
        }),
        r#"
let seen = 0;
pi.on("phase_a", function (_data, ctx) {
  seen += 1;
  ctx.ui.notify("phase_a", "info");
});
pi.on("phase_b", function (_data, ctx) {
  if (seen !== 1) {
    throw new Error("state not preserved before phase_b: " + seen);
  }
  seen += 1;
  ctx.ui.notify("phase_b", "info");
});
__pi_start_event_loop();
"#,
    );

    let notify_count = Arc::new(AtomicU32::new(0));
    let bus = Arc::new(DefaultEventBus::new());
    let dispatcher = Arc::new(
        HostApiDispatcher::new(bus.clone())
            .with_tokio_handle(tokio::runtime::Handle::current())
            .with_ui_notify_counter(notify_count.clone()),
    );
    let (manager, rm) = make_manager_with_dispatcher(bus, dispatcher);
    register_plugin(&manager, plugin_dir.path(), "phase-session-plugin");

    let handle = manager
        .start_session_vm("s1", "phase-session-plugin")
        .await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    manager.dispatch_session_event(
        "s1",
        "phase-session-plugin",
        "phase_a",
        json!({}),
        json!({ "sessionId": "s1" }),
    )?;
    manager.dispatch_session_event(
        "s1",
        "phase-session-plugin",
        "phase_b",
        json!({}),
        json!({ "sessionId": "s1" }),
    )?;

    assert!(
        wait_for_counter(notify_count.as_ref(), 2).await,
        "both lifecycle events should be processed by the same long-lived VM"
    );
    assert!(
        matches!(
            handle.current_state(),
            VmActorState::Running | VmActorState::Idle
        ),
        "session VM should remain healthy after multiple custom events"
    );

    manager.end_session("s1").await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rm.is_empty(), "end_session should clear RuntimeManager");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_toolbox_plugin_discovers_dynamic_tools_and_executes(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let plugin_dir = create_plugin_dir(
        "legacy-toolbox-plugin",
        r#"
pi.registerTool({
  name: "plugin_upper",
  description: "uppercase text",
  parameters: {
    type: "object",
    properties: {
      text: { type: "string" }
    },
    required: ["text"]
  },
  execute: function (_callId, params) {
    return params.text.toUpperCase();
  }
});

pi.registerTool({
  name: "plugin_len",
  description: "text length",
  parameters: {
    type: "object",
    properties: {
      text: { type: "string" }
    },
    required: ["text"]
  },
  execute: function (_callId, params) {
    return params.text.length;
  }
});
"#,
    );

    let (executor, manager, dispatcher, rm) = make_tool_manager(plugin_dir.path());
    let info = manager
        .get_plugin("legacy-toolbox-plugin")
        .expect("legacy toolbox plugin info");
    let mut dynamic_tools = dispatcher.registered_plugin_tools("legacy-toolbox-plugin");
    dynamic_tools.sort();

    assert!(
        info.manifest.tools.is_empty(),
        "legacy toolbox fixture should not declare static tools[]"
    );
    assert_eq!(
        dynamic_tools,
        vec!["plugin_len".to_string(), "plugin_upper".to_string()],
        "runtime registration should discover both dynamic tools"
    );
    let mut registered_tools = info.registered_tools.clone();
    registered_tools.sort();
    assert_eq!(
        registered_tools, dynamic_tools,
        "get_plugin should surface the dynamically discovered tool set"
    );

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        executor.execute(
            &make_tool("plugin_upper", "legacy-toolbox-plugin"),
            json!({ "text": "hi" }),
            "__test__",
            Some("s1"),
        ),
    )
    .await?
    .expect("execute legacy toolbox tool");

    assert_eq!(result, json!("HI"));

    manager.end_session("s1").await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rm.is_empty(), "end_session should clear RuntimeManager");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runaway_plugin_interrupted() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let runaway_dir = create_plugin_dir(
        "runaway-plugin",
        r#"
pi.on('loop', function () {
  while (true) {}
});
__pi_start_event_loop();
"#,
    );

    let (manager, rm) = make_manager();
    register_plugin(&manager, runaway_dir.path(), "runaway-plugin");

    let handle = manager.start_session_vm("s1", "runaway-plugin").await?;
    tokio::time::sleep(Duration::from_millis(200)).await;

    manager.dispatch_session_event(
        "s1",
        "runaway-plugin",
        "loop",
        serde_json::json!({}),
        serde_json::json!({}),
    )?;

    assert!(
        wait_for_state(&handle, VmActorState::Error).await,
        "runaway plugin should enter Error after interrupt budget / timeout"
    );

    let rebuilt = manager.start_session_vm("s1", "runaway-plugin").await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        matches!(
            rebuilt.current_state(),
            VmActorState::Created | VmActorState::Running | VmActorState::Idle
        ),
        "failed runtime should be cold-rebuilt on next start_session_vm"
    );
    assert!(
        !Arc::ptr_eq(&handle.state, &rebuilt.state),
        "rebuild should allocate a fresh VmActor handle"
    );

    manager.end_session("s1").await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rm.is_empty(), "end_session should clear RuntimeManager");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runaway_plugin_timeout_interrupts_when_budget_disabled(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let runaway_dir = create_plugin_dir(
        "runaway-timeout-plugin",
        r#"
pi.on('loop', function () {
  while (true) {}
});
__pi_start_event_loop();
"#,
    );

    let bus = Arc::new(DefaultEventBus::new());
    let dispatcher = Arc::new(
        HostApiDispatcher::new(bus.clone()).with_tokio_handle(tokio::runtime::Handle::current()),
    );
    let (manager, rm) = make_manager_with_dispatcher_and_config(
        bus,
        dispatcher,
        PluginEngineConfig {
            call_timeout_ms: 50,
            interrupt_budget: 0,
            ..Default::default()
        },
    );
    register_plugin(&manager, runaway_dir.path(), "runaway-timeout-plugin");

    let handle = manager
        .start_session_vm("s1", "runaway-timeout-plugin")
        .await?;
    tokio::time::sleep(Duration::from_millis(200)).await;

    manager.dispatch_session_event(
        "s1",
        "runaway-timeout-plugin",
        "loop",
        serde_json::json!({}),
        serde_json::json!({}),
    )?;

    assert!(
        wait_for_state(&handle, VmActorState::Error).await,
        "runaway plugin should enter Error when only call_timeout_ms is left enabled"
    );

    let rebuilt = manager
        .start_session_vm("s1", "runaway-timeout-plugin")
        .await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        matches!(
            rebuilt.current_state(),
            VmActorState::Created | VmActorState::Running | VmActorState::Idle
        ),
        "timed-out runtime should be cold-rebuilt on next start_session_vm"
    );
    assert!(
        !Arc::ptr_eq(&handle.state, &rebuilt.state),
        "timeout rebuild should allocate a fresh VmActor handle"
    );

    manager.end_session("s1").await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rm.is_empty(), "end_session should clear RuntimeManager");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn panicking_plugin_isolated() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let crash_dir = create_plugin_dir(
        "crashy-plugin",
        r#"
pi.on('boom', function () {
  throw new Error('boom');
});
__pi_start_event_loop();
"#,
    );
    let healthy_dir = create_plugin_dir(
        "healthy-plugin",
        r#"
pi.on('ping', function () {
  pi.log('pong');
});
__pi_start_event_loop();
"#,
    );

    let (manager, rm) = make_manager();
    register_plugin(&manager, crash_dir.path(), "crashy-plugin");
    register_plugin(&manager, healthy_dir.path(), "healthy-plugin");

    let crashy = manager.start_session_vm("s1", "crashy-plugin").await?;
    let healthy = manager.start_session_vm("s1", "healthy-plugin").await?;
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(rm.len(), 2, "both plugin runtimes should be registered");

    manager.dispatch_session_event(
        "s1",
        "crashy-plugin",
        "boom",
        serde_json::json!({}),
        serde_json::json!({}),
    )?;

    assert!(
        wait_for_state(&crashy, VmActorState::Error).await,
        "throwing plugin should be isolated into Error state"
    );

    manager.dispatch_session_event(
        "s1",
        "healthy-plugin",
        "ping",
        serde_json::json!({}),
        serde_json::json!({}),
    )?;
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        healthy.current_state(),
        VmActorState::Running,
        "neighbor runtime should keep running after another plugin fails"
    );

    manager.end_session("s1").await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rm.is_empty(), "end_session should clear RuntimeManager");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_real_function_plugin_roundtrip() {
    common::setup_logging();
    let work_dir = tempfile::tempdir().expect("work dir");
    let plugin_dir = install_real_function_fixture(work_dir.path());
    let (invoker, function_registry, _tool_registry, manager, _dispatcher, _runtime_manager) =
        make_function_manager(&plugin_dir);

    let target = function_registry
        .functions_for_point("test.echo")
        .into_iter()
        .next()
        .expect("echo target");
    let result = invoker
        .execute(&target, json!({ "text": "hello" }), Some("s1"))
        .await
        .expect("roundtrip function call");

    assert_eq!(result["plugin"], "function-echo-plugin");
    assert_eq!(result["point"], "test.echo");
    assert_eq!(result["echoed"], "hello");

    manager.end_session("s1").await.expect("end session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_real_function_plugin_supports_multi_point() {
    common::setup_logging();
    let work_dir = tempfile::tempdir().expect("work dir");
    let plugin_dir = install_real_function_fixture(work_dir.path());
    let (invoker, function_registry, _tool_registry, manager, _dispatcher, _runtime_manager) =
        make_function_manager(&plugin_dir);

    let echo_target = function_registry
        .functions_for_point("test.echo")
        .into_iter()
        .next()
        .expect("echo target");
    let counter_target = function_registry
        .functions_for_point("test.counter")
        .into_iter()
        .next()
        .expect("counter target");

    let echo_result = invoker
        .execute(&echo_target, json!({ "text": "multi" }), Some("s1"))
        .await
        .expect("echo function");
    let counter_result = invoker
        .execute(&counter_target, json!({ "label": "first" }), Some("s1"))
        .await
        .expect("counter function");

    assert_eq!(echo_result["point"], "test.echo");
    assert_eq!(counter_result["point"], "test.counter");
    assert_eq!(counter_result["count"], 1);

    manager.end_session("s1").await.expect("end session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_real_function_plugin_reuses_state_within_session() {
    common::setup_logging();
    let work_dir = tempfile::tempdir().expect("work dir");
    let plugin_dir = install_real_function_fixture(work_dir.path());
    let (invoker, function_registry, _tool_registry, manager, _dispatcher, _runtime_manager) =
        make_function_manager(&plugin_dir);

    let target = function_registry
        .functions_for_point("test.counter")
        .into_iter()
        .next()
        .expect("counter target");
    let first = invoker
        .execute(&target, json!({ "label": "first" }), Some("s1"))
        .await
        .expect("first counter call");
    let second = invoker
        .execute(&target, json!({ "label": "second" }), Some("s1"))
        .await
        .expect("second counter call");

    assert_eq!(first["count"], 1);
    assert_eq!(second["count"], 2);

    manager.end_session("s1").await.expect("end session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_real_function_plugin_isolates_state_across_sessions() {
    common::setup_logging();
    let work_dir = tempfile::tempdir().expect("work dir");
    let plugin_dir = install_real_function_fixture(work_dir.path());
    let (invoker, function_registry, _tool_registry, manager, _dispatcher, _runtime_manager) =
        make_function_manager(&plugin_dir);

    let target = function_registry
        .functions_for_point("test.counter")
        .into_iter()
        .next()
        .expect("counter target");
    let first_session = invoker
        .execute(&target, json!({ "label": "s1" }), Some("s1"))
        .await
        .expect("session one counter call");
    let second_session = invoker
        .execute(&target, json!({ "label": "s2" }), Some("s2"))
        .await
        .expect("session two counter call");

    assert_eq!(first_session["count"], 1);
    assert_eq!(second_session["count"], 1);

    manager.end_session("s1").await.expect("end session s1");
    manager.end_session("s2").await.expect("end session s2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_real_function_plugin_hidden_from_tool_registry() {
    common::setup_logging();
    let work_dir = tempfile::tempdir().expect("work dir");
    let plugin_dir = install_real_function_fixture(work_dir.path());
    let (_invoker, function_registry, tool_registry, manager, _dispatcher, _runtime_manager) =
        make_function_manager(&plugin_dir);

    let tools = tool_registry.list_tools(None).await.expect("list tools");
    assert!(
        tools.is_empty(),
        "host-facing function fixture should not register LLM tools"
    );
    assert_eq!(function_registry.functions_for_point("test.echo").len(), 1);
    assert_eq!(
        function_registry.functions_for_point("test.counter").len(),
        1
    );

    manager.end_session("s1").await.expect("end session");
}
