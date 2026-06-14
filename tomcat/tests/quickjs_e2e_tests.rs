mod common;

use async_trait::async_trait;
use futures_util::stream;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tomcat::{
    parse_manifest, BashResult, ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice,
    DefaultEventBus, DirEntry, EditFileResult, EditOperation, HostApiDispatcher, LlmProvider,
    PluginInstance, PluginManager, PluginStatus, PrimitiveExecutor, PrimitiveOperation,
    RuntimeManager, SharedRuntimeManager, StreamEvent, VmActorHandle, VmActorState, WasmEngine,
    WasmEngineConfig, WriteFileResult,
};

fn create_plugin_dir(id: &str, script: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("create temp plugin dir");
    let manifest = serde_json::json!({
        "id": id,
        "name": id,
        "version": "0.1.0",
        "description": "quickjs test plugin",
        "author": "tests",
        "main": "main.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": []
    });
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
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    manager
        .register_plugin(PluginInstance {
            id: plugin_id.to_string(),
            manifest,
            wasm_instance: None,
            status: PluginStatus::Loaded,
            registered_tools: vec![],
            registered_commands: vec![],
            event_listener_ids: vec![],
            config: serde_json::json!({}),
            created_at: now,
            loaded_at: now,
            plugin_root: plugin_dir.to_path_buf(),
        })
        .unwrap();
}

fn make_manager() -> (PluginManager, SharedRuntimeManager) {
    let bus = Arc::new(DefaultEventBus::new());
    let dispatcher = Arc::new(
        HostApiDispatcher::new(bus.clone()).with_tokio_handle(tokio::runtime::Handle::current()),
    );
    make_manager_with_dispatcher(bus, dispatcher)
}

fn make_manager_with_dispatcher(
    bus: Arc<DefaultEventBus>,
    dispatcher: Arc<HostApiDispatcher>,
) -> (PluginManager, SharedRuntimeManager) {
    let rm: SharedRuntimeManager = Arc::new(RuntimeManager::new());
    let engine = WasmEngine::global(Some(WasmEngineConfig {
        call_timeout_ms: 500,
        interrupt_budget: 50_000,
        ..Default::default()
    }))
    .expect("create quickjs engine");

    let mut manager = PluginManager::new(bus);
    manager.set_wasm_engine(engine);
    manager.set_host_dispatcher(dispatcher);
    manager.set_runtime_manager(rm.clone());
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

#[test]
fn quickjs_engine_runs_bridge_and_hostcall() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let engine = WasmEngine::global(None)?;
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
    let engine = WasmEngine::global(None)?;
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
