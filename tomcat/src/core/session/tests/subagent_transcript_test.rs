use std::sync::Arc;

use crate::core::agent_loop::SubagentType;
use crate::core::session::subagent_transcript::{
    open_subagent_transcript, subagent_transcript_path, JsonlFileAppendSink,
};
use crate::core::session::{MessageAppendSink, SessionHeader, TranscriptEntry};

#[test]
fn open_subagent_transcript_returns_none_for_empty_trail_dir() {
    let sink = open_subagent_transcript("", "child-1", SubagentType::Verifier, "model", "parent");
    assert!(sink.is_none());
}

#[test]
fn jsonl_file_append_sink_writes_header_meta_and_message_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("subagent-sessions").join("child-1.jsonl");
    let sink = JsonlFileAppendSink::new(
        path.clone(),
        "child-1",
        SubagentType::Verifier,
        "deepseek-v4",
        "parent-1",
    );

    let first_id = sink
        .append_message(serde_json::json!({
            "role": "assistant",
            "content": "first"
        }))
        .unwrap();
    let forced_id = sink
        .append_message_with_id(
            serde_json::json!({
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "ok"
            }),
            "forced-1",
        )
        .unwrap();

    assert!(!first_id.is_empty());
    assert_eq!(forced_id, "forced-1");

    let lines: Vec<_> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(lines.len(), 4);

    let header: SessionHeader = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(header.id, "child-1");
    assert_eq!(header.version, Some(3));

    let meta: TranscriptEntry = serde_json::from_str(&lines[1]).unwrap();
    match meta {
        TranscriptEntry::Custom(entry) => {
            assert_eq!(
                entry.extra["event"].as_str(),
                Some("subagent.transcript.meta")
            );
            assert_eq!(entry.extra["child_session_id"].as_str(), Some("child-1"));
            assert_eq!(entry.extra["parent_session_id"].as_str(), Some("parent-1"));
            assert_eq!(entry.extra["subagent_type"].as_str(), Some("verifier"));
            assert_eq!(entry.extra["model"].as_str(), Some("deepseek-v4"));
        }
        other => panic!("expected custom meta entry, got {other:?}"),
    }

    let first: TranscriptEntry = serde_json::from_str(&lines[2]).unwrap();
    match first {
        TranscriptEntry::Message(entry) => {
            assert_eq!(entry.id.as_deref(), Some(first_id.as_str()));
            assert_eq!(entry.message["role"].as_str(), Some("assistant"));
        }
        other => panic!("expected first message entry, got {other:?}"),
    }

    let second: TranscriptEntry = serde_json::from_str(&lines[3]).unwrap();
    match second {
        TranscriptEntry::Message(entry) => {
            assert_eq!(entry.id.as_deref(), Some("forced-1"));
            assert_eq!(entry.message["role"].as_str(), Some("tool"));
        }
        other => panic!("expected second message entry, got {other:?}"),
    }
}

#[test]
fn jsonl_file_append_sink_serializes_concurrent_appends() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("subagent-sessions").join("child-2.jsonl");
    let sink = Arc::new(JsonlFileAppendSink::new(
        path.clone(),
        "child-2",
        SubagentType::PlanReviewer,
        "gpt-5",
        "parent-2",
    ));

    let mut workers = Vec::new();
    for idx in 0..8 {
        let sink = Arc::clone(&sink);
        workers.push(std::thread::spawn(move || {
            sink.append_message(serde_json::json!({
                "role": "assistant",
                "content": format!("msg-{idx}")
            }))
            .unwrap()
        }));
    }

    let mut ids = Vec::new();
    for worker in workers {
        ids.push(worker.join().unwrap());
    }
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 8);

    let lines: Vec<_> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(lines.len(), 10);
}

#[test]
fn jsonl_file_append_sink_is_best_effort_when_path_is_unwritable() {
    let dir = tempfile::tempdir().unwrap();
    let bad_root = dir.path().join("not-a-dir");
    std::fs::write(&bad_root, "x").unwrap();

    let sink = open_subagent_transcript(
        bad_root.to_str().unwrap(),
        "child-3",
        SubagentType::Verifier,
        "model",
        "parent-3",
    )
    .unwrap();
    let id = sink
        .append_message(serde_json::json!({
            "role": "assistant",
            "content": "still ok"
        }))
        .unwrap();

    assert!(!id.is_empty());
    let path = subagent_transcript_path(bad_root.to_str().unwrap(), "child-3").unwrap();
    assert!(!path.exists());
}
