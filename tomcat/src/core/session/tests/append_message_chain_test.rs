use super::*;
use crate::core::session::transcript::{BranchSummaryEntry, MessageEntry, TranscriptEntry};
use serde_json::Value;

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
                "function": { "name": "read", "arguments": "{}" }
            })
        })
        .collect();
    serde_json::json!({ "role": "assistant", "tool_calls": tcs })
}
fn mk_assistant_tc_with_arguments(arguments: Value) -> Value {
    serde_json::json!({
        "role": "assistant",
        "tool_calls": [{
            "id": "c1",
            "type": "function",
            "function": { "name": "read", "arguments": arguments }
        }]
    })
}
fn mk_assistant_tc_missing_arguments() -> Value {
    serde_json::json!({
        "role": "assistant",
        "tool_calls": [{
            "id": "c1",
            "type": "function",
            "function": { "name": "read" }
        }]
    })
}
fn mk_tool(tc_id: &str) -> Value {
    serde_json::json!({ "role": "tool", "tool_call_id": tc_id, "content": "ok" })
}

fn transcript_message(id: &str, message: Value) -> TranscriptEntry {
    TranscriptEntry::Message(MessageEntry {
        id: Some(id.to_string()),
        parent_id: None,
        timestamp: "t".to_string(),
        message,
    })
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
fn validate_duplicate_tool_call_id_ignores_superseded_result() {
    let recent = vec![
        mk_assistant_tc(&["c1", "c2"]),
        serde_json::json!({
            "role": "tool",
            "tool_call_id": "c1",
            "content": "[pending]",
            "superseded": true,
        }),
    ];
    assert!(
        validate_append_message(&mk_tool("c1"), &recent).is_ok(),
        "superseded tool results must not block a replacement result"
    );
}

#[test]
fn pending_tool_call_predicate_uses_latest_non_superseded_result() {
    let message = |id: &str, content: &str, superseded: bool| {
        TranscriptEntry::Message(MessageEntry {
            id: Some(id.to_string()),
            parent_id: None,
            timestamp: "t".to_string(),
            message: serde_json::json!({
                "role": "tool",
                "tool_call_id": "ask-1",
                "content": content,
                "superseded": superseded,
            }),
        })
    };

    assert!(
        is_tool_call_pending(&[message("pending", "[pending]", false)], "ask-1"),
        "an active pending result keeps the question open"
    );
    assert!(
        !is_tool_call_pending(
            &[
                message("pending", "[pending]", true),
                message("answer", "{\"outcome\":\"answered\"}", false),
            ],
            "ask-1",
        ),
        "a newer real answer closes the question even if history retains its placeholder"
    );
    assert!(
        !is_tool_call_pending(
            &[
                message("answer", "{\"outcome\":\"answered\"}", false),
                message("pending", "[pending]", true),
            ],
            "ask-1",
        ),
        "superseded pending history never reopens a completed question"
    );
}

#[test]
fn outbound_guard_detects_unpaired_tool_calls_but_accepts_paired_or_pending_results() {
    let declaration = crate::core::llm::ChatMessage::assistant_with_tool_calls(
        None,
        vec![serde_json::json!({
            "id": "call-1",
            "type": "function",
            "function": { "name": "ask_question", "arguments": "{}" },
        })],
    );
    assert!(
        has_dangling_tool_calls_in_messages(std::slice::from_ref(&declaration)),
        "unpaired declaration must never reach provider"
    );
    assert!(
        !has_dangling_tool_calls_in_messages(&[
            declaration.clone(),
            crate::core::llm::ChatMessage::tool("call-1", "[pending]"),
        ]),
        "the pending placeholder structurally closes the provider protocol"
    );
    assert!(
        !has_dangling_tool_calls_in_messages(&[
            declaration,
            crate::core::llm::ChatMessage::tool(
                "call-1",
                r#"{"outcome":"skipped","cancelled":true,"answers":[]}"#,
            ),
        ]),
        "a terminal result also closes the provider protocol"
    );
}

#[test]
fn resume_tail_predicate_ignores_trailing_tool_media() {
    let mut complete = vec![
        transcript_message("user", mk_user("inspect")),
        transcript_message("assistant", mk_assistant_tc(&["call-1"])),
        transcript_message("tool", mk_tool("call-1")),
        transcript_message("media", serde_json::json!({"role":"user","kind":"tool_media","content":[]})),
    ];
    assert!(has_complete_tail_tool_results(&complete));
    complete.push(transcript_message("new-user", mk_user("new request")));
    assert!(!has_complete_tail_tool_results(&complete));
}

#[test]
fn resume_tail_predicate_requires_a_complete_active_tool_round() {
    let complete = vec![
        transcript_message("user", mk_user("inspect it")),
        transcript_message("assistant", mk_assistant_tc(&["call-1", "call-2"])),
        transcript_message("tool-1", mk_tool("call-1")),
        transcript_message("tool-2", mk_tool("call-2")),
    ];
    assert!(
        has_complete_tail_tool_results(&complete),
        "every declared call has a tail result in declaration order"
    );

    let incomplete = complete[..3].to_vec();
    assert!(
        !has_complete_tail_tool_results(&incomplete),
        "a missing result must not enter the no-input Resume path"
    );
}

#[test]
fn resume_tail_predicate_rejects_old_tool_results_hidden_by_a_failed_user() {
    let mut failed_user = mk_user("retry the last request");
    failed_user["superseded"] = serde_json::json!(true);
    failed_user["turn_failed"] = serde_json::json!(true);
    let entries = vec![
        transcript_message("old-user", mk_user("read the file")),
        transcript_message("old-assistant", mk_assistant_tc(&["old-call"])),
        transcript_message("old-tool", mk_tool("old-call")),
        transcript_message("failed-user", failed_user),
    ];

    assert!(
        !has_complete_tail_tool_results(&entries),
        "the raw tail is the failed user, not the older tool result that active projection exposes"
    );
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
fn validate_invalid_tool_call_arguments_json_is_rejected() {
    let bad = mk_assistant_tc_with_arguments(serde_json::json!("{\"country\":\""));
    let err = validate_append_message(&bad, &[]).expect_err("invalid JSON must be rejected");
    assert!(
        err.contains("tool_calls[0].function.arguments is not valid JSON"),
        "unexpected error: {err}"
    );
}

#[test]
fn validate_missing_tool_call_arguments_is_rejected() {
    let bad = mk_assistant_tc_missing_arguments();
    let err = validate_append_message(&bad, &[]).expect_err("missing arguments must be rejected");
    assert!(
        err.contains("tool_calls[0].function.arguments is missing or not a string"),
        "unexpected error: {err}"
    );
}

#[test]
fn validate_non_string_tool_call_arguments_is_rejected() {
    let bad = mk_assistant_tc_with_arguments(serde_json::json!({ "path": "/tmp/demo" }));
    let err =
        validate_append_message(&bad, &[]).expect_err("non-string arguments must be rejected");
    assert!(
        err.contains("tool_calls[0].function.arguments is missing or not a string"),
        "unexpected error: {err}"
    );
}

#[test]
fn validate_valid_tool_call_arguments_json_still_passes() {
    let good = mk_assistant_tc_with_arguments(serde_json::json!("{\"q\":\"x\"}"));
    assert!(validate_append_message(&good, &[]).is_ok());
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
        TranscriptEntry::BranchSummary(BranchSummaryEntry {
            id: None,
            parent_id: None,
            timestamp: "t".into(),
            summary: Some("s".into()),
            covered_start_id: None,
            covered_end_id: None,
            covered_count: None,
            is_boundary: None,
            preheat_compaction_id: None,
            estimated_covered_tokens_before: None,
            estimated_summary_tokens: None,
            estimated_tokens_saved: None,
            error: None,
            attempts: None,
            superseded: false,
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
fn collect_skips_superseded_messages_like_context_hydration() {
    let entries = vec![
        TranscriptEntry::Message(MessageEntry {
            id: Some("active".into()),
            parent_id: None,
            timestamp: "t".into(),
            message: mk_user("active"),
        }),
        TranscriptEntry::Message(MessageEntry {
            id: Some("discarded".into()),
            parent_id: None,
            timestamp: "t".into(),
            message: {
                let mut message = mk_assistant_tc(&["discarded-call"]);
                message
                    .as_object_mut()
                    .expect("assistant message is an object")
                    .insert("superseded".to_string(), serde_json::json!(true));
                message
            },
        }),
    ];

    let messages = collect_recent_chat_messages_from_tail(&entries);
    assert_eq!(messages, vec![mk_user("active")]);
    assert!(
        validate_append_message(&mk_user("next"), &messages).is_ok(),
        "append validation must not observe messages hidden from hydrated context"
    );
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
