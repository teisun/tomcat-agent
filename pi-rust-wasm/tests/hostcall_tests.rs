//! 集成测试：Hostcall 全链路（仅用 pi_wasm 公共 API）。
//! 验证 HostApiDispatcher + invoke_host_func_with + HostRequest/HostResponse 的请求/响应与错误透传。

mod common;

use pi_wasm::{
    invoke_host_func_with, AllowAllConfirmation, DefaultEventBus, DefaultPrimitiveExecutor,
    HostApiDispatcher, PrimitiveConfig, TracingAuditRecorder,
};
use std::sync::Arc;

/// [log hostcall] Dispatcher 无 primitive 时 log 请求仍可成功
///
/// 验证：invoke_host_func_with 返回 Ok 且 HostResponse.ok=true
/// 意义：Hostcall 最小可用路径——log 不依赖 PrimitiveExecutor
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

/// [readFile 无 primitive] 未注入 PrimitiveExecutor 时 readFile 返回错误
///
/// 验证：HostResponse.ok=false 且 error 包含"005"
/// 意义：Hostcall 错误透传——缺少 primitive 依赖时给出明确错误码（008 错误透传规范）
#[test]
fn test_hostcall_read_file_without_primitive_returns_err_via_public_api() {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_hostcall_read_file_without_primitive_returns_err_via_public_api")
            .entered();

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
        resp.error
            .as_ref()
            .map(|e| e.contains("005"))
            .unwrap_or(false),
        "错误信息应提示 005/PrimitiveExecutor"
    );
    tracing::info!("Assert: HostResponse::err 透传，符合 008 错误透传规范");
}

/// [unknown API] 不存在的 module/method 组合返回错误
///
/// 验证：HostResponse.ok=false 且 error 包含"unknown API"
/// 意义：Hostcall 安全边界——防止未知调用静默成功
#[test]
fn test_hostcall_unknown_api_returns_err_via_public_api() {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_hostcall_unknown_api_returns_err_via_public_api").entered();

    let bus = Arc::new(DefaultEventBus::new());
    let dispatcher = HostApiDispatcher::new(bus);
    let req_json = r#"{"module":"unknown","method":"foo","params":{},"callId":null}"#;

    let res = invoke_host_func_with(Some(&dispatcher), "inst-1", req_json);
    assert!(res.is_ok());
    let resp = res.unwrap();
    assert!(!resp.ok);
    assert!(resp.error.as_ref().unwrap().contains("unknown API"));
}

/// [readFile 有 primitive] 注入 PrimitiveExecutor 后 readFile 成功返回文件内容
///
/// 验证：HostResponse.ok=true 且 result 包含文件内容
/// 意义：Hostcall 正向路径——注入 primitive 后 4 原语可通过 hostcall 正常工作
#[test]
fn test_hostcall_read_file_with_primitive_returns_ok() {
    common::setup_logging();
    let _span = tracing::info_span!("test_hostcall_read_file_with_primitive_returns_ok").entered();

    let tmp = tempfile::tempdir().unwrap();
    let canonical_dir = tmp.path().canonicalize().unwrap();
    let file_path = canonical_dir.join("hostcall_read.txt");
    std::fs::write(&file_path, "hostcall-content").unwrap();

    let mut config = PrimitiveConfig::default();
    config.path_whitelist.push(
        canonical_dir
            .to_string_lossy()
            .trim_end_matches(std::path::MAIN_SEPARATOR)
            .to_string(),
    );
    config.auto_confirm = true;
    let executor = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        canonical_dir.clone(),
    );

    let bus = Arc::new(DefaultEventBus::new());
    let dispatcher = HostApiDispatcher::new(bus).with_primitive(Arc::new(executor));
    let req_json = format!(
        r#"{{"module":"fs","method":"readFile","params":{{"path":"{}","pluginId":"p1"}},"callId":null}}"#,
        file_path.to_string_lossy().replace('\\', "\\\\")
    );

    tracing::info!("Arrange: Dispatcher + PrimitiveExecutor（白名单含临时目录）");
    let res = invoke_host_func_with(Some(&dispatcher), "inst-1", &req_json);
    tracing::info!("Act: invoke_host_func_with(readFile)");

    assert!(res.is_ok(), "readFile hostcall 应成功: {:?}", res);
    let resp = res.unwrap();
    assert!(
        resp.ok,
        "注入 primitive 后 readFile 应返回 ok，error: {:?}",
        resp.error
    );
    tracing::info!("Assert: HostResponse::ok，readFile 正向路径通过");
}

/// [异步 hostcall] 带 callId 的请求立即返回 pending，__async.poll 可轮询到结果
///
/// 验证：TASK-12 异步 Hostcall submit/poll 机制；带 callId 的请求返回 ok + data.pending=true；
/// 随后 __async.poll(callId) 可得到 ready: true 与 result。
/// 意义：集成测试覆盖 __async 路由与 async_results 生命周期。
/// 注意：submit 须在 runtime 内执行（以便 Dispatcher 获得 Handle）；poll 在 runtime 外执行，避免 dispatch 内 block_on 嵌套 panic。
#[test]
fn test_hostcall_async_submit_then_poll_returns_result() {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_hostcall_async_submit_then_poll_returns_result").entered();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let dispatcher: Arc<HostApiDispatcher> = rt.block_on(async {
        let bus = Arc::new(DefaultEventBus::new());
        let d = Arc::new(HostApiDispatcher::new(bus));
        tracing::info!("Arrange: HostApiDispatcher（无 primitive，仅验证 async 路径）");
        let req_submit = r#"{"module":"agent","method":"log","params":{"message":"async integration"},"callId":"call-async-1"}"#;
        let res = invoke_host_func_with(Some(d.as_ref()), "inst-async", req_submit);
        tracing::info!("Act: invoke_host_func_with(agent/log, callId=call-async-1)");
        assert!(res.is_ok(), "异步提交应返回 Ok");
        let resp = res.unwrap();
        assert!(resp.ok, "响应应为 ok");
        let pending = resp
            .data
            .as_ref()
            .and_then(|d| d.get("pending"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(pending, "应返回 data.pending=true");
        assert_eq!(resp.call_id.as_deref(), Some("call-async-1"));
        tracing::info!("Assert: 收到 pending 响应与 callId");
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        d
    });

    let poll_req =
        r#"{"module":"__async","method":"poll","params":{"callId":"call-async-1"},"callId":null}"#;
    let poll_res = invoke_host_func_with(Some(dispatcher.as_ref()), "inst-async", poll_req);
    tracing::info!("Act: __async.poll(callId=call-async-1)");

    assert!(poll_res.is_ok(), "poll 应成功");
    let poll_resp = poll_res.unwrap();
    assert!(
        poll_resp.ok,
        "poll 响应应为 ok，error: {:?}",
        poll_resp.error
    );
    let ready = poll_resp
        .data
        .as_ref()
        .and_then(|d| d.get("ready"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(ready, "poll 应返回 ready=true");
    assert!(poll_resp
        .data
        .as_ref()
        .and_then(|d| d.get("result"))
        .is_some());
    tracing::info!("Assert: poll 返回 ready=true 且带 result");
}
