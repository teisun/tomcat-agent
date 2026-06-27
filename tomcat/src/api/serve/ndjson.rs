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
    parse_command_value(value)
}

pub(crate) fn extract_response_refs(line: &str) -> (Option<String>, Option<String>) {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => return (None, None),
    };
    extract_response_refs_from_value(&value)
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

fn parse_command_value(value: Value) -> Result<super::types::ServeCommand, AppError> {
    let command_type = value
        .get("type")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    serde_json::from_value(value).map_err(|error| map_command_parse_error(command_type.as_deref(), error))
}

fn map_command_parse_error(command_type: Option<&str>, error: serde_json::Error) -> AppError {
    let message = error.to_string();
    if let Some(command_type) = command_type.filter(|_| message.contains("unknown variant")) {
        return AppError::Config(format!("unknown_command: {command_type}"));
    }
    AppError::Config(format!("parse_error: {message}"))
}

fn extract_response_refs_from_value(value: &Value) -> (Option<String>, Option<String>) {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let session_id = value
        .get("sessionId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    (id, session_id)
}
