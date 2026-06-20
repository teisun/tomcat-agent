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
    assert!(
        err.to_string().contains("unknown_command: mystery"),
        "unexpected error: {err}"
    );
}

#[test]
fn parse_command_line_rejects_explicit_null_session_id() {
    let err = parse_command_line(r#"{"type":"prompt","id":"u1","sessionId":null,"text":"hello"}"#)
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("invalid_request: sessionId must be omitted or a string"),
        "unexpected error: {err}"
    );
}
