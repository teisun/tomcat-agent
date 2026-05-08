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
    /// 参数（JSON 对象）。JS 侧如 __session.waitForEvent 可能不传 params，默认空对象。
    #[serde(default)]
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
///
/// # Arguments
/// * `instance_id` - 插件实例 ID，用于审计与会话关联。
/// * `request_json` - 序列化后的 [`HostRequest`] JSON。
///
/// # Returns
/// 成功时返回 [`HostResponse`]（含 ok/data 或 error）；桩模式下返回 `{ "stub": true }`。
///
/// # Errors
/// * [`AppError::Plugin`] - `request_json` 解析失败时返回。
pub fn invoke_host_func(instance_id: &str, request_json: &str) -> Result<HostResponse, AppError> {
    invoke_host_func_with(None, instance_id, request_json)
}

/// 使用指定 HostApiDispatcher 的 Hostcall 入口（插件加载时注入）。
///
/// # Arguments
/// * `dispatcher` - 若为 `Some` 则按 module/method 分发；`None` 时返回桩响应。
/// * `instance_id` - 插件实例 ID。
/// * `request_json` - 序列化后的 [`HostRequest`] JSON。
///
/// # Returns
/// 成功时返回 [`HostResponse`]；分发失败时 `HostResponse.ok == false` 且带 error 信息。
///
/// # Errors
/// * [`AppError::Plugin`] - `request_json` 解析失败时返回；分发逻辑内部错误通过 `HostResponse::err` 返回。
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
