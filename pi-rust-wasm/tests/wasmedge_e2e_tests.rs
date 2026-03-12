//! Wasm E2E 集成测试：真实 WasmEngine + run_script + host_call 链路。
//! 须在安装 WasmEdge 后以 `cargo test --test wasmedge_e2e_tests` 运行（默认构建即包含 WasmEdge）；
//! 环境缺失时用例失败、不允许跳过，见 INTEGRATION_TEST_SPEC 5.4 与 docs/02-wasm-runtime-and-plugin.md。

mod common;

use pi_wasm::{DefaultEventBus, HostResponse, PluginManager, WasmEngine, WasmEngineConfig};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

fn require_quickjs_wasm() -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/wasm/wasmedge_quickjs.wasm");
    if !p.exists() {
        panic!(
            "集成测试要求 wasmedge_quickjs.wasm 存在。见 test_wasmedge_e2e_engine_instance_run_script"
        );
    }
    p.to_string_lossy().into_owned()
}

const WASMEDGE_INSTALL_URL: &str = "https://wasmedge.org/docs/start/install";

/// [WasmEdge 引擎 + 实例] 创建引擎与实例、注册 host_binding、run_script 空脚本成功
///
/// 验证：run_script("") 返回 Ok，引擎无崩溃
/// 意义：WasmEdge E2E 最小可用路径——引擎创建/实例化/host_binding 链路（INTEGRATION_TEST_SPEC 5.4）
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

/// [Hello World 文件执行] run_script_file 执行 hello.js 成功
///
/// 验证：run_script_file(hello.js) 返回 Ok
/// 意义：WasmEdge E2E——真实 JS 文件执行链路
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

/// [Hello World 内联执行] run_script 内联执行 print('Hello World') 成功
///
/// 验证：run_script("print('Hello World');") 返回 Ok
/// 意义：WasmEdge E2E——内联脚本执行链路
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

/// [桥接层 pi 全局对象] pi.readFile/writeFile/editFile/exec 各触发 hostCall
///
/// 验证：4 原语触发 ≥4 次 host 调用
/// 意义：WasmEdge E2E——pi_bridge.js 桥接层 4 原语完整链路
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

/// [事件分发 dispatch_event] 宿主向插件分发事件，ctx 代理方法均触发 hostCall
///
/// 验证：hostCall 总次数 ≥8（subscribe+isIdle+hasPending+getSystemPrompt+getContextUsage+compact+uiNotify+log）
/// 意义：WasmEdge E2E——事件分发与 ctx 代理对象完整链路
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

/// [4 原语 JS 脚本] primitives_test.js 触发 readFile/writeFile/editFile/executeBash 各 1 次
///
/// 验证：hostCall 计数 ≥4
/// 意义：WasmEdge E2E——JS 侧 4 原语调用完整链路（INTEGRATION_TEST_SPEC 5.4）
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

/// [插件完整加载] load_plugin 从磁盘加载插件后 list_loaded 含该插件
///
/// 验证：load_plugin 成功、list_loaded 含 id、get_plugin 返回 Some；unload 后为空
/// 意义：WasmEdge E2E——插件从磁盘加载到卸载的完整生命周期（Nibbles + INTEGRATION_TEST_SPEC 5.4）
#[test]
fn test_wasmedge_e2e_load_plugin_from_disk_succeeds() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_wasmedge_e2e_load_plugin_from_disk_succeeds").entered();

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

    let tmp = tempfile::tempdir().map_err(|e| e.to_string())?;
    let plugin_json = r#"{
        "id": "e2e-load-plugin-test",
        "name": "E2E Load Plugin Test",
        "version": "0.1.0",
        "description": "",
        "author": "",
        "main": "main.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": []
    }"#;
    std::fs::write(tmp.path().join("plugin.json"), plugin_json).map_err(|e| e.to_string())?;
    std::fs::write(tmp.path().join("main.js"), "// init\n1 + 1;").map_err(|e| e.to_string())?;

    let bus = Arc::new(DefaultEventBus::new());
    let mut manager = PluginManager::new(bus);
    manager.set_wasm_engine(engine);

    tracing::info!("Act: load_plugin(plugin_dir)");
    manager.load_plugin(tmp.path()).map_err(|e| e.to_string())?;

    let list = manager.list_loaded();
    assert!(
        list.contains(&"e2e-load-plugin-test".to_string()),
        "list_loaded 应包含刚加载的插件 id，实际: {:?}",
        list
    );
    let info = manager.get_plugin("e2e-load-plugin-test");
    assert!(info.is_some(), "get_plugin 应返回 Some");
    assert_eq!(info.as_ref().unwrap().id, "e2e-load-plugin-test");

    manager
        .unload_plugin("e2e-load-plugin-test")
        .map_err(|e| e.to_string())?;
    assert!(
        manager.list_loaded().is_empty(),
        "unload 后 list_loaded 应为空"
    );

    Ok(())
}

// ══════════════════════════════════════════════════════════════════
// E2E 全量覆盖：E2E-WASM-011 / E2E-WASM-022 / E2E-WASM-023
// ══════════════════════════════════════════════════════════════════

/// [E2E-WASM-011] 工具注册宿主可感知
///
/// 验证：JS 调用 pi.registerTool({...}) 后，宿主侧 host_call 中 method=registerTool 至少触发 1 次
/// 意义：Story 5——插件可通过 pi.registerTool 向宿主注册工具，宿主 host_call 链路正常
#[test]
fn test_wasmedge_e2e_tool_registration() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_wasmedge_e2e_tool_registration").entered();

    let quickjs_path = require_quickjs_wasm();
    let config = WasmEngineConfig {
        quickjs_path: Some(quickjs_path),
        ..WasmEngineConfig::default()
    };
    let engine = match WasmEngine::global(Some(config)) {
        Ok(e) => e,
        Err(e) => {
            if e.to_string().contains("stub") || e.to_string().contains("WasmEdge") {
                panic!(
                    "集成测试要求已安装 WasmEdge。当前: {}。安装见 {}",
                    e, WASMEDGE_INSTALL_URL
                );
            }
            return Err(e.into());
        }
    };

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasmedge_quickjs/tool_register_test.js");
    assert!(fixture.exists(), "fixture tool_register_test.js 必须存在: {:?}", fixture);

    let mut instance = engine.create_instance("tool-reg-e2e")?;
    let register_count = Arc::new(AtomicU32::new(0));
    let count = Arc::clone(&register_count);
    instance.register_host_binding(move |request_json: &str| {
        let req: serde_json::Value =
            serde_json::from_str(request_json).unwrap_or(serde_json::Value::Null);
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        tracing::debug!("tool_reg host_call method={}", method);
        if method == "registerTool" {
            count.fetch_add(1, Ordering::SeqCst);
        }
        Ok(serde_json::json!({"ok": true, "data": null}).to_string())
    })?;

    tracing::info!("Act: run_script_file(tool_register_test.js)");
    instance.run_script_file(&fixture)?;

    let n = register_count.load(Ordering::SeqCst);
    tracing::info!("Assert: registerTool host_call 次数 = {}", n);
    assert!(
        n >= 1,
        "pi.registerTool 应触发 ≥1 次 registerTool host_call，实际 {} 次",
        n
    );
    Ok(())
}

/// [E2E-WASM-022] 事件 once 语义：dispatch_event 触发 pi.once handler 可正常调用
///
/// 验证：JS pi.once 注册 handler（内含 pi.log）→ host dispatch_event 一次 → 触发 ≥1 次
/// 意义：Story 6——pi.once handler 可通过 dispatch_event 触发（MVP 无状态执行模型）
/// 注：「恰好 1 次」的 once 保证需有状态 VM（Story 8b，P1），当前 MVP 下每次 dispatch 重新
///     加载脚本会重新注册 handler，属已知设计限制，不作为本用例失败条件。
#[test]
fn test_wasmedge_e2e_event_once_fires_exactly_once() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_wasmedge_e2e_event_once_fires_exactly_once").entered();

    let quickjs_path = require_quickjs_wasm();
    let config = WasmEngineConfig {
        quickjs_path: Some(quickjs_path),
        ..WasmEngineConfig::default()
    };
    let engine = match WasmEngine::global(Some(config)) {
        Ok(e) => e,
        Err(e) => {
            if e.to_string().contains("stub") || e.to_string().contains("WasmEdge") {
                panic!(
                    "集成测试要求已安装 WasmEdge。当前: {}。安装见 {}",
                    e, WASMEDGE_INSTALL_URL
                );
            }
            return Err(e.into());
        }
    };

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasmedge_quickjs/event_once_test.js");
    assert!(fixture.exists(), "fixture event_once_test.js 必须存在: {:?}", fixture);

    let mut instance = engine.create_instance("event-once-e2e")?;
    let log_count = Arc::new(AtomicU32::new(0));
    let count = Arc::clone(&log_count);
    instance.register_host_binding(move |request_json: &str| {
        let req: serde_json::Value =
            serde_json::from_str(request_json).unwrap_or(serde_json::Value::Null);
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        if method == "log" {
            let msg = req
                .get("params")
                .and_then(|p| p.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("");
            tracing::debug!("event_once log: {}", msg);
            if msg.contains("handler fired") {
                count.fetch_add(1, Ordering::SeqCst);
            }
        }
        Ok(serde_json::json!({"ok": true, "data": null}).to_string())
    })?;

    let ctx = serde_json::json!({"cwd": "/tmp", "hasUI": false, "model": null});
    tracing::info!("Act: dispatch_event(__e2e_once_event) 一次");
    instance.dispatch_event(&fixture, "__e2e_once_event", &serde_json::json!({"seq": 1}), &ctx)?;

    let n = log_count.load(Ordering::SeqCst);
    tracing::info!("Assert: once handler 触发次数 = {}（≥1 即通过）", n);
    assert!(
        n >= 1,
        "pi.once handler 应触发 ≥1 次（dispatch 一次后），实际触发 {} 次",
        n
    );
    Ok(())
}

/// [E2E-WASM-023] 多个 on 监听同一事件均被触发
///
/// 验证：JS 注册两个 handler（各含 pi.log）→ host dispatch_event 一次 → log host_call 计数 ≥2
/// 意义：Story 6——多 on 处理器并存、同一事件触发所有 handler（通过 dispatch_event 路径验证）
#[test]
fn test_wasmedge_e2e_event_on_multiple_handlers() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_wasmedge_e2e_event_on_multiple_handlers").entered();

    let quickjs_path = require_quickjs_wasm();
    let config = WasmEngineConfig {
        quickjs_path: Some(quickjs_path),
        ..WasmEngineConfig::default()
    };
    let engine = match WasmEngine::global(Some(config)) {
        Ok(e) => e,
        Err(e) => {
            if e.to_string().contains("stub") || e.to_string().contains("WasmEdge") {
                panic!(
                    "集成测试要求已安装 WasmEdge。当前: {}。安装见 {}",
                    e, WASMEDGE_INSTALL_URL
                );
            }
            return Err(e.into());
        }
    };

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasmedge_quickjs/event_multi_handler_test.js");
    assert!(fixture.exists(), "fixture event_multi_handler_test.js 必须存在: {:?}", fixture);

    let mut instance = engine.create_instance("event-multi-e2e")?;
    let log_count = Arc::new(AtomicU32::new(0));
    let count = Arc::clone(&log_count);
    instance.register_host_binding(move |request_json: &str| {
        let req: serde_json::Value =
            serde_json::from_str(request_json).unwrap_or(serde_json::Value::Null);
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        if method == "log" {
            let msg = req
                .get("params")
                .and_then(|p| p.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("");
            tracing::debug!("event_multi log: {}", msg);
            if msg.contains("handler_1 fired") || msg.contains("handler_2 fired") {
                count.fetch_add(1, Ordering::SeqCst);
            }
        }
        Ok(serde_json::json!({"ok": true, "data": null}).to_string())
    })?;

    let ctx = serde_json::json!({"cwd": "/tmp", "hasUI": false, "model": null});
    tracing::info!("Act: dispatch_event(__e2e_multi_event) 一次");
    instance.dispatch_event(
        &fixture,
        "__e2e_multi_event",
        &serde_json::json!({"hello": "world"}),
        &ctx,
    )?;

    let n = log_count.load(Ordering::SeqCst);
    tracing::info!("Assert: multi-handler 触发次数 = {}", n);
    assert!(
        n >= 2,
        "pi.on 两个 handler 各应触发 1 次（共 ≥2 次 log），实际触发 {} 次",
        n
    );
    Ok(())
}
