use crate::core::session::{
    preheat_cache, resume_index, tool_display_sidecar, transcript, user_message_sidecar,
    CompactionResult, SessionManager,
};
use crate::infra::events::ToolDisplay;

#[test]
fn usage_guard_blocks_delete_until_explicit_release_even_with_clones() {
    let temp = tempfile::tempdir().unwrap();
    let owner = SessionManager::new(temp.path().into());
    let entry = owner.new_current_session(None).unwrap();
    owner.pin_session(&entry.session_id).unwrap();
    let lingering = owner.clone();
    let other = SessionManager::new(temp.path().into());
    assert!(other
        .begin_delete_session(&entry.session_id)
        .err()
        .unwrap()
        .to_string()
        .contains("session_in_use"));
    owner.release_session_usage();
    assert_eq!(
        lingering.current_session_id().unwrap().as_deref(),
        Some(entry.session_id.as_str())
    );
    let result = other.delete_session(&entry.session_id).unwrap();
    assert!(result.warnings.is_empty());
    assert!(!owner.transcript_path(&entry.session_id).exists());
    assert!(!temp
        .path()
        .join("locks")
        .join(format!("{}.lock", entry.session_id))
        .exists());
    assert!(lingering
        .append_message(serde_json::json!({"role":"user","content":"late"}))
        .is_err());
    assert!(other
        .delete_session(&entry.session_id)
        .unwrap()
        .warnings
        .is_empty());
    assert!(lingering.pin_session(&entry.session_id).is_err());
}

#[test]
fn usage_guard_moves_with_binding_and_does_not_release_old_on_failed_switch() {
    let temp = tempfile::tempdir().unwrap();
    let owner = SessionManager::new(temp.path().into());
    let a = owner.new_current_session(None).unwrap();
    owner.pin_session(&a.session_id).unwrap();
    let b = owner.new_current_session(None).unwrap();
    let other = SessionManager::new(temp.path().into());
    other.delete_session(&a.session_id).unwrap();
    assert!(other.get_session_by_id(&a.session_id).unwrap().is_none());
    assert!(owner.switch_current_to_session_id("missing").is_err());
    assert!(other.delete_session(&b.session_id).is_err());
    owner.release_session_usage();
    other.delete_session(&b.session_id).unwrap();
    assert!(other.get_session_by_id(&b.session_id).unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn releasing_usage_unlocks_passive_fork_copies_but_not_other_owners() {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let temp = tempfile::tempdir().unwrap();
    let owner = SessionManager::new(temp.path().into());
    let session = owner.new_current_session(None).unwrap();
    owner.pin_session(&session.session_id).unwrap();
    let peer = SessionManager::new(temp.path().into());
    peer.pin_session(&session.session_id).unwrap();
    let deleter = SessionManager::new(temp.path().into());
    let (mut parent, child) = UnixStream::pair().unwrap();
    parent
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    child
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let fd = child.as_raw_fd();
    let executable = std::env::current_exe().unwrap();
    let spawn = std::thread::spawn(move || {
        let _child_socket = child;
        let mut command = Command::new(executable);
        command
            .arg("--list")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: the post-fork callback only uses async-signal-safe syscalls.
        // Keep inherited descriptors alive before exec closes their CLOEXEC copies.
        unsafe {
            command.pre_exec(move || {
                let mut byte = 1_u8;
                for writing in [true, false] {
                    loop {
                        let result = if writing {
                            libc::write(fd, (&byte as *const u8).cast(), 1)
                        } else {
                            libc::read(fd, (&mut byte as *mut u8).cast(), 1)
                        };
                        if result == 1 {
                            break;
                        }
                        let error = std::io::Error::last_os_error();
                        if result < 0 && error.kind() == std::io::ErrorKind::Interrupted {
                            continue;
                        }
                        return Err(error);
                    }
                }
                Ok(())
            });
        }
        command.spawn()
    });
    let ready = parent.read_exact(&mut [0_u8]);
    owner.release_session_usage();
    let peer_still_blocks = matches!(
        deleter.delete_session(&session.session_id),
        Err(crate::AppError::Config(code)) if code == "session_in_use"
    );
    peer.release_session_usage();
    let deletion = deleter.delete_session(&session.session_id);
    // Always let the child exec and reap it before asserting, including the red run.
    let released = parent.write_all(&[1]);
    let status = spawn.join().unwrap().unwrap().wait().unwrap();
    ready.unwrap();
    released.unwrap();
    assert!(status.success());
    assert!(
        peer_still_blocks,
        "an independently pinned owner must still block deletion"
    );
    deletion.expect("passive pre-exec copies must not retain a released owner's usage lock");
    assert!(deleter
        .get_session_by_id(&session.session_id)
        .unwrap()
        .is_none());
}

#[test]
fn usage_guard_rejects_unsafe_ids_and_scope_before_any_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let owner = SessionManager::new_scoped(temp.path().into(), "A");
    let target = owner.new_current_session(None).unwrap();
    let other = SessionManager::new_scoped(temp.path().into(), "B");
    for id in ["../escape", "/absolute", "", ".", "a/b"] {
        assert!(other.begin_delete_session(id).is_err());
    }
    assert!(other.begin_delete_session(&target.session_id).is_err());
    assert!(!temp.path().join("locks").exists());
    assert!(owner.transcript_path(&target.session_id).exists());
}

#[test]
fn deletion_guard_drop_without_commit_and_store_failure_preserve_files() {
    let temp = tempfile::tempdir().unwrap();
    let manager = SessionManager::new(temp.path().into());
    let entry = manager.new_current_session(None).unwrap();
    drop(manager.begin_delete_session(&entry.session_id).unwrap());
    assert!(manager
        .get_session_by_id(&entry.session_id)
        .unwrap()
        .is_some());
    let guard = manager.begin_delete_session(&entry.session_id).unwrap();
    std::fs::write(manager.store_path(), "corrupt").unwrap();
    assert!(guard.commit(None).is_err());
    assert!(manager.transcript_path(&entry.session_id).exists());
}

#[test]
fn deletion_cleans_only_owned_trail_data_and_reports_partial_io_failure() {
    let temp = tempfile::tempdir().unwrap();
    let manager = SessionManager::new(temp.path().join("sessions"));
    let target = manager.new_current_session(None).unwrap();
    let trail = temp.path().join("trail");
    let todo = crate::core::plan_runtime::todo_runtime::todo_path_for(&trail, &target.session_id);
    std::fs::create_dir_all(todo.parent().unwrap()).unwrap();
    std::fs::write(&todo, "draft todos").unwrap();
    let results = trail.join("tool-results").join(&target.session_id);
    std::fs::create_dir_all(&results).unwrap();
    std::fs::write(results.join("one.txt"), "target").unwrap();
    std::fs::write(trail.join("tool-results/shared.txt"), "keep").unwrap();
    let transcript = manager.transcript_path(&target.session_id);
    let sidecar = tool_display_sidecar::tool_display_sidecar_path(&transcript);
    std::fs::create_dir(&sidecar).unwrap(); // remove_file fails, after metadata is committed.
    let result = manager
        .begin_delete_session(&target.session_id)
        .unwrap()
        .commit(Some(&trail))
        .unwrap();
    assert!(!result.warnings.is_empty());
    assert!(manager
        .get_session_by_id(&target.session_id)
        .unwrap()
        .is_none());
    assert!(!todo.exists() && !results.exists() && !transcript.exists());
    assert!(trail.join("tool-results/shared.txt").exists());
}

#[test]
fn deletion_reports_target_lease_cleanup_failure_without_erasing_shared_data() {
    let temp = tempfile::tempdir().unwrap();
    let manager = SessionManager::new(temp.path().into());
    let target = manager.new_current_session(None).unwrap();
    let other = manager.new_current_session(None).unwrap();
    let store = manager.attachment_store();
    let sha = store.put(b"shared attachment").unwrap();
    store.mark_pending(&other.session_id, &sha).unwrap();
    let bad_marker = store
        .root()
        .join("pending")
        .join(&target.session_id)
        .join(&sha);
    std::fs::create_dir_all(&bad_marker).unwrap();
    let expected = std::fs::remove_file(&bad_marker).unwrap_err().to_string();
    let result = manager.delete_session(&target.session_id).unwrap();
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.contains(&expected)));
    assert!(manager
        .get_session_by_id(&target.session_id)
        .unwrap()
        .is_none());
    assert!(!manager.transcript_path(&target.session_id).exists());
    assert!(manager
        .get_session_by_id(&other.session_id)
        .unwrap()
        .is_some());
    assert!(manager.transcript_path(&other.session_id).exists());
    assert_eq!(
        store.list_pending(&other.session_id).unwrap(),
        vec![sha.clone()]
    );
    assert_eq!(
        store.get(&sha).unwrap(),
        Some(b"shared attachment".to_vec())
    );
}

#[test]
fn late_title_rewrite_cannot_recreate_deleted_transcript() {
    let temp = tempfile::tempdir().unwrap();
    let manager = SessionManager::new(temp.path().into());
    let entry = manager.new_current_session(None).unwrap();
    manager.append_message_with_id(
        serde_json::json!({"role": "assistant", "content": "answer", "summary_title": "before"}),
        "answer-1",
    ).unwrap();
    let path = manager.transcript_path(&entry.session_id);
    let before = std::fs::read(&path).unwrap();
    let deletion = manager.begin_delete_session(&entry.session_id).unwrap();
    let error = manager
        .rewrite_message_summary_title_in_session(&entry.session_id, "answer-1", "after")
        .unwrap_err();
    assert!(matches!(error, crate::AppError::Config(ref code) if code == "session_in_use"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(deletion.commit(None).unwrap().warnings.is_empty());
    assert!(manager
        .rewrite_message_summary_title_in_session(&entry.session_id, "answer-1", "late",)
        .is_err());
    assert!(!path.exists());
    assert!(!resume_index::resume_index_path(&path).exists());
}

#[test]
fn title_rewrite_after_release_preserves_existing_session_behavior() {
    let temp = tempfile::tempdir().unwrap();
    let manager = SessionManager::new(temp.path().into());
    let entry = manager.new_current_session(None).unwrap();
    manager.pin_session(&entry.session_id).unwrap();
    manager.append_message_with_id(
        serde_json::json!({"role": "assistant", "content": "answer", "summary_title": "before"}),
        "answer-1",
    ).unwrap();
    assert_eq!(
        manager
            .rewrite_message_summary_title_in_session(&entry.session_id, "answer-1", "while-open",)
            .unwrap(),
        1
    );
    manager.release_session_usage();
    assert_eq!(
        manager
            .rewrite_message_summary_title_in_session(&entry.session_id, "answer-1", "after-close",)
            .unwrap(),
        1
    );
    let path = manager.transcript_path(&entry.session_id);
    let raw = std::fs::read_to_string(&path).unwrap();
    let message: serde_json::Value = raw
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|value| value["id"] == "answer-1")
        .unwrap();
    assert_eq!(message["message"]["summary_title"], "after-close");
    assert!(resume_index::resume_index_path(&path).exists());
}

#[test]
fn late_transcript_and_sidecar_writes_do_not_create_an_absent_owner() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("gone.jsonl");
    assert!(transcript::append_line(&path, "{}").is_err());
    assert!(tool_display_sidecar::append_tool_display(
        &path,
        "t",
        "2026-01-01T00:00:00Z",
        &ToolDisplay::Text {
            text: "test".into()
        }
    )
    .is_err());
    assert!(user_message_sidecar::ensure_user_message_sidecar(&path).is_err());
    preheat_cache::write_preheat_cache(
        &path,
        &CompactionResult {
            summary_text: "late summary".into(),
            covered_start_id: "start".into(),
            covered_end_id: "end".into(),
            covered_count: 2,
            transcript_compaction_entry_id: Some("marker".into()),
            estimated_covered_tokens_before: None,
            estimated_summary_tokens: None,
            estimated_tokens_saved: None,
            preheat_elapsed_ms: 0,
        },
    )
    .unwrap();
    assert!(!preheat_cache::preheat_cache_path(&path).exists());
    assert!(!path.exists());
    assert!(!tool_display_sidecar::tool_display_sidecar_path(&path).exists());
    assert!(!user_message_sidecar::user_message_sidecar_path(&path).exists());
}
