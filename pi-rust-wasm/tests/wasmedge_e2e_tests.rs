//! Wasm E2E 集成测试：真实 WasmEngine + run_script + host_call 链路。
//! 须在安装 WasmEdge 后以 `cargo test --test wasmedge_e2e_tests` 运行（默认构建即包含 WasmEdge）；
//! 环境缺失时用例失败、不允许跳过，见 INTEGRATION_TEST_SPEC 5.4 与 docs/02-wasm-runtime-and-plugin.md。

mod common;

use pi_awsm::{HostResponse, WasmEngine, WasmEngineConfig};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

const WASMEDGE_INSTALL_URL: &str = "https://wasmedge.org/docs/start/install";

/// 真实 Wasm 运行时 E2E：创建引擎与实例、注册 host_binding、run_script，断言宿主可调或脚本执行成功。
/// 环境缺失不允许跳过：未安装 WasmEdge 或 quickjs 路径不可用时用例失败（panic），见 INTEGRATION_TEST_SPEC 5.4。
#[test]
fn test_wasmedge_e2e_engine_instance_run_script() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_wasmedge_e2e_engine_instance_run_script").entered();

    let quickjs_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/wasm/wasmedge_quickjs.wasm");
    if !quickjs_path.exists() {
        panic!(
            "集成测试要求 wasmedge_quickjs.wasm 路径存在。请设置 WASMEDGE_QUICKJS_PATH 或确保 {:?} 存在，或运行 ./scripts/install-wasmedge.sh。见 INTEGRATION_TEST_SPEC 5.4 与 docs/02-wasm-runtime-and-plugin.md",
            quickjs_path
        );
    }

    tracing::info!(
        "Arrange: 配置 WasmEngineConfig quickjs_path = {:?}",
        quickjs_path
    );
    let config = WasmEngineConfig {
        quickjs_path: Some(quickjs_path.to_string_lossy().into_owned()),
        ..WasmEngineConfig::default()
    };

    let engine = match WasmEngine::global(Some(config)) {
        Ok(e) => e,
        Err(e) => {
            if e.to_string().contains("stub") || e.to_string().contains("WasmEdge") {
                panic!(
                    "集成测试要求已安装 WasmEdge 并以 cargo test --test wasmedge_e2e_tests 运行，不得跳过。当前: {}。安装见 {} 或运行 ./scripts/install-wasmedge.sh，规范见 INTEGRATION_TEST_SPEC 5.4 与 docs/02-wasm-runtime-and-plugin.md",
                    e,
                    WASMEDGE_INSTALL_URL
                );
            }
            return Err(e.into());
        }
    };

    tracing::info!("Act: create_instance、register_host_binding、run_script 空脚本");
    let mut instance = engine.create_instance("e2e-plugin")?;
    let call_count = std::sync::Arc::new(AtomicU32::new(0));
    let count = std::sync::Arc::clone(&call_count);
    instance.register_host_binding(move |request_json: &str| {
        count.fetch_add(1, Ordering::SeqCst);
        tracing::debug!("host_call 收到: {}", request_json);
        Ok(serde_json::json!({"ok":true,"result":null}).to_string())
    })?;

    let run_result = instance.run_script("");
    tracing::info!("Assert: run_script 返回 Ok，真实 Wasm 运行时无崩溃");
    run_result?;

    tracing::info!("Assert: E2E 通过；具备 WasmEdge 时 host_call 可由 quickjs 触发");
    Ok(())
}

/// 真实 .js 脚本 Hello World：用 run_script_file 执行 tests/fixtures/wasmedge_quickjs/hello.js，断言返回 Ok。
#[test]
fn test_wasmedge_e2e_hello_world_script_file() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let quickjs_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/wasm/wasmedge_quickjs.wasm");
    if !quickjs_path.exists() {
        panic!(
            "集成测试要求 wasmedge_quickjs.wasm 存在。见 test_wasmedge_e2e_engine_instance_run_script",
        );
    }
    let config = WasmEngineConfig {
        quickjs_path: Some(quickjs_path.to_string_lossy().into_owned()),
        ..WasmEngineConfig::default()
    };
    let engine = WasmEngine::global(Some(config)).map_err(|e| e.to_string())?;
    let hello_js =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasmedge_quickjs/hello.js");
    assert!(
        hello_js.exists(),
        "fixture hello.js 必须存在: {:?}",
        hello_js
    );
    let mut instance = engine.create_instance("hello-e2e")?;
    instance.run_script_file(&hello_js)?;
    Ok(())
}

/// 真实 .js 脚本 Hello World：用 run_script 内联执行 print('Hello World');，断言返回 Ok。
#[test]
fn test_wasmedge_e2e_hello_world_inline() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let quickjs_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/wasm/wasmedge_quickjs.wasm");
    if !quickjs_path.exists() {
        panic!(
            "集成测试要求 wasmedge_quickjs.wasm 存在。见 test_wasmedge_e2e_engine_instance_run_script",
        );
    }
    let config = WasmEngineConfig {
        quickjs_path: Some(quickjs_path.to_string_lossy().into_owned()),
        ..WasmEngineConfig::default()
    };
    let engine = WasmEngine::global(Some(config)).map_err(|e| e.to_string())?;
    let mut instance = engine.create_instance("hello-inline")?;
    instance.run_script("print('Hello World');")?;
    Ok(())
}

/// 桥接层集成测试：通过 pi_bridge.js 预加载构建的 pi 全局对象调用 4 原语及事件/日志/会话 API。
/// 断言：pi.readFile/writeFile/editFile/exec 各触发 1 次 hostCall（共 ≥ 4），pi.on/pi.log/pi.session 不崩溃。
#[test]
fn test_wasmedge_e2e_bridge_layer() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let quickjs_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/wasm/wasmedge_quickjs.wasm");
    if !quickjs_path.exists() {
        panic!("集成测试要求 wasmedge_quickjs.wasm 存在。");
    }
    let config = WasmEngineConfig {
        quickjs_path: Some(quickjs_path.to_string_lossy().into_owned()),
        ..WasmEngineConfig::default()
    };
    let engine = WasmEngine::global(Some(config)).map_err(|e| e.to_string())?;
    let bridge_js = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasmedge_quickjs/bridge_test.js");
    assert!(
        bridge_js.exists(),
        "fixture bridge_test.js 必须存在: {:?}",
        bridge_js
    );
    let mut instance = engine.create_instance("bridge-e2e")?;
    let call_count = std::sync::Arc::new(AtomicU32::new(0));
    let count = std::sync::Arc::clone(&call_count);
    instance.register_host_binding(move |request_json: &str| {
        let req: serde_json::Value =
            serde_json::from_str(request_json).unwrap_or(serde_json::Value::Null);
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        if ["readFile", "writeFile", "editFile", "executeBash"].contains(&method) {
            count.fetch_add(1, Ordering::SeqCst);
        }
        Ok(serde_json::to_string(&HostResponse::ok(serde_json::json!({ "content": "" }))).unwrap())
    })?;
    instance.run_script_file(&bridge_js)?;
    let n = call_count.load(Ordering::SeqCst);
    assert!(
        n >= 4,
        "桥接层测试须通过 pi.readFile/writeFile/editFile/exec 触发 ≥ 4 次 host 调用，实际 {} 次",
        n
    );
    Ok(())
}

/// 事件分发集成测试：宿主通过 dispatch_event 向插件脚本分发事件，
/// 验证 pi.on 注册的 handler 被触发、ctx 代理对象的动态方法（isIdle/hasPendingMessages/
/// getSystemPrompt/getContextUsage/compact/ui.notify）均触发 hostCall。
/// 断言：events.subscribe 1 + context.* 至少 6 + agent.log 1 = 总计 ≥ 8 次 hostCall。
#[test]
fn test_wasmedge_e2e_event_dispatch() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let quickjs_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/wasm/wasmedge_quickjs.wasm");
    if !quickjs_path.exists() {
        panic!("集成测试要求 wasmedge_quickjs.wasm 存在。");
    }
    let config = WasmEngineConfig {
        quickjs_path: Some(quickjs_path.to_string_lossy().into_owned()),
        ..WasmEngineConfig::default()
    };
    let engine = WasmEngine::global(Some(config)).map_err(|e| e.to_string())?;
    let plugin_js = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasmedge_quickjs/event_dispatch_test.js");
    assert!(
        plugin_js.exists(),
        "fixture event_dispatch_test.js 必须存在: {:?}",
        plugin_js
    );
    let mut instance = engine.create_instance("event-dispatch-e2e")?;
    let call_count = std::sync::Arc::new(AtomicU32::new(0));
    let count = std::sync::Arc::clone(&call_count);
    instance.register_host_binding(move |request_json: &str| {
        count.fetch_add(1, Ordering::SeqCst);
        let req: serde_json::Value =
            serde_json::from_str(request_json).unwrap_or(serde_json::Value::Null);
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let resp = match method {
            "isIdle" => serde_json::json!({"ok":true,"data":{"idle":true}}),
            "hasPendingMessages" => serde_json::json!({"ok":true,"data":{"pending":false}}),
            "getSystemPrompt" => serde_json::json!({"ok":true,"data":{"prompt":""}}),
            "getContextUsage" => {
                serde_json::json!({"ok":true,"data":{"tokens":null,"contextWindow":0,"percent":null}})
            }
            _ => serde_json::json!({"ok":true,"data":null}),
        };
        Ok(serde_json::to_string(&resp).unwrap())
    })?;
    instance.dispatch_event(
        &plugin_js,
        "test_event",
        &serde_json::json!({ "hello": "world" }),
        &serde_json::json!({ "cwd": "/tmp", "hasUI": false, "model": null }),
    )?;
    let n = call_count.load(Ordering::SeqCst);
    assert!(
        n >= 8,
        "事件分发测试须触发 ≥ 8 次 hostCall（subscribe+isIdle+hasPending+getSystemPrompt+getContextUsage+compact+uiNotify+log），实际 {} 次",
        n
    );
    Ok(())
}

/// 4 原语 .js 测试：执行 primitives_test.js，须断言宿主侧 4 次 host 调用（readFile/writeFile/editFile/executeBash 各 1 次）。
/// 符合 INTEGRATION_TEST_SPEC 5.4：断言 host_call 被调用、返回符合预期；不得降低断言或改为可选校验（Constitution 第 24 条）。
#[test]
fn test_wasmedge_e2e_primitives_script_file() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let quickjs_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/wasm/wasmedge_quickjs.wasm");
    if !quickjs_path.exists() {
        panic!(
            "集成测试要求 wasmedge_quickjs.wasm 存在。见 test_wasmedge_e2e_engine_instance_run_script",
        );
    }
    let config = WasmEngineConfig {
        quickjs_path: Some(quickjs_path.to_string_lossy().into_owned()),
        ..WasmEngineConfig::default()
    };
    let engine = WasmEngine::global(Some(config)).map_err(|e| e.to_string())?;
    let primitives_js = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasmedge_quickjs/primitives_test.js");
    assert!(
        primitives_js.exists(),
        "fixture primitives_test.js 必须存在: {:?}",
        primitives_js
    );
    let mut instance = engine.create_instance("primitives-e2e")?;
    let call_count = std::sync::Arc::new(AtomicU32::new(0));
    let count = std::sync::Arc::clone(&call_count);
    instance.register_host_binding(move |request_json: &str| {
        let req: serde_json::Value =
            serde_json::from_str(request_json).unwrap_or(serde_json::Value::Null);
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        if ["readFile", "writeFile", "editFile", "executeBash"].contains(&method) {
            count.fetch_add(1, Ordering::SeqCst);
        }
        Ok(serde_json::to_string(&HostResponse::ok(serde_json::json!({ "content": "" }))).unwrap())
    })?;
    instance.run_script_file(&primitives_js)?;
    let n = call_count.load(Ordering::SeqCst);
    assert!(
        n >= 4,
        "4 原语测试须触发 4 次 host 调用（readFile/writeFile/editFile/executeBash），实际 {} 次；wasmedge_quickjs 须向 JS 暴露 __pi_host_call（见 INTEGRATION_TEST_SPEC 5.4、host-call-protocol.md）",
        n
    );
    Ok(())
}
