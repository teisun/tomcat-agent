//! # 宿主导入绑定骨架 (Host Binding Skeleton)
//!
//! 定义统一 Hostcall 入口 `invoke_host_func` 的协议与类型，供 Wasm 实例调用宿主 API。
//! 具体分发逻辑与 API 实现在 T1-P0-008。

use crate::infra::error::AppError;
use serde::{Deserialize, Serialize};

/// 宿主请求 DTO，与 JS 侧对齐，camelCase 序列化。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRequest {
    /// 模块标识，如 "fs"、"llm"、"agent"。
    pub module: String,
    /// 方法名，如 "readFile"、"createChatCompletion"。
    pub method: String,
    /// 参数（JSON 对象）。
    pub params: serde_json::Value,
    /// 调用 ID，用于异步回传关联。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
}

/// 宿主响应 DTO，与 JS 侧对齐。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostResponse {
    /// 是否成功。
    pub ok: bool,
    /// 成功时结果数据。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// 失败时错误信息。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 关联的 call_id（异步回调时使用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
}

impl HostResponse {
    /// 构造成功响应。
    pub fn ok(data: serde_json::Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
            call_id: None,
        }
    }

    /// 构造失败响应。
    pub fn err(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(error.into()),
            call_id: None,
        }
    }
}

/// 统一 Hostcall 入口：若传入 dispatcher 则按 module/method 分发，否则返回桩响应。
pub fn invoke_host_func(
    instance_id: &str,
    request_json: &str,
) -> Result<HostResponse, AppError> {
    invoke_host_func_with(None, instance_id, request_json)
}

/// 使用指定 HostApiDispatcher 的 Hostcall 入口（插件加载时注入）。
pub fn invoke_host_func_with(
    dispatcher: Option<&super::HostApiDispatcher>,
    instance_id: &str,
    request_json: &str,
) -> Result<HostResponse, AppError> {
    let request: HostRequest = serde_json::from_str(request_json)
        .map_err(|e| AppError::Plugin(format!("hostcall request parse error: {}", e)))?;
    match dispatcher {
        Some(d) => d.dispatch(instance_id, request),
        None => Ok(HostResponse::ok(serde_json::json!({ "stub": true }))),
    }
}

#[cfg(test)]
mod tests {
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
}
