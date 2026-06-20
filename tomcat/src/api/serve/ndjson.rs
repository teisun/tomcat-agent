//! NDJSON 序列化与命令反序列化工具。

use serde::Serialize;
use serde_json::Value;

use crate::AppError;

pub fn ndjson_safe_stringify<T: Serialize>(value: &T) -> Result<String, AppError> {
    let json = serde_json::to_string(value)
        .map_err(|error| AppError::Config(format!("serialize ndjson frame failed: {error}")))?;
    Ok(json
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029"))
}

pub fn parse_command_line(line: &str) -> Result<super::types::ServeCommand, AppError> {
    let value: Value =
        serde_json::from_str(line).map_err(|error| AppError::Config(format!("parse_error: {error}")))?;
    reject_explicit_null_session_id(&value)?;
    if let Some(command_type) = value.get("type").and_then(Value::as_str) {
        if !matches!(
            command_type,
            "prompt"
                | "steer"
                | "follow_up"
                | "get_state"
                | "set_model"
                | "new_session"
                | "switch_session"
                | "get_messages"
                | "close_session"
                | "list_sessions"
                | "interrupt"
                | "control_request"
                | "control_response"
                | "control_cancel"
        ) {
            return Err(AppError::Config(format!("unknown_command: {command_type}")));
        }
    }
    serde_json::from_value(value).map_err(|error| AppError::Config(format!("parse_error: {error}")))
}

fn reject_explicit_null_session_id(value: &Value) -> Result<(), AppError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    if object
        .get("sessionId")
        .is_some_and(serde_json::Value::is_null)
    {
        return Err(AppError::Config(
            "invalid_request: sessionId must be omitted or a string".to_string(),
        ));
    }
    Ok(())
}
