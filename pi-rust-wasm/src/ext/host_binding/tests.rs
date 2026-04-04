use super::*;

#[test]
fn host_request_response_roundtrip() {
    let req = HostRequest {
        module: "test".to_string(),
        method: "ping".to_string(),
        params: serde_json::json!({ "x": 1 }),
        call_id: Some("id-1".to_string()),
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(!json.contains("camelCase"));
    assert!(json.contains("callId"));
    let back: HostRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(back.module, "test");
    assert_eq!(back.call_id.as_deref(), Some("id-1"));
}

#[test]
fn invoke_host_func_stub() {
    let json = r#"{"module":"x","method":"y","params":{}}"#;
    let res = invoke_host_func("inst-1", json).unwrap();
    assert!(res.ok);
    assert!(res.data.unwrap().get("stub").unwrap().as_bool().unwrap());
}

#[test]
fn invoke_host_func_invalid_json() {
    let res = invoke_host_func("inst-1", "not json");
    assert!(res.is_err());
}

#[test]
fn invoke_host_func_with_dispatcher_routes() {
    use crate::ext::HostApiDispatcher;
    use crate::infra::DefaultEventBus;
    use std::sync::Arc;

    let bus = Arc::new(DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    let json = r#"{"module":"fs","method":"readFile","params":{"path":"/x","pluginId":"p1"}}"#;
    let res = invoke_host_func_with(Some(&d), "inst-1", json).unwrap();
    assert!(!res.ok);
    assert!(res.error.as_ref().unwrap().contains("005"));
}
