//! 集成测试：Hostcall 全链路（仅用 pi_awsm 公共 API）。
//! 验证 HostApiDispatcher + invoke_host_func_with + HostRequest/HostResponse 的请求/响应与错误透传。

mod common;

use pi_awsm::{DefaultEventBus, HostApiDispatcher, invoke_host_func_with};
use std::sync::Arc;

#[test]
fn test_hostcall_log_via_public_api() {
    common::setup_logging();
    let _span = tracing::info_span!("test_hostcall_log_via_public_api").entered();

    let bus = Arc::new(DefaultEventBus::new());
    let dispatcher = HostApiDispatcher::new(bus);
    let req_json = r#"{"module":"agent","method":"log","params":{"message":"hello from integration"},"callId":null}"#;

    tracing::info!("Arrange: DefaultEventBus + HostApiDispatcher（无 primitive/llm/tools）");
    let res = invoke_host_func_with(Some(&dispatcher), "inst-1", req_json);
    tracing::info!("Act: invoke_host_func_with(dispatcher, inst-1, log request)");

    assert!(res.is_ok(), "log hostcall 应成功");
    let resp = res.unwrap();
    assert!(resp.ok, "响应应为 ok");
    tracing::info!("Assert: 响应 ok，错误透传符合预期");
}

#[test]
fn test_hostcall_read_file_without_primitive_returns_err_via_public_api() {
    common::setup_logging();
    let _span = tracing::info_span!("test_hostcall_read_file_without_primitive_returns_err_via_public_api").entered();

    let bus = Arc::new(DefaultEventBus::new());
    let dispatcher = HostApiDispatcher::new(bus);
    let req_json = r#"{"module":"fs","method":"readFile","params":{"path":"/tmp/x","pluginId":"p1"},"callId":null}"#;

    tracing::info!("Arrange: Dispatcher 未注入 PrimitiveExecutor");
    let res = invoke_host_func_with(Some(&dispatcher), "inst-1", req_json);
    tracing::info!("Act: invoke_host_func_with(dispatcher, inst-1, readFile request)");

    assert!(res.is_ok(), "解析与分发应返回 Ok(HostResponse)");
    let resp = res.unwrap();
    assert!(!resp.ok, "未配置 primitive 时应返回错误响应");
    assert!(
        resp.error.as_ref().map(|e| e.contains("005")).unwrap_or(false),
        "错误信息应提示 005/PrimitiveExecutor"
    );
    tracing::info!("Assert: HostResponse::err 透传，符合 008 错误透传规范");
}

#[test]
fn test_hostcall_unknown_api_returns_err_via_public_api() {
    common::setup_logging();
    let _span = tracing::info_span!("test_hostcall_unknown_api_returns_err_via_public_api").entered();

    let bus = Arc::new(DefaultEventBus::new());
    let dispatcher = HostApiDispatcher::new(bus);
    let req_json = r#"{"module":"unknown","method":"foo","params":{},"callId":null}"#;

    let res = invoke_host_func_with(Some(&dispatcher), "inst-1", req_json);
    assert!(res.is_ok());
    let resp = res.unwrap();
    assert!(!resp.ok);
    assert!(resp.error.as_ref().unwrap().contains("unknown API"));
}
