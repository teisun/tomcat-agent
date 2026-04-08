use super::*;
use crate::core::session::transcript::{CompactionEntry, MessageEntry, TranscriptEntry};

fn mk_user(text: &str) -> Value {
    serde_json::json!({ "role": "user", "content": text })
}
fn mk_system(text: &str) -> Value {
    serde_json::json!({ "role": "system", "content": text })
}
fn mk_assistant(text: &str) -> Value {
    serde_json::json!({ "role": "assistant", "content": text })
}
fn mk_assistant_tc(ids: &[&str]) -> Value {
    let tcs: Vec<Value> = ids
        .iter()
        .map(|id| {
            serde_json::json!({
                "id": *id,
                "type": "function",
                "function": { "name": "read_file", "arguments": "{}" }
            })
        })
        .collect();
    serde_json::json!({ "role": "assistant", "tool_calls": tcs })
}
fn mk_tool(tc_id: &str) -> Value {
    serde_json::json!({ "role": "tool", "tool_call_id": tc_id, "content": "ok" })
}

#[test]
fn validate_empty_then_user() {
    assert!(validate_append_message(&mk_user("hi"), &[]).is_ok());
}

#[test]
fn validate_empty_then_tool() {
    let r = validate_append_message(&mk_tool("c1"), &[]);
    assert!(r.is_err(), "tool as first entry should fail");
}

#[test]
fn validate_empty_then_assistant_tc() {
    assert!(validate_append_message(&mk_assistant_tc(&["c1"]), &[]).is_ok());
}

#[test]
fn validate_user_then_tool() {
    let recent = vec![mk_user("q")];
    let r = validate_append_message(&mk_tool("c1"), &recent);
    assert!(r.is_err());
}

#[test]
fn validate_assistant_tc_then_matching_tool() {
    let recent = vec![mk_assistant_tc(&["c1", "c2"])];
    assert!(validate_append_message(&mk_tool("c1"), &recent).is_ok());
}

#[test]
fn validate_assistant_tc_then_mismatched_tool() {
    let recent = vec![mk_assistant_tc(&["c1"])];
    let r = validate_append_message(&mk_tool("c99"), &recent);
    assert!(r.is_err());
}

#[test]
fn validate_tool_missing_tool_call_id() {
    let recent = vec![mk_assistant_tc(&["c1"])];
    let bad = serde_json::json!({ "role": "tool", "content": "ok" });
    assert!(validate_append_message(&bad, &recent).is_err());
}

#[test]
fn validate_duplicate_tool_call_id() {
    let recent = vec![mk_assistant_tc(&["c1", "c2"]), mk_tool("c1")];
    let r = validate_append_message(&mk_tool("c1"), &recent);
    assert!(r.is_err(), "duplicate tool_call_id should fail");
}

#[test]
fn validate_assistant_tc_then_assistant() {
    let recent = vec![mk_assistant_tc(&["c1"])];
    assert!(validate_append_message(&mk_assistant("hi"), &recent).is_err());
    assert!(validate_append_message(&mk_assistant_tc(&["c2"]), &recent).is_err());
}

#[test]
fn validate_tool_then_plain_assistant() {
    let recent = vec![mk_assistant_tc(&["c1"]), mk_tool("c1")];
    assert!(validate_append_message(&mk_assistant("done"), &recent).is_ok());
}

#[test]
fn validate_bad_tool_calls_shape() {
    let bad = serde_json::json!({
        "role": "assistant",
        "tool_calls": [{ "id": "c1", "type": "function", "function": {} }]
    });
    assert!(validate_append_message(&bad, &[]).is_err());
}

#[test]
fn validate_pending_tool_round_then_user() {
    let recent = vec![mk_assistant_tc(&["c1"])];
    assert!(validate_append_message(&mk_user("q"), &recent).is_err());
}

#[test]
fn validate_pending_tool_round_then_system() {
    let recent = vec![mk_assistant_tc(&["c1"])];
    assert!(validate_append_message(&mk_system("sys"), &recent).is_err());
}

#[test]
fn validate_partial_tool_round_then_user() {
    let recent = vec![mk_assistant_tc(&["c1", "c2"]), mk_tool("c1")];
    assert!(validate_append_message(&mk_user("q"), &recent).is_err());
}

#[test]
fn validate_complete_tool_round_then_user() {
    let recent = vec![mk_assistant_tc(&["c1", "c2"]), mk_tool("c1"), mk_tool("c2")];
    assert!(validate_append_message(&mk_user("q"), &recent).is_ok());
}

#[test]
fn validate_unknown_role() {
    let bad = serde_json::json!({ "role": "function", "content": "x" });
    assert!(validate_append_message(&bad, &[]).is_err());
}

#[test]
fn validate_complete_round_then_user() {
    let recent = vec![
        mk_user("q"),
        mk_assistant_tc(&["c1"]),
        mk_tool("c1"),
        mk_assistant("done"),
    ];
    assert!(validate_append_message(&mk_user("next"), &recent).is_ok());
}

#[test]
fn validate_multi_tool_consecutive() {
    let recent = vec![
        mk_assistant_tc(&["c1", "c2", "c3"]),
        mk_tool("c1"),
        mk_tool("c2"),
    ];
    assert!(validate_append_message(&mk_tool("c3"), &recent).is_ok());
}

#[test]
fn collect_skips_non_message() {
    let entries = vec![
        TranscriptEntry::Message(MessageEntry {
            id: Some("1".into()),
            parent_id: None,
            timestamp: "t".into(),
            message: mk_user("a"),
        }),
        TranscriptEntry::Compaction(CompactionEntry {
            id: None,
            parent_id: None,
            timestamp: "t".into(),
            summary: Some("s".into()),
            covered_start_id: None,
            covered_end_id: None,
            covered_count: None,
            is_boundary: None,
            preheat_compaction_id: None,
        }),
        TranscriptEntry::Message(MessageEntry {
            id: Some("2".into()),
            parent_id: None,
            timestamp: "t".into(),
            message: mk_assistant("b"),
        }),
    ];
    let msgs = collect_recent_chat_messages_from_tail(&entries);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["role"], "user");
    assert_eq!(msgs[1]["role"], "assistant");
}

#[test]
fn pending_tool_round_detection() {
    assert!(is_in_pending_tool_round(&[mk_assistant_tc(&["c1"])]));
    assert!(is_in_pending_tool_round(&[
        mk_assistant_tc(&["c1", "c2"]),
        mk_tool("c1")
    ]));
    assert!(!is_in_pending_tool_round(&[
        mk_assistant_tc(&["c1"]),
        mk_tool("c1")
    ]));
    assert!(!is_in_pending_tool_round(&[]));
    assert!(!is_in_pending_tool_round(&[mk_user("q")]));
}
