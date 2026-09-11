//! # 原地改写 / 插入条目
//!
//! 覆盖两组写路径：
//!
//! - `insert_entry_after_message_id_*`：在指定锚点 message 之后插入新条目，
//!   并保持原有更晚消息的相对顺序。

use super::super::transcript::*;

#[test]
fn insert_entry_after_message_id_inserts_before_later_messages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("insert_anchor.jsonl");
    write_header(
        &path,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "sid".to_string(),
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();

    let m_anchor = TranscriptEntry::Message(MessageEntry {
        id: Some("mid_anchor".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:01.000Z".to_string(),
        message: serde_json::json!({"role": "user", "content": "u"}),
    });
    append_entry(&path, &m_anchor).unwrap();

    let m_later = TranscriptEntry::Message(MessageEntry {
        id: Some("mid_later".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:02.000Z".to_string(),
        message: serde_json::json!({"role": "assistant", "content": "a"}),
    });
    append_entry(&path, &m_later).unwrap();

    let c = TranscriptEntry::BranchSummary(BranchSummaryEntry {
        id: Some("S::E".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:03.000Z".to_string(),
        summary: Some("sum".to_string()),
        covered_start_id: Some("S".to_string()),
        covered_end_id: Some("mid_anchor".to_string()),
        covered_count: Some(1),
        is_boundary: Some(false),
        preheat_compaction_id: None,
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        error: None,
        attempts: None,
    });
    insert_entry_after_message_id(&path, "mid_anchor", &c).unwrap();

    let entries = read_entries_tail(&path, 10).unwrap();
    assert_eq!(entries.len(), 3);
    assert!(matches!(&entries[0], TranscriptEntry::Message(_)));
    assert!(matches!(&entries[1], TranscriptEntry::BranchSummary(_)));
    assert!(matches!(&entries[2], TranscriptEntry::Message(_)));
}

#[test]
fn mark_message_entries_after_anchor_superseded_marks_only_later_messages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("superseded.jsonl");
    write_header(
        &path,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "sid".to_string(),
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();

    append_entry(
        &path,
        &TranscriptEntry::Message(MessageEntry {
            id: Some("m1".to_string()),
            parent_id: None,
            timestamp: "2025-01-01T00:00:01.000Z".to_string(),
            message: serde_json::json!({"role":"user","content":"u1"}),
        }),
    )
    .unwrap();
    append_entry(
        &path,
        &TranscriptEntry::Message(MessageEntry {
            id: Some("m2".to_string()),
            parent_id: None,
            timestamp: "2025-01-01T00:00:02.000Z".to_string(),
            message: serde_json::json!({"role":"assistant","content":"a1"}),
        }),
    )
    .unwrap();
    append_entry(
        &path,
        &TranscriptEntry::Message(MessageEntry {
            id: Some("m3".to_string()),
            parent_id: None,
            timestamp: "2025-01-01T00:00:03.000Z".to_string(),
            message: serde_json::json!({"role":"assistant","content":"a2"}),
        }),
    )
    .unwrap();

    let changed = mark_message_entries_after_anchor_superseded(&path, "m1").unwrap();
    assert_eq!(changed, 2);

    let entries = read_entries_tail(&path, 10).unwrap();
    match &entries[0] {
        TranscriptEntry::Message(me) => assert_eq!(
            me.message.get("superseded").and_then(|v| v.as_bool()),
            None,
            "锚点本身不应被标 superseded"
        ),
        other => panic!("unexpected first entry: {other:?}"),
    }
    for entry in &entries[1..] {
        match entry {
            TranscriptEntry::Message(me) => assert_eq!(
                me.message.get("superseded").and_then(|v| v.as_bool()),
                Some(true)
            ),
            other => panic!("unexpected non-message entry: {other:?}"),
        }
    }
}

#[test]
fn mark_message_entries_after_anchor_superseded_requires_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("superseded_missing.jsonl");
    write_header(
        &path,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "sid".to_string(),
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();
    let err = mark_message_entries_after_anchor_superseded(&path, "missing");
    assert!(err.is_err());
}

#[test]
fn mark_tool_result_entries_by_tool_call_id_superseded_marks_only_matching_active_results() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tool_result_superseded.jsonl");
    write_header(
        &path,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "sid".to_string(),
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();

    for (id, message) in [
        (
            "assistant-1",
            serde_json::json!({
                "role":"assistant",
                "tool_calls":[{
                    "id":"ask-1",
                    "type":"function",
                    "function":{"name":"ask_question","arguments":"{}"}
                }]
            }),
        ),
        (
            "tool-1",
            serde_json::json!({
                "role":"tool",
                "tool_call_id":"ask-1",
                "content":"[pending]"
            }),
        ),
        (
            "tool-2",
            serde_json::json!({
                "role":"tool",
                "tool_call_id":"other",
                "content":"ok"
            }),
        ),
        (
            "tool-3",
            serde_json::json!({
                "role":"tool",
                "tool_call_id":"ask-1",
                "content":"stale old result",
                "superseded":true
            }),
        ),
    ] {
        append_entry(
            &path,
            &TranscriptEntry::Message(MessageEntry {
                id: Some(id.to_string()),
                parent_id: None,
                timestamp: "2025-01-01T00:00:01.000Z".to_string(),
                message,
            }),
        )
        .unwrap();
    }

    let changed = mark_tool_result_entries_by_tool_call_id_superseded(&path, "ask-1").unwrap();
    assert_eq!(changed, 1);

    let entries = read_entries_tail(&path, 10).unwrap();
    let tool_flags: Vec<(String, Option<bool>)> = entries
        .into_iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Message(me)
                if me.message.get("role").and_then(|value| value.as_str()) == Some("tool") =>
            {
                Some((
                    me.message["tool_call_id"].as_str().unwrap().to_string(),
                    me.message
                        .get("superseded")
                        .and_then(|value| value.as_bool()),
                ))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        tool_flags,
        vec![
            ("ask-1".to_string(), Some(true)),
            ("other".to_string(), None),
            ("ask-1".to_string(), Some(true)),
        ]
    );
}

#[test]
fn mark_trailing_user_messages_superseded_marks_only_active_user_tail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tail_superseded.jsonl");
    write_header(
        &path,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "sid".to_string(),
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();

    for (id, role, content) in [
        ("m1", "user", "q1"),
        ("m2", "assistant", "a1"),
        ("m3", "user", "retry-1"),
        ("m4", "user", "retry-2"),
    ] {
        append_entry(
            &path,
            &TranscriptEntry::Message(MessageEntry {
                id: Some(id.to_string()),
                parent_id: None,
                timestamp: "2025-01-01T00:00:01.000Z".to_string(),
                message: serde_json::json!({"role": role, "content": content}),
            }),
        )
        .unwrap();
    }

    let changed = mark_trailing_user_messages_superseded(&path).unwrap();
    assert_eq!(changed, 2);

    let entries = read_entries_tail(&path, 10).unwrap();
    let flags: Vec<(Option<bool>, Option<bool>)> = entries
        .into_iter()
        .map(|entry| match entry {
            TranscriptEntry::Message(me) => (
                me.message
                    .get("superseded")
                    .and_then(|value| value.as_bool()),
                me.message
                    .get("turn_failed")
                    .and_then(|value| value.as_bool()),
            ),
            other => panic!("unexpected non-message entry: {other:?}"),
        })
        .collect();
    assert_eq!(
        flags,
        vec![
            (None, None),
            (None, None),
            (Some(true), Some(true)),
            (Some(true), Some(true)),
        ]
    );
}

#[test]
fn mark_user_message_entry_superseded_by_id_is_precise_and_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("copy_forward_anchor.jsonl");
    write_header(
        &path,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "sid".to_string(),
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();
    for (id, role) in [
        ("source", "user"),
        ("assistant", "assistant"),
        ("other", "user"),
    ] {
        append_entry(
            &path,
            &TranscriptEntry::Message(MessageEntry {
                id: Some(id.to_string()),
                parent_id: None,
                timestamp: "2025-01-01T00:00:01.000Z".to_string(),
                message: serde_json::json!({"role": role, "content": id}),
            }),
        )
        .unwrap();
    }

    assert_eq!(
        mark_user_message_entry_superseded_by_id(&path, "source").unwrap(),
        1
    );
    let after_first_stamp = std::fs::read(&path).unwrap();
    assert_eq!(
        mark_user_message_entry_superseded_by_id(&path, "source").unwrap(),
        0,
        "repeating the stamp must not rewrite an already stamped source row"
    );
    assert_eq!(std::fs::read(&path).unwrap(), after_first_stamp);

    let entries = read_entries_tail(&path, 8).unwrap();
    let messages = entries
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Message(message) => Some(message),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(messages[0].message["superseded"], true);
    assert_eq!(messages[0].message["turn_failed"], true);
    assert!(messages[1].message.get("superseded").is_none());
    assert!(messages[2].message.get("superseded").is_none());
}

#[test]
fn error_entry_roundtrips_as_type_error() {
    let entry = TranscriptEntry::Error(ErrorEntry {
        id: Some("err-1".to_string()),
        parent_id: None,
        timestamp: "2025-01-01T00:00:01.000Z".to_string(),
        phase: Some("Connect".to_string()),
        provider: Some("openai".to_string()),
        model: Some("gpt-5.4".to_string()),
        api_family: Some("openai-responses".to_string()),
        status_code: Some(403),
        request_id: Some("req-123".to_string()),
        failure_kind: Some("billing".to_string()),
        failure_domain: Some("account".to_string()),
        summary: "API 错误 403 · aigateway.sunmi.com · Request-Id req-123".to_string(),
        detail: "API 错误 403: <html>...</html>".to_string(),
    });

    let json = serde_json::to_value(&entry).unwrap();
    assert_eq!(
        json.get("type").and_then(|value| value.as_str()),
        Some("error")
    );
    assert_eq!(
        json.get("summary").and_then(|value| value.as_str()),
        Some("API 错误 403 · aigateway.sunmi.com · Request-Id req-123")
    );

    let roundtrip: TranscriptEntry = serde_json::from_value(json).unwrap();
    match roundtrip {
        TranscriptEntry::Error(error) => {
            assert_eq!(error.request_id.as_deref(), Some("req-123"));
            assert_eq!(error.status_code, Some(403));
            assert_eq!(error.provider.as_deref(), Some("openai"));
        }
        other => panic!("expected error entry, got {other:?}"),
    }
}
#[test]
fn rewrite_message_text_entries_by_id_updates_target_messages_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rewrite_messages.jsonl");
    write_header(
        &path,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "sid".to_string(),
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();

    for (id, role, content) in [
        ("m1", "assistant", "old-a"),
        ("m2", "tool", "old-b"),
        ("m3", "assistant", "keep"),
    ] {
        append_entry(
            &path,
            &TranscriptEntry::Message(MessageEntry {
                id: Some(id.to_string()),
                parent_id: None,
                timestamp: "2025-01-01T00:00:01.000Z".to_string(),
                message: serde_json::json!({"role": role, "content": content}),
            }),
        )
        .unwrap();
    }

    let changed = rewrite_message_text_entries_by_id(
        &path,
        &[
            MessageTextRewrite {
                message_id: "m1".to_string(),
                new_content: "new-a".to_string(),
            },
            MessageTextRewrite {
                message_id: "m2".to_string(),
                new_content: "new-b".to_string(),
            },
        ],
    )
    .unwrap();
    assert_eq!(changed, 2);

    let entries = read_entries_tail(&path, 10).unwrap();
    let contents: Vec<_> = entries
        .into_iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Message(me) => me
                .message
                .get("content")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            _ => None,
        })
        .collect();
    assert_eq!(contents, vec!["new-a", "new-b", "keep"]);
}
