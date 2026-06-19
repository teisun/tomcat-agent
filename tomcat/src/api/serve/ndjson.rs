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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ndjson_safe_stringify_escapes_line_separators() {
        let frame = json!({ "text": "a\u{2028}b\u{2029}c" });
        let rendered = ndjson_safe_stringify(&frame).unwrap();
        assert!(rendered.contains("\\u2028"));
        assert!(rendered.contains("\\u2029"));
        assert!(!rendered.contains('\u{2028}'));
        assert!(!rendered.contains('\u{2029}'));
    }

    #[test]
    fn parse_command_line_rejects_bad_json() {
        let err = parse_command_line("{bad json").unwrap_err();
        assert!(err.to_string().contains("parse_error"));
    }

    #[test]
    fn parse_command_line_rejects_unknown_command_type() {
        let err = parse_command_line(r#"{"type":"mystery","id":"u1"}"#).unwrap_err();
        assert_eq!(err.to_string(), "unknown_command: mystery");
    }
}
