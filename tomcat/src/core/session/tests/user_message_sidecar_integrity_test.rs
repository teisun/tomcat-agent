use std::fs::OpenOptions;
use std::io::Write;

use crate::core::session::transcript::{
    append_entry, write_header, MessageEntry, SessionHeader, TranscriptEntry,
};
use crate::core::session::user_message_sidecar::{
    ensure_user_message_sidecar, user_message_sidecar_path,
};

#[test]
fn valid_header_with_malformed_data_record_is_rebuilt() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("integrity.jsonl");
    write_header(
        &transcript,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "integrity".to_string(),
            timestamp: "2026-08-10T00:00:00.000Z".to_string(),
            cwd: None,
            project_root: None,
        },
    )
    .unwrap();
    append_entry(
        &transcript,
        &TranscriptEntry::Message(MessageEntry {
            id: Some("u1".to_string()),
            parent_id: None,
            timestamp: "2026-08-10T00:00:01.000Z".to_string(),
            message: serde_json::json!({"role":"user","content":"authoritative input"}),
        }),
    )
    .unwrap();

    let sidecar = ensure_user_message_sidecar(&transcript).unwrap();
    OpenOptions::new()
        .append(true)
        .open(&sidecar)
        .unwrap()
        .write_all(b"{truncated sidecar record\n")
        .unwrap();

    ensure_user_message_sidecar(&transcript).unwrap();
    let lines: Vec<serde_json::Value> =
        std::fs::read_to_string(user_message_sidecar_path(&transcript))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert_eq!(
        lines.len(),
        2,
        "corrupt trailing record must be removed by rebuild"
    );
    assert_eq!(lines[1]["message"]["content"], "authoritative input");
}
#[test]
fn branch_summary_append_rebuilds_sidecar_once_then_preserves_next_fingerprint_hit() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("fingerprint.jsonl");
    write_header(
        &transcript,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "fingerprint".to_string(),
            timestamp: "2026-08-10T00:00:00.000Z".to_string(),
            cwd: None,
            project_root: None,
        },
    )
    .unwrap();
    append_entry(
        &transcript,
        &TranscriptEntry::Message(MessageEntry {
            id: Some("u1".to_string()),
            parent_id: None,
            timestamp: "2026-08-10T00:00:01.000Z".to_string(),
            message: serde_json::json!({"role":"user","content":"normal input"}),
        }),
    )
    .unwrap();
    let sidecar = ensure_user_message_sidecar(&transcript).unwrap();

    append_entry(
        &transcript,
        &TranscriptEntry::Message(MessageEntry {
            id: Some("boundary".to_string()),
            parent_id: None,
            timestamp: "2026-08-10T00:00:02.000Z".to_string(),
            message: serde_json::json!({"role":"assistant","content":"boundary summary"}),
        }),
    )
    .unwrap();
    ensure_user_message_sidecar(&transcript).unwrap();
    let refreshed_bytes = std::fs::read(&sidecar).unwrap();

    ensure_user_message_sidecar(&transcript).unwrap();
    assert_eq!(
        std::fs::read(sidecar).unwrap(),
        refreshed_bytes,
        "写入 branch summary 后重建的 sidecar 必须供下一次 ensure 命中缓存"
    );
}
