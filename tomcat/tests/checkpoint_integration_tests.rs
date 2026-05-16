use std::fs;
use std::path::PathBuf;

use serde_json::json;
use tomcat::core::session::{mark_message_entries_after_anchor_superseded, read_entries_tail};
use tomcat::{
    init_context_state, CheckpointId, CheckpointKind, CheckpointRecordRequest, CheckpointStore,
    ContextConfig, RestoreOptions, SessionManager, ShadowGitStore, TranscriptEntry,
};

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

struct Fixture {
    _root: tempfile::TempDir,
    worktree: PathBuf,
    trail: PathBuf,
    session: SessionManager,
    session_key: String,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("workspace");
    let trail = root.path().join("trail");
    let sessions = root.path().join("sessions");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&trail).unwrap();
    fs::create_dir_all(&sessions).unwrap();

    let session = SessionManager::new(sessions);
    let session_key = session.current_session_key().to_string();
    session.create_session(&session_key, None).unwrap();

    Fixture {
        _root: root,
        worktree,
        trail,
        session,
        session_key,
    }
}

fn record_checkpoint(
    store: &dyn CheckpointStore,
    session_key: &str,
    turn_id: &str,
    kind: CheckpointKind,
    message_anchor: Option<String>,
) -> CheckpointId {
    store
        .record(CheckpointRecordRequest {
            session_id: session_key.to_string(),
            turn_id: turn_id.to_string(),
            kind,
            message_anchor,
            notes: None,
        })
        .unwrap()
}

#[test]
fn restore_path_checkout_only_touches_selected_file() {
    if !git_available() {
        return;
    }
    let fx = fixture();
    let store = ShadowGitStore::new(fx.trail.clone(), fx.worktree.clone());
    fs::write(fx.worktree.join("a.txt"), "v1-a").unwrap();
    fs::write(fx.worktree.join("b.txt"), "v1-b").unwrap();
    let id = record_checkpoint(
        &store,
        &fx.session_key,
        "turn-1",
        CheckpointKind::TurnEnd,
        None,
    );

    fs::write(fx.worktree.join("a.txt"), "v2-a").unwrap();
    fs::write(fx.worktree.join("b.txt"), "v2-b").unwrap();

    store
        .restore(
            &id,
            RestoreOptions {
                paths: vec![PathBuf::from("a.txt")],
                dry_run: false,
            },
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(fx.worktree.join("a.txt")).unwrap(),
        "v1-a"
    );
    assert_eq!(
        fs::read_to_string(fx.worktree.join("b.txt")).unwrap(),
        "v2-b"
    );
}

#[test]
fn restore_turn_end_supersedes_transcript_and_updates_last_checkpoint() {
    if !git_available() {
        return;
    }
    let fx = fixture();
    let store = ShadowGitStore::new(fx.trail.clone(), fx.worktree.clone());

    let m1 = fx
        .session
        .append_message(json!({"role":"user","content":"q1"}))
        .unwrap();
    let m2 = fx
        .session
        .append_message(json!({"role":"assistant","content":"a1"}))
        .unwrap();

    fs::write(fx.worktree.join("note.txt"), "good").unwrap();
    let id = record_checkpoint(
        &store,
        &fx.session_key,
        "turn-1",
        CheckpointKind::TurnEnd,
        Some(m2.clone()),
    );

    let m3 = fx
        .session
        .append_message(json!({"role":"user","content":"q2"}))
        .unwrap();
    let m4 = fx
        .session
        .append_message(json!({"role":"assistant","content":"a2"}))
        .unwrap();
    fs::write(fx.worktree.join("note.txt"), "bad").unwrap();

    store
        .restore(&id, RestoreOptions::default())
        .expect("restore should succeed");

    let transcript_path = fx
        .session
        .current_transcript_path()
        .unwrap()
        .expect("transcript path");
    mark_message_entries_after_anchor_superseded(&transcript_path, &m2).unwrap();
    fx.session
        .append_custom_entry(json!({
            "customType": "checkpoint.restore",
            "checkpointId": id.to_string(),
            "checkpointKind": "turn_end",
            "anchorMessageId": m2,
            "restoredPaths": ["note.txt"],
        }))
        .unwrap();
    fx.session
        .update_session(&fx.session_key, |entry| {
            entry.last_checkpoint_id = Some(id.to_string());
        })
        .unwrap();

    assert_eq!(
        fs::read_to_string(fx.worktree.join("note.txt")).unwrap(),
        "good"
    );

    let entries = read_entries_tail(&transcript_path, 16).unwrap();
    let superseded_ids: Vec<String> = entries
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Message(me)
                if me
                    .message
                    .get("superseded")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false) =>
            {
                me.id.clone()
            }
            _ => None,
        })
        .collect();
    assert!(superseded_ids.contains(&m3));
    assert!(superseded_ids.contains(&m4));
    assert!(!superseded_ids.contains(&m1));
    assert!(!superseded_ids.contains(&m2));
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry, TranscriptEntry::Custom(_))),
        "TurnEnd restore 应追加 checkpoint.restore custom 条目"
    );

    let entry = fx.session.get_session(&fx.session_key).unwrap().unwrap();
    assert_eq!(entry.last_checkpoint_id.as_deref(), Some(id.as_str()));
}

#[test]
fn restore_manual_checkpoint_keeps_transcript_live() {
    if !git_available() {
        return;
    }
    let fx = fixture();
    let store = ShadowGitStore::new(fx.trail.clone(), fx.worktree.clone());

    let _m1 = fx
        .session
        .append_message(json!({"role":"user","content":"q1"}))
        .unwrap();
    let _m2 = fx
        .session
        .append_message(json!({"role":"assistant","content":"a1"}))
        .unwrap();

    fs::write(fx.worktree.join("note.txt"), "stable").unwrap();
    let id = record_checkpoint(
        &store,
        &fx.session_key,
        "turn-manual",
        CheckpointKind::Manual {
            label: "pre-rollback".to_string(),
        },
        None,
    );

    let m3 = fx
        .session
        .append_message(json!({"role":"user","content":"q2"}))
        .unwrap();
    fs::write(fx.worktree.join("note.txt"), "broken").unwrap();

    store.restore(&id, RestoreOptions::default()).unwrap();
    assert_eq!(
        fs::read_to_string(fx.worktree.join("note.txt")).unwrap(),
        "stable"
    );

    let transcript_path = fx
        .session
        .current_transcript_path()
        .unwrap()
        .expect("transcript path");
    let entries = read_entries_tail(&transcript_path, 16).unwrap();
    let superseded_ids: Vec<String> = entries
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Message(me)
                if me
                    .message
                    .get("superseded")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false) =>
            {
                me.id.clone()
            }
            _ => None,
        })
        .collect();
    assert!(
        superseded_ids.is_empty(),
        "Manual restore 不应把 transcript 标记为 superseded"
    );
    assert!(
        entries
            .iter()
            .all(|entry| !matches!(entry, TranscriptEntry::Custom(_))),
        "Manual restore 不应追加 checkpoint.restore custom 条目"
    );

    let entry = fx.session.get_session(&fx.session_key).unwrap().unwrap();
    assert_eq!(entry.last_checkpoint_id, None);
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            TranscriptEntry::Message(me) if me.id.as_deref() == Some(m3.as_str())
        )),
        "Manual restore 后后续对话仍应保持有效"
    );
}

#[test]
fn resume_after_interrupt_keeps_partial_assistant() {
    if !git_available() {
        return;
    }
    let fx = fixture();
    let store = ShadowGitStore::new(fx.trail.clone(), fx.worktree.clone());

    let _m1 = fx
        .session
        .append_message(json!({"role":"user","content":"hello"}))
        .unwrap();
    let m2 = fx
        .session
        .append_message(json!({"role":"assistant","content":"partial reply"}))
        .unwrap();
    fs::write(fx.worktree.join("note.txt"), "interrupt-good").unwrap();
    let interrupt_id = record_checkpoint(
        &store,
        &fx.session_key,
        "turn-1",
        CheckpointKind::Interrupt,
        Some(m2),
    );

    let state = init_context_state(&fx.session, &ContextConfig::default(), "sys").unwrap();
    let texts: Vec<String> = state
        .messages
        .iter()
        .filter_map(|m| m.text_content().map(str::to_string))
        .collect();

    assert!(
        texts.iter().any(|t| t == "hello"),
        "resume hydrate 应保留用户消息"
    );
    assert!(
        texts.iter().any(|t| t == "partial reply"),
        "Interrupt checkpoint 存在时，partial assistant 仍应参与后续 hydrate"
    );
    let listed = store
        .list(&fx.session_key, Default::default())
        .expect("interrupt checkpoint should be listable");
    assert!(
        listed.iter().any(|meta| meta.id == interrupt_id),
        "Interrupt checkpoint 应已写入，供 resume 后 /ckpt list 与 restore 使用"
    );
}
