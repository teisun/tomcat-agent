//! # `__async` 协议（提交-轮询）覆盖
//!
//! 8.4.8 引入的 hostcall async 协议要点：
//!
//! - `dispatch` 在 `call_id != None` 时立即返回 pending，并在后台 spawn 真正
//!   的 primitive 调用；调用方稍后用 `__async.poll` 取结果。
//! - `sync_path_unchanged_without_call_id`：未带 `call_id` 时按同步路径返回，
//!   保证向下兼容。
//! - `async_poll_*`：覆盖 pending / done / error / 缺 call_id / 已 cleanup 五种状态。
//! - `async_timeout_produces_error`：`with_async_timeout` 触发后，poll 返回的
//!   错误信息包含 `timeout`。
//! - `async_multiple_call_ids_concurrent`：并发提交 5 个 callId 各自独立。
//! - `async_cleanup_instance_removes_pending`：`cleanup_instance` 仅清理对应
//!   实例的 pending，不影响其他实例。

use std::sync::Arc;

use tokio::runtime::Handle;

use super::super::{AsyncCallStatus, HostApiDispatcher};
use super::mocks::make_dispatcher_with_primitive;
use crate::core::{
    BashResult, DirEntry, EditFileResult, EditOperation, PrimitiveExecutor, PrimitiveOperation,
    WriteFileResult,
};
use crate::ext::host_binding::{HostRequest, HostResponse};
use crate::infra::error::AppError;
use crate::infra::DefaultEventBus;

#[tokio::test]
async fn async_submit_poll_full_roundtrip() {
    let d = make_dispatcher_with_primitive();
    let req = HostRequest {
        module: "fs".to_string(),
        method: "executeBash".to_string(),
        params: serde_json::json!({"command": "echo hi"}),
        call_id: Some("req-1".to_string()),
    };
    let submit = d.dispatch("inst-a", req).unwrap();
    assert!(submit.ok);
    assert_eq!(submit.call_id.as_deref(), Some("req-1"));
    assert!(submit
        .data
        .as_ref()
        .unwrap()
        .get("pending")
        .unwrap()
        .as_bool()
        .unwrap());

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let poll_req = HostRequest {
        module: "__async".to_string(),
        method: "poll".to_string(),
        params: serde_json::json!({"callId": "req-1"}),
        call_id: None,
    };
    let poll_res = d.dispatch_async("inst-a", poll_req).await.unwrap();
    assert!(poll_res.ok);
    let data = poll_res.data.unwrap();
    assert!(data.get("ready").unwrap().as_bool().unwrap());
    assert!(data.get("result").is_some());
}

#[tokio::test]
async fn sync_path_unchanged_without_call_id() {
    let d = make_dispatcher_with_primitive();
    let res = tokio::task::spawn_blocking(move || {
        let req = HostRequest {
            module: "fs".to_string(),
            method: "executeBash".to_string(),
            params: serde_json::json!({"command": "echo hi"}),
            call_id: None,
        };
        d.dispatch("inst-a", req)
    })
    .await
    .unwrap()
    .unwrap();
    assert!(res.ok);
    assert!(res.data.as_ref().unwrap().get("stdout").is_some());
}

#[tokio::test]
async fn async_poll_not_ready_immediately() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
    d.async_results
        .insert("pending-1".to_string(), AsyncCallStatus::Pending);
    let poll_req = HostRequest {
        module: "__async".to_string(),
        method: "poll".to_string(),
        params: serde_json::json!({"callId": "pending-1"}),
        call_id: None,
    };
    let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
    assert!(res.ok);
    assert!(!res.data.unwrap().get("ready").unwrap().as_bool().unwrap());
}

#[tokio::test]
async fn async_poll_ready_returns_result() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
    d.async_results.insert(
        "done-1".to_string(),
        AsyncCallStatus::Done(HostResponse::ok(serde_json::json!({"stdout": "hello"}))),
    );
    let poll_req = HostRequest {
        module: "__async".to_string(),
        method: "poll".to_string(),
        params: serde_json::json!({"callId": "done-1"}),
        call_id: None,
    };
    let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
    assert!(res.ok);
    let data = res.data.unwrap();
    assert!(data.get("ready").unwrap().as_bool().unwrap());
    let result = data.get("result").unwrap();
    assert_eq!(result.get("stdout").unwrap().as_str().unwrap(), "hello");
}

#[tokio::test]
async fn async_poll_error_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
    d.async_results.insert(
        "err-1".to_string(),
        AsyncCallStatus::Error("something broke".to_string()),
    );
    let poll_req = HostRequest {
        module: "__async".to_string(),
        method: "poll".to_string(),
        params: serde_json::json!({"callId": "err-1"}),
        call_id: None,
    };
    let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
    assert!(!res.ok);
    assert!(res.error.unwrap().contains("something broke"));
}

#[tokio::test]
async fn async_timeout_produces_error() {
    let bus = Arc::new(DefaultEventBus::new());
    struct SlowPrimitive;
    #[async_trait::async_trait]
    impl PrimitiveExecutor for SlowPrimitive {
        async fn read_file(&self, _: &str, _: &str) -> Result<String, AppError> {
            Ok(String::new())
        }
        async fn list_dir(&self, _: &str, _: &str) -> Result<Vec<DirEntry>, AppError> {
            Ok(vec![])
        }
        async fn write_file(
            &self,
            _: &str,
            _: &str,
            _: bool,
            _: &str,
        ) -> Result<WriteFileResult, AppError> {
            Ok(WriteFileResult {
                path: String::new(),
                written: false,
            })
        }
        async fn edit_file(
            &self,
            _: &str,
            _: Vec<EditOperation>,
            _: &str,
        ) -> Result<EditFileResult, AppError> {
            Ok(EditFileResult {
                path: String::new(),
                applied: false,
            })
        }
        async fn execute_bash(
            &self,
            _: &str,
            _: Option<&str>,
            _: &str,
            _: Option<&[String]>,
        ) -> Result<BashResult, AppError> {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            Ok(BashResult {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            })
        }
        async fn require_user_confirmation(
            &self,
            _: PrimitiveOperation,
            _: &str,
            _: &str,
        ) -> Result<bool, AppError> {
            Ok(true)
        }
    }
    let d = HostApiDispatcher::new(bus)
        .with_tokio_handle(Handle::current())
        .with_primitive(Arc::new(SlowPrimitive))
        .with_async_timeout(std::time::Duration::from_millis(100));
    let req = HostRequest {
        module: "fs".to_string(),
        method: "executeBash".to_string(),
        params: serde_json::json!({"command": "slow"}),
        call_id: Some("timeout-1".to_string()),
    };
    d.dispatch("inst-a", req).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let poll_req = HostRequest {
        module: "__async".to_string(),
        method: "poll".to_string(),
        params: serde_json::json!({"callId": "timeout-1"}),
        call_id: None,
    };
    let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
    assert!(!res.ok);
    assert!(res.error.unwrap().contains("timeout"));
}

#[tokio::test]
async fn async_multiple_call_ids_concurrent() {
    let d = make_dispatcher_with_primitive();
    for i in 0..5 {
        let req = HostRequest {
            module: "fs".to_string(),
            method: "executeBash".to_string(),
            params: serde_json::json!({"command": format!("echo {i}")}),
            call_id: Some(format!("multi-{i}")),
        };
        let submit = d.dispatch("inst-a", req).unwrap();
        assert!(submit.ok);
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    for i in 0..5 {
        let poll_req = HostRequest {
            module: "__async".to_string(),
            method: "poll".to_string(),
            params: serde_json::json!({"callId": format!("multi-{i}")}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
        assert!(res.ok);
        assert!(res.data.unwrap().get("ready").unwrap().as_bool().unwrap());
    }
}

#[tokio::test]
async fn async_cleanup_instance_removes_pending() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
    d.async_results
        .insert("ci-1".to_string(), AsyncCallStatus::Pending);
    d.async_results
        .insert("ci-2".to_string(), AsyncCallStatus::Pending);
    d.instance_calls
        .entry("inst-x".to_string())
        .or_default()
        .extend(["ci-1".to_string(), "ci-2".to_string()]);
    d.async_results
        .insert("other-1".to_string(), AsyncCallStatus::Pending);
    d.instance_calls
        .entry("inst-y".to_string())
        .or_default()
        .push("other-1".to_string());

    d.cleanup_instance("inst-x");

    assert!(d.async_results.get("ci-1").is_none());
    assert!(d.async_results.get("ci-2").is_none());
    assert!(d.async_results.get("other-1").is_some());
}

#[tokio::test]
async fn async_poll_cleans_up_after_ready() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
    d.async_results.insert(
        "once-1".to_string(),
        AsyncCallStatus::Done(HostResponse::ok(serde_json::json!({"v": 42}))),
    );
    let poll_req = || HostRequest {
        module: "__async".to_string(),
        method: "poll".to_string(),
        params: serde_json::json!({"callId": "once-1"}),
        call_id: None,
    };
    let res1 = d.dispatch_async("inst-a", poll_req()).await.unwrap();
    assert!(res1.ok);
    assert!(res1.data.unwrap().get("ready").unwrap().as_bool().unwrap());

    let res2 = d.dispatch_async("inst-a", poll_req()).await.unwrap();
    assert!(!res2.ok);
    assert!(res2.error.unwrap().contains("unknown callId"));
}

#[tokio::test]
async fn async_poll_missing_call_id_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
    let poll_req = HostRequest {
        module: "__async".to_string(),
        method: "poll".to_string(),
        params: serde_json::json!({}),
        call_id: None,
    };
    let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
    assert!(!res.ok);
    assert!(res.error.unwrap().contains("missing callId"));
}
