use std::fs;

use super::*;
use crate::core::session::transcript::{
    append_entry, append_line, mark_user_message_entry_superseded_by_id, write_header,
    MessageEntry, SessionHeader, TranscriptEntry,
};

fn header() -> SessionHeader {
    SessionHeader {
        r#type: "session".to_string(),
        version: Some(3),
        id: "sidecar-session".to_string(),
        timestamp: "2026-08-10T00:00:00.000Z".to_string(),
        cwd: None,
        project_root: None,
    }
}

fn append_message(path: &std::path::Path, id: &str, message: serde_json::Value) {
    append_entry(
        path,
        &TranscriptEntry::Message(MessageEntry {
            id: Some(id.to_string()),
            parent_id: None,
            timestamp: format!("2026-08-10T00:00:{id}Z"),
            message,
        }),
    )
    .unwrap();
}

fn sidecar_lines(path: &std::path::Path) -> Vec<serde_json::Value> {
    fs::read_to_string(user_message_sidecar_path(path))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn rebuild_keeps_only_active_normal_user_messages_with_original_json() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("s.jsonl");
    write_header(&transcript, &header()).unwrap();

    let structured = serde_json::json!([
        {"type":"input_text","text":"保留文本"},
        {"type":"input_reference","path":"src/lib.rs","label":"lib.rs"},
        {"type":"input_image","image_url":"data:image/png;base64,abc"}
    ]);
    append_message(
        &transcript,
        "legacy",
        serde_json::json!({"role":"user","content":"legacy normal"}),
    );
    append_message(
        &transcript,
        "structured",
        serde_json::json!({"role":"user","kind":"normal","content":structured}),
    );
    for (id, kind) in [
        ("signal", "signal"),
        ("plan", "plan_build"),
        ("steer", "steering"),
        ("nudge", "nudge"),
        ("summary", "compaction_summary"),
    ] {
        append_message(
            &transcript,
            id,
            serde_json::json!({"role":"user","kind":kind,"content":format!("skip-{id}")}),
        );
    }
    append_message(
        &transcript,
        "assistant",
        serde_json::json!({"role":"assistant","content":"skip assistant"}),
    );
    append_message(
        &transcript,
        "superseded",
        serde_json::json!({"role":"user","kind":"normal","superseded":true,"content":"skip stale"}),
    );

    ensure_user_message_sidecar(&transcript).unwrap();
    let lines = sidecar_lines(&transcript);
    assert_eq!(lines.len(), 3, "header + two active Normal users");
    assert_eq!(lines[0]["type"], SIDECAR_TYPE);
    assert_eq!(lines[1]["id"], "legacy");
    assert_eq!(lines[2]["id"], "structured");
    assert_eq!(lines[2]["message"]["content"], structured);
}

#[test]
fn recent_user_message_texts_reads_the_sidecar_tail_and_keeps_multipart_text() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("recent.jsonl");
    write_header(&transcript, &header()).unwrap();
    append_message(
        &transcript,
        "u1",
        serde_json::json!({"role":"user","content":"first"}),
    );
    append_message(
        &transcript,
        "u2",
        serde_json::json!({
            "role":"user",
            "kind":"normal",
            "content":[
                {"type":"input_text","text":"second "},
                {"type":"input_reference","path":"src/lib.rs","label":"lib.rs"},
                {"type":"input_text","text":"message"}
            ]
        }),
    );
    append_message(
        &transcript,
        "u3",
        serde_json::json!({"role":"user","content":"third"}),
    );
    ensure_user_message_sidecar(&transcript).unwrap();

    let path = user_message_sidecar_path(&transcript);
    assert_eq!(
        recent_user_message_texts(&path, 2).unwrap(),
        ["second message", "third"],
        "the tail reader must ignore the header, retain authored multipart text, and return chronological order"
    );
}

#[test]
fn valid_fingerprint_reuses_existing_sidecar_then_new_user_rebuilds_it() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("cache.jsonl");
    write_header(&transcript, &header()).unwrap();
    append_message(
        &transcript,
        "u1",
        serde_json::json!({"role":"user","content":"first"}),
    );

    let path = ensure_user_message_sidecar(&transcript).unwrap();
    assert_eq!(
        read_sidecar_fingerprint(&path).unwrap(),
        Some(transcript_fingerprint(&transcript).unwrap()),
        "a just-written sidecar header must deserialize and match the transcript"
    );
    let before = fs::read(&path).unwrap();
    ensure_user_message_sidecar(&transcript).unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        before,
        "fingerprint hit must not rewrite sidecar"
    );

    append_message(
        &transcript,
        "u2",
        serde_json::json!({"role":"user","content":"second"}),
    );
    ensure_user_message_sidecar(&transcript).unwrap();
    let sidecar = sidecar_lines(&transcript);
    let ids: Vec<_> = sidecar
        .iter()
        .skip(1)
        .map(|line| line["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["u1", "u2"]);
}

#[test]
fn rebuild_reflects_supersede_and_repairs_corrupt_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("retry.jsonl");
    write_header(&transcript, &header()).unwrap();
    append_message(
        &transcript,
        "old",
        serde_json::json!({"role":"user","kind":"normal","content":"retry me"}),
    );
    append_message(
        &transcript,
        "new",
        serde_json::json!({"role":"user","kind":"normal","content":"new copy"}),
    );
    ensure_user_message_sidecar(&transcript).unwrap();

    mark_user_message_entry_superseded_by_id(&transcript, "old").unwrap();
    fs::write(user_message_sidecar_path(&transcript), b"not json\n").unwrap();
    ensure_user_message_sidecar(&transcript).unwrap();

    let lines = sidecar_lines(&transcript);
    assert_eq!(lines.len(), 2, "header + active copy");
    assert_eq!(lines[1]["id"], "new");
}

#[test]
fn rebuild_skips_corrupt_transcript_lines() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("corrupt.jsonl");
    write_header(&transcript, &header()).unwrap();
    append_line(&transcript, "not valid json").unwrap();
    append_message(
        &transcript,
        "u1",
        serde_json::json!({"role":"user","content":"still present"}),
    );

    ensure_user_message_sidecar(&transcript).unwrap();
    let lines = sidecar_lines(&transcript);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["id"], "u1");
}

#[test]
fn concurrent_rebuilds_share_a_path_lock_and_leave_valid_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("concurrent.jsonl");
    write_header(&transcript, &header()).unwrap();
    append_message(
        &transcript,
        "u1",
        serde_json::json!({"role":"user","content":"concurrent input"}),
    );

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let path = transcript.clone();
        let barrier = std::sync::Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            ensure_user_message_sidecar(&path).unwrap()
        }));
    }
    barrier.wait();
    let paths: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().expect("sidecar worker panicked"))
        .collect();

    assert_eq!(paths[0], paths[1]);
    let lines = sidecar_lines(&transcript);
    assert_eq!(lines.len(), 2, "header + one Normal user line");
    assert_eq!(lines[1]["id"], "u1");
}

#[test]
fn appending_transcript_messages_does_not_materialize_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("send-path.jsonl");
    write_header(&transcript, &header()).unwrap();
    append_message(
        &transcript,
        "u1",
        serde_json::json!({"role":"user","content":"send path"}),
    );

    assert!(
        !user_message_sidecar_path(&transcript).exists(),
        "发送路径只能写 transcript，sidecar 必须等到 compaction 按需重建"
    );
}

#[tokio::test]
async fn async_ensure_degrades_when_sidecar_path_cannot_be_read_or_rebuilt() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("degrade.jsonl");
    write_header(&transcript, &header()).unwrap();
    append_message(
        &transcript,
        "u1",
        serde_json::json!({"role":"user","content":"input"}),
    );
    std::fs::create_dir(user_message_sidecar_path(&transcript)).unwrap();

    assert_eq!(ensure_user_message_sidecar_current(&transcript).await, None);
}

#[test]
fn missing_transcript_path_is_rejected_for_caller_to_gracefully_degrade() {
    let err = ensure_user_message_sidecar(std::path::Path::new("")).unwrap_err();
    assert!(err.to_string().contains("缺少 transcript 路径"));
}
