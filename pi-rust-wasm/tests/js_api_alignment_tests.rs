//! 集成测试：JS API 与 pi-mono 对齐（TASK-13 / T1-P0-008 8.7.5 + 8.7.6）。
//!
//! 验证 pi_bridge.js 的异步 API（exec、createChatCompletion）与
//! pi-mono ExtensionAPI 兼容：返回 Promise、结果格式对齐，off/emit/once 行为正确。
//!
//! 须在安装 WasmEdge 后运行（同 wasmedge_e2e_tests）；
//! 宿主侧使用 HostApiDispatcher + PrimitiveExecutor + Tokio 运行时，
//! 模拟真实插件执行环境。

mod common;

use pi_wasm::{
    AllowAllConfirmation, DefaultEventBus, DefaultPrimitiveExecutor, HostApiDispatcher,
    PrimitiveConfig, TracingAuditRecorder, WasmEngine, WasmEngineConfig,
};
use std::path::Path;
use std::sync::Arc;

const WASMEDGE_INSTALL_URL: &str = "https://wasmedge.org/docs/start/install";

/// 构建带完整 Tokio 运行时 + PrimitiveExecutor 的 Dispatcher，用于 async API 测试。
fn build_async_dispatcher() -> Arc<HostApiDispatcher> {
    let bus = Arc::new(DefaultEventBus::new());
    let mut cfg = PrimitiveConfig::default();
    // Whitelist /tmp for exec/file operations in tests.
    cfg.path_whitelist.push("/tmp".to_string());
    cfg.auto_confirm = true;
    let executor = DefaultPrimitiveExecutor::new(
        cfg,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        std::path::PathBuf::from("/tmp"),
    );
    Arc::new(
        HostApiDispatcher::new(bus)
            .with_primitive(Arc::new(executor))
            .with_audit(Arc::new(TracingAuditRecorder)),
    )
}

/// 检查 wasmedge_quickjs.wasm 是否存在；不存在则 panic（不允许跳过）。
fn require_quickjs_path() -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/wasm/wasmedge_quickjs.wasm");
    if !p.exists() {
        panic!(
            "集成测试要求 wasmedge_quickjs.wasm 存在于 {:?}。\
             请安装 WasmEdge（{WASMEDGE_INSTALL_URL}）或运行 ./scripts/install-wasmedge.sh。",
            p
        );
    }
    p.to_string_lossy().into_owned()
}

/// [JS API 异步对齐] js_api_async_test.js 的全量 Assert 通过
///
/// 验证：
/// - `pi.exec("echo hello")` 返回 Promise，resolve 为 `{stdout, stderr, exitCode}` 格式
/// - `pi.createChatCompletion(...)` 返回 Promise（无 LLM 时 reject 亦可接受）
/// - `pi.once` 只触发一次
/// - `pi.off(event, handler)` 按 handler 引用注销有效
/// - `pi.readFile/writeFile/editFile` 返回 Promise
/// - `pi.getModel()` 同步返回（不是 Promise）
/// - `pi.setModel(...)` 返回 Promise
/// - `pi.unregisterTool(...)` 无异常
///
/// 意义：覆盖 TASK-13 8.7.1–8.7.8 所有 P0/P1 改动；确保 pi-mono 风格插件可用
#[test]
fn test_js_api_async_alignment_full() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_js_api_async_alignment_full").entered();

    let quickjs_path = require_quickjs_path();
    let config = WasmEngineConfig {
        quickjs_path: Some(quickjs_path),
        ..WasmEngineConfig::default()
    };

    let engine = match WasmEngine::global(Some(config)) {
        Ok(e) => e,
        Err(e) => {
            if e.to_string().contains("stub") || e.to_string().contains("WasmEdge") {
                panic!(
                    "集成测试要求已安装 WasmEdge。当前错误: {}。安装见 {}",
                    e, WASMEDGE_INSTALL_URL
                );
            }
            return Err(e.into());
        }
    };

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasmedge_quickjs/js_api_async_test.js");
    assert!(
        fixture.exists(),
        "fixture js_api_async_test.js 必须存在: {fixture:?}"
    );

    let rt = tokio::runtime::Runtime::new()?;
    let dispatcher = rt.block_on(async { build_async_dispatcher() });

    let mut instance = engine.create_instance("js-api-alignment-e2e")?;

    // Use the shared Tokio handle so async hostcalls (exec with callId) work correctly.
    let d = Arc::clone(&dispatcher);
    instance.register_host_binding(move |request_json: &str| {
        pi_wasm::invoke_host_func_with(Some(d.as_ref()), "js-api-alignment-e2e", request_json)
            .map(|r| serde_json::to_string(&r).unwrap_or_default())
    })?;

    tracing::info!("Act: run js_api_async_test.js (pi_bridge.js auto-injected)");
    // run_script_file auto-prepends pi_bridge.js; the JS test asserts internally
    // and throws on failure, which propagates as an AppError.
    instance.run_script_file(&fixture)?;

    tracing::info!("Assert: js_api_async_test.js 所有断言通过");
    Ok(())
}

/// [exec 异步 Promise - 独立路径] exec('echo hello') 的 submit→poll 链路验证
///
/// 验证：exec 触发带 callId 的 hostcall（executeBash），poll 最终返回 stdout 含 "hello"
/// 意义：8.7.2 exec 异步化核心路径隔离验证
#[test]
fn test_js_api_exec_async_submit_poll() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_js_api_exec_async_submit_poll").entered();

    let quickjs_path = require_quickjs_path();
    let config = WasmEngineConfig {
        quickjs_path: Some(quickjs_path),
        ..WasmEngineConfig::default()
    };
    let engine = WasmEngine::global(Some(config)).map_err(|e| e.to_string())?;

    // Minimal JS: await pi.exec, print the stdout, throw on mismatch.
    let script_content = r#"
async function main() {
    var r = await pi.exec('echo hello_from_wasm');
    if (!r || typeof r.stdout !== 'string') throw new Error('exec result missing stdout');
    if (r.stdout.indexOf('hello_from_wasm') === -1) throw new Error('stdout mismatch: ' + r.stdout);
    if (r.exitCode !== 0) throw new Error('exitCode should be 0, got ' + r.exitCode);
    print('exec_async_test: PASSED stdout=' + r.stdout.trim());
}
main().catch(function(e) { throw e; });
"#;

    let rt = tokio::runtime::Runtime::new()?;
    let dispatcher = rt.block_on(async { build_async_dispatcher() });

    let mut instance = engine.create_instance("exec-async-test")?;
    let d = Arc::clone(&dispatcher);
    instance.register_host_binding(move |request_json: &str| {
        pi_wasm::invoke_host_func_with(Some(d.as_ref()), "exec-async-test", request_json)
            .map(|r| serde_json::to_string(&r).unwrap_or_default())
    })?;

    // run_script auto-prepends pi_bridge.js via build_combined_script logic... but
    // run_script (inline) does NOT get pi_bridge.js prepended automatically unless
    // it goes through run_script_file. Use a temp file approach via run_script_file.
    let tmp_dir = tempfile::tempdir()?;
    let script_path = tmp_dir.path().join("exec_async_inline.js");
    std::fs::write(&script_path, script_content)?;
    instance.run_script_file(&script_path)?;

    tracing::info!("Assert: exec 异步 submit→poll 链路验证通过");
    Ok(())
}
