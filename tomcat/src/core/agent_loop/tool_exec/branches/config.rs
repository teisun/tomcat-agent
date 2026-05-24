use crate::infra::events::ToolDisplay;

use super::super::ToolExecCtx;

pub(in super::super) async fn handle_config_get(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
) -> Result<String, String> {
    let Some(backend) = ctx.config_backend.as_ref() else {
        return Err("config 工具未启用：当前会话不允许通过 LLM 读改配置".to_string());
    };
    let key = args["key"].as_str().unwrap_or("");
    backend
        .config_get(key)
        .await
        .map(|v| serde_json::to_string(&v).unwrap_or_else(|_| v.to_string()))
        .map_err(|e| e.to_string())
}

pub(in super::super) async fn handle_config_set(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
    display_out: &mut Option<ToolDisplay>,
) -> Result<String, String> {
    let Some(backend) = ctx.config_backend.as_ref() else {
        return Err("config 工具未启用：当前会话不允许通过 LLM 读改配置".to_string());
    };
    let key = args["key"].as_str().unwrap_or("");
    let value = args["value"].as_str().unwrap_or("");
    backend
        .config_set(key, value)
        .await
        .map(|v| {
            if let Some(text) = v.get("message").and_then(|value| value.as_str()) {
                *display_out = Some(ToolDisplay::Text {
                    text: text.to_string(),
                });
            }
            serde_json::to_string(&v).unwrap_or_else(|_| v.to_string())
        })
        .map_err(|e| e.to_string())
}
