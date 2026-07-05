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

#[test]
fn parse_command_line_accepts_all_known_command_types() {
    let commands = [
        r#"{"type":"prompt","id":"u1","text":"hello"}"#,
        r#"{"type":"steer","id":"u2","text":"focus"}"#,
        r#"{"type":"follow_up","id":"u3","text":"continue"}"#,
        r#"{"type":"get_state","id":"u4","sessionId":"s1"}"#,
        r#"{"type":"set_plan_mode","id":"u5","sessionId":"s1","action":"enter"}"#,
        r#"{"type":"set_model","id":"u6","sessionId":"s1","model":"gpt-5.4"}"#,
        r#"{"type":"set_thinking_level","id":"u7","sessionId":"s1","model":"gpt-5.4","level":"high"}"#,
        r#"{"type":"list_models","id":"u8"}"#,
        r#"{"type":"new_session","id":"u9","params":{"mode":"code"}}"#,
        r#"{"type":"switch_session","id":"u10","sessionId":"s1"}"#,
        r#"{"type":"get_messages","id":"u11","sessionId":"s1","params":{"limit":20}}"#,
        r#"{"type":"close_session","id":"u12","sessionId":"s1"}"#,
        r#"{"type":"list_sessions","id":"u13","scope":"disk"}"#,
        r#"{"type":"interrupt","id":"u14","sessionId":"s1"}"#,
        r#"{"type":"control_request","requestId":"req-1","subtype":"initialize","payload":{}}"#,
        r#"{"type":"control_response","requestId":"req-2","sessionId":"s1","payload":{"ok":true}}"#,
        r#"{"type":"control_cancel","requestId":"req-3","sessionId":"s1","payload":{"reason":"stop"}}"#,
    ];

    for command in commands {
        parse_command_line(command).unwrap_or_else(|error| {
            panic!("expected command to parse successfully: {command}\nerror: {error}");
        });
    }
}
