//! # `SessionManager` 与附件 blob store 的联动测试
//!
//! 覆盖三条只有在 `SessionManager` 这一层才能验证的行为：
//!
//! - `discard_legacy_draft_dir`：升级后一次性丢弃旧版本的后端草稿目录
//! - `collect_live_blob_shas`：一次扫描所有会话，构建 GC 在用名单
//! - `delete_session`：删会话时释放租约并触发一次标记清扫

use super::super::*;
use super::mocks::temp_sessions_dir;
use crate::core::session::tool_display_sidecar::{append_tool_display, tool_display_sidecar_path};
use crate::core::session::user_message_sidecar::user_message_sidecar_path;
use crate::core::tools::primitive::{DiffTag, FileDiffLine};
use crate::infra::events::ToolDisplay;
use std::io::Write;

fn fresh_dir() -> std::path::PathBuf {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_transcript_references(dir: &std::path::Path, session_id: &str, shas: &[String]) {
    let lines = shas
        .iter()
        .map(|sha| format!(r#"{{"message":{{"content":[{{"blob_sha":"{sha}"}}]}}}}"#))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(
        dir.join(format!("{session_id}.jsonl")),
        format!("{lines}\n"),
    )
    .unwrap();
}

fn file_display(text: &str) -> ToolDisplay {
    ToolDisplay::File {
        file: "src/lib.rs".to_string(),
        added: Some(1),
        removed: Some(0),
        diff: Some(vec![FileDiffLine {
            tag: DiffTag::Add,
            old_line: None,
            new_line: Some(1),
            skipped_lines: None,
            text: text.to_string(),
        }]),
        diff_truncated: false,
        expired: false,
    }
}

#[test]
fn discard_legacy_draft_dir_removes_the_old_backend_drafts() {
    let dir = fresh_dir();
    let legacy = dir.join("drafts");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("sid_old.json"), b"{\"text\":\"unsent\"}").unwrap();

    let mgr = SessionManager::new(dir.clone());
    mgr.discard_legacy_draft_dir();

    assert!(!legacy.exists(), "旧草稿目录必须被整体删除，不做迁移");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn discard_legacy_draft_dir_is_a_no_op_when_there_is_nothing_to_discard() {
    // 绝大多数启动都会走这条路，它必须便宜且安静。
    let dir = fresh_dir();
    let mgr = SessionManager::new(dir.clone());
    mgr.discard_legacy_draft_dir();
    mgr.discard_legacy_draft_dir();
    assert!(dir.exists(), "不得误伤 sessions 目录本身");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tool_owned_media_gc_keeps_subagent_only_refs_and_namespaces_cached_ids() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("sessions");
    let children = temp.path().join("subagent-sessions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&children).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let main = mgr.attachment_store().put(b"main").unwrap();
    let child = mgr.attachment_store().put(b"child").unwrap();
    write_transcript_references(&dir, "same", std::slice::from_ref(&main));
    write_transcript_references(&children, "same", std::slice::from_ref(&child));
    for sha in [&main, &child] {
        std::fs::File::open(mgr.attachment_store().blobs_dir().join(sha))
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
            .unwrap();
    }
    let first = mgr.run_incremental_attachment_housekeeping().unwrap();
    assert_eq!(first.live_blob_references.transcripts_scanned, 2);
    assert!(first.live_blob_references.shas.contains(&child));
    assert!(first.live_blob_references.shas.contains(&main));
    let second = mgr.run_incremental_attachment_housekeeping().unwrap();
    assert_eq!(second.live_blob_references.transcripts_scanned, 0);
    assert_eq!(
        first.live_blob_references.shas,
        second.live_blob_references.shas
    );
    mgr.attachment_store()
        .sweep_orphan_blobs(&second.live_blob_references.shas, std::time::Duration::ZERO)
        .unwrap();
    assert!(mgr.attachment_store().exists(&child));
    std::fs::remove_file(children.join("same.jsonl")).unwrap();
    let last = mgr.run_incremental_attachment_housekeeping().unwrap();
    assert!(!last.live_blob_references.shas.contains(&child));
    assert!(last.live_blob_references.shas.contains(&main));
    let ledger: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join(".housekeeping.json")).unwrap()).unwrap();
    assert!(ledger["sessions"].get("subagent/same").is_none());
    assert!(ledger["sessions"].get("same").is_some());
}

#[test]
fn collect_live_blob_shas_scans_each_transcript_once_and_ignores_sidecars() {
    let dir = fresh_dir();
    let mgr = SessionManager::new(dir.clone());
    let shared_shas = (b'a'..=b'e')
        .map(|byte| char::from(byte).to_string().repeat(64))
        .collect::<Vec<_>>();
    let orphan_sha = "f".repeat(64);

    for session_id in ["sid_one", "sid_two", "sid_three"] {
        let lines = shared_shas
            .iter()
            .map(|sha| format!("{{\"role\":\"user\",\"content\":[{{\"blob_sha\":\"{sha}\"}}]}}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(
            dir.join(format!("{session_id}.jsonl")),
            format!("{lines}\n"),
        )
        .unwrap();
    }
    let pending_sha = mgr.attachment_store().put(b"pending image bytes").unwrap();
    mgr.attachment_store()
        .mark_pending("draft", &pending_sha)
        .unwrap();
    std::fs::write(
        dir.join("sid_one.tool_display.jsonl"),
        format!("{{\"blob_sha\":\"{orphan_sha}\"}}\n"),
    )
    .unwrap();

    let live = mgr.collect_live_blob_shas().unwrap();
    for sha in &shared_shas {
        assert!(live.shas.contains(sha));
    }
    assert!(live.shas.contains(&pending_sha));
    assert!(!live.shas.contains(&orphan_sha), "sidecar 不是 GC 的事实源");
    let expected_bytes: u64 = ["sid_one", "sid_two", "sid_three"]
        .into_iter()
        .map(|session_id| {
            std::fs::metadata(dir.join(format!("{session_id}.jsonl")))
                .unwrap()
                .len()
        })
        .sum();
    assert_eq!(live.bytes_scanned, expected_bytes);
    assert_eq!(
        live.transcripts_scanned, 3,
        "one GC mark pass must open each of the three main transcripts exactly once"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn incremental_housekeeping_reuses_unchanged_transcript_and_sidecar_records() {
    let dir = fresh_dir();
    let mgr = SessionManager::new(dir.clone());
    let shas = (b'a'..=b'e')
        .map(|byte| char::from(byte).to_string().repeat(64))
        .collect::<Vec<_>>();
    let transcripts = ["sid_one", "sid_two", "sid_three"];
    for session_id in transcripts {
        write_transcript_references(&dir, session_id, &shas);
        append_tool_display(
            &dir.join(format!("{session_id}.jsonl")),
            "fresh-display",
            &chrono::Utc::now().to_rfc3339(),
            &file_display("fresh"),
        )
        .unwrap();
    }

    let first = mgr.run_incremental_attachment_housekeeping().unwrap();
    assert_eq!(first.live_blob_references.transcripts_scanned, 3);
    let sidecar_mtimes = transcripts
        .iter()
        .map(|session_id| {
            std::fs::metadata(tool_display_sidecar_path(
                &dir.join(format!("{session_id}.jsonl")),
            ))
            .unwrap()
            .modified()
            .unwrap()
        })
        .collect::<Vec<_>>();

    let second = mgr.run_incremental_attachment_housekeeping().unwrap();
    assert_eq!(second.live_blob_references.bytes_scanned, 0);
    assert_eq!(second.live_blob_references.transcripts_scanned, 0);
    assert_eq!(
        second.live_blob_references.shas,
        first.live_blob_references.shas
    );
    for (index, session_id) in transcripts.iter().enumerate() {
        assert_eq!(
            std::fs::metadata(tool_display_sidecar_path(
                &dir.join(format!("{session_id}.jsonl")),
            ))
            .unwrap()
            .modified()
            .unwrap(),
            sidecar_mtimes[index],
            "unchanged sidecars must not be rewritten on a second startup"
        );
    }
    assert!(dir.join(".housekeeping.json").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn incremental_housekeeping_rescans_only_the_transcript_that_changed() {
    let dir = fresh_dir();
    let mgr = SessionManager::new(dir.clone());
    let old_sha = "a".repeat(64);
    let new_sha = "b".repeat(64);
    for session_id in ["sid_one", "sid_two", "sid_three"] {
        write_transcript_references(&dir, session_id, std::slice::from_ref(&old_sha));
    }
    mgr.run_incremental_attachment_housekeeping().unwrap();

    let changed_path = dir.join("sid_two.jsonl");
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&changed_path)
            .unwrap(),
        r#"{{"message":{{"content":[{{"blob_sha":"{new_sha}"}}]}}}}"#
    )
    .unwrap();

    let second = mgr.run_incremental_attachment_housekeeping().unwrap();
    assert_eq!(second.live_blob_references.transcripts_scanned, 1);
    assert_eq!(
        second.live_blob_references.bytes_scanned,
        std::fs::metadata(&changed_path).unwrap().len()
    );
    assert!(second.live_blob_references.shas.contains(&old_sha));
    assert!(second.live_blob_references.shas.contains(&new_sha));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn malformed_housekeeping_ledger_falls_back_to_a_full_scan_and_rebuilds_it() {
    let dir = fresh_dir();
    let mgr = SessionManager::new(dir.clone());
    let sha = "a".repeat(64);
    for session_id in ["sid_one", "sid_two"] {
        write_transcript_references(&dir, session_id, std::slice::from_ref(&sha));
    }
    mgr.run_incremental_attachment_housekeeping().unwrap();
    std::fs::write(dir.join(".housekeeping.json"), b"{not-json}").unwrap();

    let rebuilt = mgr.run_incremental_attachment_housekeeping().unwrap();
    assert_eq!(rebuilt.live_blob_references.transcripts_scanned, 2);
    assert!(rebuilt.live_blob_references.shas.contains(&sha));
    let ledger = std::fs::read_to_string(dir.join(".housekeeping.json")).unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&ledger).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn incremental_housekeeping_prunes_deleted_session_entries_from_its_ledger() {
    let dir = fresh_dir();
    let mgr = SessionManager::new(dir.clone());
    let sha = "a".repeat(64);
    for session_id in ["sid_kept", "sid_deleted"] {
        write_transcript_references(&dir, session_id, std::slice::from_ref(&sha));
    }
    mgr.run_incremental_attachment_housekeeping().unwrap();
    std::fs::remove_file(dir.join("sid_deleted.jsonl")).unwrap();

    mgr.run_incremental_attachment_housekeeping().unwrap();

    let ledger: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join(".housekeeping.json")).unwrap()).unwrap();
    assert!(
        ledger["sessions"].get("sid_deleted").is_none(),
        "a deleted transcript must not leave a stale cache entry"
    );
    assert!(ledger["sessions"].get("sid_kept").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn incremental_housekeeping_compacts_only_the_sidecar_that_becomes_due() {
    let dir = fresh_dir();
    let mgr = SessionManager::new(dir.clone());
    let first = dir.join("sid_one.jsonl");
    let second = dir.join("sid_two.jsonl");
    std::fs::write(&first, "").unwrap();
    std::fs::write(&second, "").unwrap();
    let now = chrono::Utc::now();
    append_tool_display(
        &first,
        "fresh-one",
        &now.to_rfc3339(),
        &file_display("fresh one"),
    )
    .unwrap();
    append_tool_display(
        &second,
        "fresh-two",
        &now.to_rfc3339(),
        &file_display("fresh two"),
    )
    .unwrap();
    mgr.run_incremental_attachment_housekeeping().unwrap();
    let untouched_sidecar = tool_display_sidecar_path(&second);
    let untouched_mtime = std::fs::metadata(&untouched_sidecar)
        .unwrap()
        .modified()
        .unwrap();

    append_tool_display(
        &first,
        "expired-one",
        &(now - chrono::Duration::days(8)).to_rfc3339(),
        &file_display("expired one"),
    )
    .unwrap();
    let report = mgr.run_incremental_attachment_housekeeping().unwrap();

    assert_eq!(report.compacted_displays, 1);
    assert!(std::fs::read_to_string(tool_display_sidecar_path(&first))
        .unwrap()
        .contains(r#""expired":true"#));
    assert_eq!(
        std::fs::metadata(&untouched_sidecar)
            .unwrap()
            .modified()
            .unwrap(),
        untouched_mtime,
        "a fresh sidecar must not be rewritten because another sidecar was due"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn malformed_transcript_row_is_skipped_without_stopping_blob_gc() {
    let dir = fresh_dir();
    let mgr = SessionManager::new(dir.clone());
    let before_bad_row = mgr
        .attachment_store()
        .put(b"referenced before malformed row")
        .unwrap();
    let after_bad_row = mgr
        .attachment_store()
        .put(b"referenced after malformed row")
        .unwrap();
    let orphan = mgr
        .attachment_store()
        .put(b"unreferenced image bytes")
        .unwrap();
    std::fs::write(
        dir.join("sid_corrupt.jsonl"),
        format!(
            concat!(
                r#"{{"message":{{"content":[{{"blob_sha":"{before_bad_row}"}}]}}}}"#,
                "\n",
                r#"{{"message":{{"content":[{{"blob_sha":"{after_bad_row}"}}]}}}}"#,
                "\n"
            ),
            before_bad_row = before_bad_row,
            after_bad_row = after_bad_row,
        ),
    )
    .unwrap();
    let corrupt_path = dir.join("sid_corrupt.jsonl");
    let valid_rows = std::fs::read_to_string(&corrupt_path).unwrap();
    let mut rows = valid_rows.lines();
    let first = rows.next().unwrap();
    let second = rows.next().unwrap();
    std::fs::write(&corrupt_path, format!("{first}\n{{not-json}}\n{second}\n")).unwrap();

    let orphan_path = mgr.attachment_store().blobs_dir().join(&orphan);
    let file = std::fs::File::options()
        .write(true)
        .open(&orphan_path)
        .unwrap();
    file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3601))
        .unwrap();

    let live = mgr.collect_live_blob_shas().unwrap();
    assert!(live.shas.contains(&before_bad_row));
    assert!(live.shas.contains(&after_bad_row));
    assert!(
        !live.shas.contains(&orphan),
        "malformed rows cannot manufacture a live blob reference"
    );
    mgr.attachment_store()
        .sweep_orphan_blobs(&live.shas, std::time::Duration::from_secs(3600))
        .unwrap();

    assert!(
        mgr.attachment_store().exists(&before_bad_row),
        "bad row before the valid reference must not stop collection"
    );
    assert!(
        mgr.attachment_store().exists(&after_bad_row),
        "bad row must be skipped so later valid references still enter the live set"
    );
    assert!(!mgr.attachment_store().exists(&orphan));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn refreshing_pending_leases_drops_only_expired_lease_references() {
    let dir = fresh_dir();
    let mgr = SessionManager::new(dir.clone());
    let transcript_sha = mgr.attachment_store().put(b"durable image bytes").unwrap();
    let expired_lease_sha = mgr.attachment_store().put(b"draft image bytes").unwrap();
    std::fs::write(
        dir.join("sid_history.jsonl"),
        format!(r#"{{"message":{{"content":[{{"blob_sha":"{transcript_sha}"}}]}}}}"#),
    )
    .unwrap();
    mgr.attachment_store()
        .mark_pending("expired-draft", &expired_lease_sha)
        .unwrap();

    let mut live = mgr.collect_live_blob_shas().unwrap();
    assert!(live.shas.contains(&transcript_sha));
    assert!(live.shas.contains(&expired_lease_sha));

    mgr.attachment_store()
        .clear_session("expired-draft")
        .unwrap();
    live.refresh_pending_blob_shas(mgr.attachment_store().collect_pending_blob_shas().unwrap());

    assert!(
        live.shas.contains(&transcript_sha),
        "刷新租约不能丢失 durable transcript 引用"
    );
    assert!(
        !live.shas.contains(&expired_lease_sha),
        "已释放的 pending lease 不能再延长 orphan grace"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_session_releases_unsent_attachment_but_respects_orphan_grace() {
    let dir = fresh_dir();
    let mgr = SessionManager::new(dir.clone());
    let entry = mgr.create_session(mgr.current_session_key(), None).unwrap();

    let store = mgr.attachment_store();
    let sha = store.put(b"unsent image bytes").unwrap();
    store.mark_pending(&entry.session_id, &sha).unwrap();
    std::fs::write(
        user_message_sidecar_path(&mgr.transcript_path(&entry.session_id)),
        format!("{{\"message\":{{\"content\":[{{\"blobSha\":\"{sha}\"}}]}}}}\n"),
    )
    .unwrap();

    mgr.delete_session(&entry.session_id).unwrap();

    assert!(
        store.exists(&sha),
        "会话删除后仍需等孤儿宽限期，避免并发写入与清扫竞态"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_session_keeps_bytes_another_session_still_references() {
    let dir = fresh_dir();
    let mgr = SessionManager::new(dir.clone());
    let doomed = mgr.create_session(mgr.current_session_key(), None).unwrap();

    let store = mgr.attachment_store();
    let sha = store.put(b"shared image bytes").unwrap();
    store.mark_pending(&doomed.session_id, &sha).unwrap();
    // 另一个会话已经把这张图发出去了，字节写进了它的 transcript。
    std::fs::write(
        dir.join("sid_keeper.jsonl"),
        format!("{{\"role\":\"user\",\"content\":[{{\"blob_sha\":\"{sha}\"}}]}}\n"),
    )
    .unwrap();

    mgr.delete_session(&doomed.session_id).unwrap();

    assert!(
        store.exists(&sha),
        "别的会话还在引用这份字节，删会话不得把它带走"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
