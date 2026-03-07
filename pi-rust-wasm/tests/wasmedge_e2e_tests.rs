//! Wasm E2E 集成测试：真实 WasmEngine + run_script + host_call 链路。
//! 须在安装 WasmEdge 后以 `cargo test --features wasmedge --test wasmedge_e2e_tests` 运行；
//! 环境缺失时用例失败、不允许跳过，见 INTEGRATION_TEST_SPEC 5.4 与 docs/02-wasm-runtime-and-plugin.md。

mod common;

use pi_awsm::{WasmEngine, WasmEngineConfig};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

const WASMEDGE_INSTALL_URL: &str = "https://wasmedge.org/docs/start/install";

/// 真实 Wasm 运行时 E2E：创建引擎与实例、注册 host_binding、run_script，断言宿主可调或脚本执行成功。
/// 环境缺失不允许跳过：未安装 WasmEdge 或 quickjs 路径不可用时用例失败（panic），见 INTEGRATION_TEST_SPEC 5.4。
#[test]
fn test_wasmedge_e2e_engine_instance_run_script() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!(
        "test_wasmedge_e2e_engine_instance_run_script"
    )
    .entered();

    let quickjs_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/wasm/wasmedge_quickjs.wasm");
    if !quickjs_path.exists() {
        panic!(
            "集成测试要求 wasmedge_quickjs.wasm 路径存在。请设置 WASMEDGE_QUICKJS_PATH 或确保 {:?} 存在。见 INTEGRATION_TEST_SPEC 5.4 与 docs/02-wasm-runtime-and-plugin.md",
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
                    "集成测试要求已安装 WasmEdge 并以 cargo test --features wasmedge --test wasmedge_e2e_tests 运行，不得跳过。当前: {}。安装见 {}，规范见 INTEGRATION_TEST_SPEC 5.4 与 docs/02-wasm-runtime-and-plugin.md",
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
