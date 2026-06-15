//! # `HostApiDispatcher` 缺扩展时的兜底分支
//!
//! 这些用例覆盖：
//!
//! - 未注册 `primitive` / `llm` / `tools` / `session` 时，相应模块返回错误，
//!   并保留 005/006/004 等错误码用于上层映射。
//! - `agent.log` 永远成功；`events.on/emit` 不依赖外部扩展也能工作。
//! - `with_audit` 注入的 `AuditRecorder` 在 hostcall 路径上准确触发一次。
//!
//! 与 `dispatch_with_extension` 互补：那里的用例都已经显式 `with_*` 注入扩展。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::super::HostApiDispatcher;
use crate::ext::host_binding::HostRequest;
use crate::infra::{AuditRecorder, DefaultEventBus};

#[tokio::test]
async fn dispatch_unknown_api_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    let req = HostRequest {
        module: "unknown".to_string(),
        method: "foo".to_string(),
        params: serde_json::json!({}),
        call_id: None,
    };
    let res = d.dispatch_async("inst-1", req).await.unwrap();
    assert!(!res.ok);
    assert!(res.error.unwrap().contains("unknown API"));
}

#[tokio::test]
async fn dispatch_log_succeeds() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    let req = HostRequest {
        module: "agent".to_string(),
        method: "log".to_string(),
        params: serde_json::json!({ "message": "hello" }),
        call_id: None,
    };
    let res = d.dispatch_async("inst-1", req).await.unwrap();
    assert!(res.ok);
}

#[tokio::test]
async fn dispatch_read_file_without_primitive_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    let req = HostRequest {
        module: "fs".to_string(),
        method: "readFile".to_string(),
        params: serde_json::json!({ "path": "/tmp/x", "pluginId": "p1" }),
        call_id: None,
    };
    let res = d.dispatch_async("inst-1", req).await.unwrap();
    assert!(!res.ok);
    assert!(res.error.unwrap().contains("005"));
}

#[tokio::test]
async fn dispatch_session_get_current_without_session_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    let req = HostRequest {
        module: "session".to_string(),
        method: "getCurrentSession".to_string(),
        params: serde_json::json!({}),
        call_id: None,
    };
    let res = d.dispatch_async("inst-1", req).await.unwrap();
    assert!(!res.ok);
    assert!(res.error.unwrap().contains("SessionManager not configured"));
}

#[tokio::test]
async fn dispatch_events_on_returns_listener_id() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    let req = HostRequest {
        module: "events".to_string(),
        method: "on".to_string(),
        params: serde_json::json!({ "eventName": "test_event" }),
        call_id: None,
    };
    let res = d.dispatch_async("inst-1", req).await.unwrap();
    assert!(res.ok);
    let data = res.data.unwrap();
    assert!(data.get("listenerId").is_some());
}

#[tokio::test]
async fn dispatch_events_emit_succeeds() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    let req = HostRequest {
        module: "events".to_string(),
        method: "emit".to_string(),
        params: serde_json::json!({ "eventName": "ev", "payload": {} }),
        call_id: None,
    };
    let res = d.dispatch_async("inst-1", req).await.unwrap();
    assert!(res.ok);
}

#[tokio::test]
async fn dispatch_with_audit_records_hostcall() {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    struct CountAudit;
    impl AuditRecorder for CountAudit {
        fn record_primitive(&self, _: crate::infra::PrimitiveAuditEntry) {}
        fn record_tool_call(&self, _: crate::infra::ToolAuditEntry) {}
        fn record_hostcall(&self, _: crate::infra::HostcallAuditEntry) {
            COUNT.fetch_add(1, Ordering::SeqCst);
        }
        fn record_plugin_lifecycle(&self, _: crate::infra::PluginLifecycleAuditEntry) {}
    }
    let bus = Arc::new(DefaultEventBus::new());
    let audit = Arc::new(CountAudit);
    let d = HostApiDispatcher::new(bus).with_audit(audit);
    let req = HostRequest {
        module: "agent".to_string(),
        method: "log".to_string(),
        params: serde_json::json!({ "message": "audit test" }),
        call_id: None,
    };
    let _ = d.dispatch_async("inst-1", req).await.unwrap();
    assert_eq!(COUNT.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn dispatch_tools_without_registry_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    let req = HostRequest {
        module: "tools".to_string(),
        method: "getToolList".to_string(),
        params: serde_json::json!({}),
        call_id: None,
    };
    let res = d.dispatch_async("inst-1", req).await.unwrap();
    assert!(!res.ok);
    assert!(res.error.unwrap().contains("006"));
}

#[tokio::test]
async fn dispatch_llm_without_provider_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    let req = HostRequest {
        module: "llm".to_string(),
        method: "createChatCompletion".to_string(),
        params: serde_json::json!({ "messages": [], "model": "default" }),
        call_id: None,
    };
    let res = d.dispatch_async("inst-1", req).await.unwrap();
    assert!(!res.ok);
    assert!(res.error.unwrap().contains("004"));
}
