use super::super::file_baselines::*;
use std::fs;
use std::path::Path;

#[test]
fn prune_keeps_active_sessions_and_drops_idle_ones() {
    let dir = tempfile::tempdir().unwrap();
    let now = std::time::SystemTime::now();
    let old = now - std::time::Duration::from_secs(8 * 86400);
    for id in ["active", "idle", "deleted"] {
        let transcript = dir.path().join(format!("{id}.jsonl"));
        for name in ["user-one", "keep-100"] {
            let backup = session_dir(&transcript).join(name);
            fs::create_dir_all(&backup).unwrap();
            fs::File::open(&backup).unwrap().set_modified(old).unwrap();
        }
        if id != "deleted" {
            fs::write(&transcript, "session").unwrap();
            fs::File::open(&transcript)
                .unwrap()
                .set_modified(if id == "idle" { old } else { now })
                .unwrap();
        }
    }
    prune(dir.path(), 7, now);
    assert!(session_dir(&dir.path().join("active.jsonl"))
        .join("user-one")
        .exists());
    assert!(session_dir(&dir.path().join("active.jsonl"))
        .join("keep-100")
        .exists());
    assert!(!session_dir(&dir.path().join("idle.jsonl")).exists());
    assert!(!session_dir(&dir.path().join("deleted.jsonl")).exists());
}

#[test]
fn discard_keeps_after_preserves_earlier_boundaries_and_message_backups() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("s.jsonl");
    let root = session_dir(&transcript);
    for name in ["u1", "keep-100", "keep-200", "keep-invalid"] {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    fs::create_dir_all(root.join("keep-300")).unwrap();
    fs::write(root.join("keep-300/baselines.jsonl"), "legacy snapshot").unwrap();
    fs::create_dir_all(root.join("keep-+400")).unwrap();
    assert_eq!(latest_keep(&transcript).unwrap().unwrap().0, 200);
    discard_keeps_after(&transcript, 100);
    assert!(root.join("keep-100").exists());
    assert!(!root.join("keep-200").exists());
    assert!(root.join("u1").exists());
    assert!(root.join("keep-invalid").exists());
    discard_keeps_after(&transcript, 99);
    assert!(!root.join("keep-100").exists());
    assert_eq!(
        fs::read_to_string(root.join("keep-300/baselines.jsonl")).unwrap(),
        "legacy snapshot"
    );
    assert!(root.join("keep-+400").exists());
}

#[test]
fn file_baseline_recapture_after_keep_preserves_original_for_rewind() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let transcript = root.join("s.jsonl");
    let path = root.join("a");
    fs::write(&path, b"original").unwrap();
    let first = TurnFileBaselines::new(&transcript, "u1", root.clone()).unwrap();
    capture(&first, &path, b"accepted");
    let keep = crate::core::checkpoint::session_files::keep(&transcript, "s", "u1").unwrap();
    assert!(!first.keep_is_current());
    let second = TurnFileBaselines::new(&transcript, "u1", root.clone()).unwrap();
    assert!(second.keep_is_current());
    capture(&second, &path, b"next cycle");
    let owner = session_dir(&transcript).join("u1");
    let rows = read_rows(&owner).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].at < rows[1].at);
    assert_ne!(rows[0].backup, rows[1].backup);
    assert_eq!(
        fs::read(owner.join(rows[0].backup.as_ref().unwrap())).unwrap(),
        b"original"
    );
    assert_eq!(
        fs::read(owner.join(rows[1].backup.as_ref().unwrap())).unwrap(),
        b"accepted"
    );
    assert_eq!(
        crate::core::checkpoint::session_files::baseline(
            &transcript,
            "s",
            &keep.source_turn_id,
            path.to_str().unwrap()
        )
        .unwrap()
        .text,
        "accepted"
    );
    let now = chrono::Utc::now();
    preview(
        &transcript,
        &["u1".into()],
        &now.to_rfc3339(),
        &root,
        7,
        now,
    )
    .unwrap()
    .restore()
    .unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"original");
    discard_keeps_after(&transcript, 0);
    assert!(!second.keep_is_current());
}

#[test]
fn file_baseline_legacy_rows_do_not_seed_tracker_or_rewind() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let transcript = root.join("s.jsonl");
    let path = root.join("a");
    fs::write(&path, b"legacy work").unwrap();
    let owner = session_dir(&transcript).join("u1");
    fs::create_dir_all(&owner).unwrap();
    fs::write(
        owner.join("baselines.jsonl"),
        serde_json::json!({"path":path,"backup":null,"git_head":"obsolete"}).to_string() + "\n",
    )
    .unwrap();
    let now = chrono::Utc::now();
    assert!(matches!(
        preview(
            &transcript,
            &["u1".into()],
            &now.to_rfc3339(),
            &root,
            7,
            now
        ),
        Err(RevertReason::NoBaselines)
    ));
    let tracker = TurnFileBaselines::new(&transcript, "u1", root.clone()).unwrap();
    capture(&tracker, &path, b"new work");
    preview(
        &transcript,
        &["u1".into()],
        &now.to_rfc3339(),
        &root,
        7,
        now,
    )
    .unwrap()
    .restore()
    .unwrap();
    assert_eq!(fs::read(path).unwrap(), b"legacy work");
}

#[cfg(unix)]
#[test]
fn file_baseline_content_dedup_preserves_per_path_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let transcript = root.join("s.jsonl");
    let tracker = TurnFileBaselines::new(&transcript, "u1", root.clone()).unwrap();
    for (name, mode) in [("a", 0o755), ("b", 0o644)] {
        let path = root.join(name);
        fs::write(&path, b"same original bytes").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        capture(&tracker, &path, b"changed");
        fs::remove_file(path).unwrap();
    }
    let rows = read_rows(&session_dir(&transcript).join("u1")).unwrap();
    assert_eq!(rows[0].backup, rows[1].backup);
    let now = chrono::Utc::now();
    preview(
        &transcript,
        &["u1".into()],
        &now.to_rfc3339(),
        &root,
        7,
        now,
    )
    .unwrap()
    .restore()
    .unwrap();
    for (name, mode) in [("a", 0o755), ("b", 0o644)] {
        assert_eq!(fs::read(root.join(name)).unwrap(), b"same original bytes");
        assert_eq!(
            fs::metadata(root.join(name)).unwrap().permissions().mode() & 0o777,
            mode
        );
    }
}

#[test]
fn file_baseline_session_cwd_comes_only_from_header() {
    let temp = tempfile::tempdir().unwrap();
    let manager = crate::SessionManager::new(temp.path().join("sessions"));
    let entry = manager
        .create_session(manager.current_session_key(), None)
        .unwrap();
    let transcript = manager.transcript_path(&entry.session_id);
    assert!(session_cwd(&transcript).is_none());
    let capture =
        session_cwd(&transcript).and_then(|cwd| TurnFileBaselines::new(&transcript, "u", cwd));
    assert!(capture.is_none());
}

fn capture(tracker: &std::sync::Arc<TurnFileBaselines>, path: &Path, after: &[u8]) {
    let pending = tracker.prepare(path.to_str().unwrap());
    fs::write(path, after).unwrap();
    if let Some(pending) = pending {
        pending.commit();
    }
}

#[test]
fn file_baseline_first_write_per_turn_and_restore_earliest_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("sessions/s.jsonl");
    let a = dir.path().join("a [test].txt");
    let b = dir.path().join("new.txt");
    let untouched = dir.path().join("bash-only.txt");
    fs::write(&a, b"original").unwrap();
    fs::write(&untouched, b"leave me").unwrap();
    let first = TurnFileBaselines::new(&transcript, "u1", dir.path().into()).unwrap();
    capture(&first, &a, b"first edit");
    assert!(first.prepare(a.to_str().unwrap()).is_none());
    capture(&first, &b, b"created");
    let second = TurnFileBaselines::new(&transcript, "u2", dir.path().into()).unwrap();
    capture(&second, &a, b"manual edit after AI");
    let now = chrono::Utc::now();
    let files = preview(
        &transcript,
        &["u1".into(), "u2".into()],
        &now.to_rfc3339(),
        dir.path(),
        7,
        now,
    )
    .unwrap();
    assert_eq!(files.paths().len(), 2);
    files.restore().unwrap();
    files.restore().unwrap();
    assert_eq!(fs::read(&a).unwrap(), b"original");
    assert!(!b.exists());
    assert_eq!(fs::read(&untouched).unwrap(), b"leave me");
    discard_turns(&transcript, &["u2".into()]);
    assert!(session_dir(&transcript).join("u1").exists());
    assert!(!session_dir(&transcript).join("u2").exists());
    discard_session(&transcript);
    assert!(!session_dir(&transcript).exists());
}

#[test]
fn file_baseline_declined_write_does_not_publish_and_missing_backup_skips() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("s.jsonl");
    let a = dir.path().join("a.txt");
    fs::write(&a, b"original").unwrap();
    let tracker = TurnFileBaselines::new(&transcript, "u", dir.path().into()).unwrap();
    drop(tracker.prepare(a.to_str().unwrap()));
    let now = chrono::Utc::now();
    assert!(matches!(
        preview(
            &transcript,
            &["u".into()],
            &now.to_rfc3339(),
            dir.path(),
            7,
            now
        ),
        Err(RevertReason::NoBaselines)
    ));
    capture(&tracker, &a, b"changed");
    let manifest = fs::read_to_string(session_dir(&transcript).join("u/baselines.jsonl")).unwrap();
    let row: serde_json::Value = serde_json::from_str(manifest.trim()).unwrap();
    fs::remove_file(
        session_dir(&transcript)
            .join("u")
            .join(row["backup"].as_str().unwrap()),
    )
    .unwrap();
    fs::write(&a, b"changed").unwrap();
    preview(
        &transcript,
        &["u".into()],
        &now.to_rfc3339(),
        dir.path(),
        7,
        now,
    )
    .unwrap()
    .restore()
    .unwrap();
    assert_eq!(fs::read(a).unwrap(), b"changed");
}

#[test]
fn file_baseline_expiration_and_large_file() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("s.jsonl");
    let file = dir.path().join("large.bin");
    fs::write(&file, vec![9u8; 20 * 1024 * 1024]).unwrap();
    let tracker = TurnFileBaselines::new(&transcript, "u", dir.path().into()).unwrap();
    capture(&tracker, &file, &[8u8]);
    let now = chrono::Utc::now();
    let old = now - chrono::Duration::days(7);
    assert!(matches!(
        preview(
            &transcript,
            &["u".into()],
            &old.to_rfc3339(),
            dir.path(),
            7,
            now
        ),
        Err(RevertReason::Expired)
    ));
    assert!(preview(
        &transcript,
        &["u".into()],
        &(old + chrono::Duration::seconds(1)).to_rfc3339(),
        dir.path(),
        7,
        now
    )
    .is_ok());
}

#[test]
fn file_baseline_relative_path_matches_write_tool_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("s.jsonl");
    let tracker = TurnFileBaselines::new(&transcript, "u", dir.path().into()).unwrap();
    let scratch = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let filename = scratch
        .path()
        .strip_prefix(std::env::current_dir().unwrap())
        .unwrap()
        .join("new.txt");
    let pending = tracker.prepare(filename.to_str().unwrap()).unwrap();
    fs::write(&filename, b"created").unwrap();
    pending.commit();
    let now = chrono::Utc::now();
    preview(
        &transcript,
        &["u".into()],
        &now.to_rfc3339(),
        dir.path(),
        7,
        now,
    )
    .unwrap()
    .restore()
    .unwrap();
    assert!(!filename.exists());
}

#[test]
fn file_baseline_git_head_change_disables_restore_without_mutating_files() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "--allow-empty",
        "-qm",
        "baseline",
    ]);
    let transcript = dir.path().join("s.jsonl");
    let file = dir.path().join("file.txt");
    fs::write(&file, b"before").unwrap();
    let tracker = TurnFileBaselines::new(&transcript, "u", dir.path().into()).unwrap();
    capture(&tracker, &file, b"later");
    let now = chrono::Utc::now();
    assert!(preview(
        &transcript,
        &["u".into()],
        &now.to_rfc3339(),
        dir.path(),
        7,
        now
    )
    .is_ok());
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "--allow-empty",
        "-qm",
        "new commit",
    ]);
    assert!(matches!(
        preview(
            &transcript,
            &["u".into()],
            &now.to_rfc3339(),
            dir.path(),
            7,
            now
        ),
        Err(RevertReason::GitHeadMoved)
    ));
    assert_eq!(fs::read(&file).unwrap(), b"later");
}

#[test]
fn file_baseline_git_branch_reset_and_late_repository_boundaries() {
    for initially_git in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        if initially_git {
            git(&["init", "-q"]);
            git(&[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "original",
            ]);
        }
        let original_head = git_head(dir.path());
        let transcript = dir.path().join("s.jsonl");
        let file = dir.path().join("file.txt");
        fs::write(&file, "original").unwrap();
        let tracker = TurnFileBaselines::new(&transcript, "u", dir.path().into()).unwrap();
        capture(&tracker, &file, b"AI change");
        let now = chrono::Utc::now();
        let check = || {
            preview(
                &transcript,
                &["u".into()],
                &now.to_rfc3339(),
                dir.path(),
                7,
                now,
            )
        };
        assert!(check().is_ok(), "non-git/null HEAD must be usable too");
        if initially_git {
            git(&["checkout", "-qb", "other"]);
        } else {
            git(&["init", "-q"]);
        }
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "new HEAD",
        ]);
        assert!(matches!(check(), Err(RevertReason::GitHeadMoved)));
        assert_eq!(fs::read_to_string(&file).unwrap(), "AI change");
        if let Some(head) = original_head {
            git(&["reset", "--soft", &head]);
            check().unwrap().restore().unwrap();
            assert_eq!(fs::read_to_string(&file).unwrap(), "original");
        }
    }
}

#[test]
fn file_baseline_prune_and_partial_failure_are_retryable() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("s.jsonl");
    fs::write(&transcript, "active session").unwrap();
    let first = dir.path().join("a.txt");
    let second = dir.path().join("b.txt");
    fs::write(&first, b"before").unwrap();
    let tracker = TurnFileBaselines::new(&transcript, "u", dir.path().into()).unwrap();
    capture(&tracker, &first, b"after");
    capture(&tracker, &second, b"created");
    fs::remove_file(&second).unwrap();
    fs::create_dir(&second).unwrap(); // Never recursively delete a substituted directory.
    let now = chrono::Utc::now();
    let files = preview(
        &transcript,
        &["u".into()],
        &now.to_rfc3339(),
        dir.path(),
        7,
        now,
    )
    .unwrap();
    assert!(files.restore().is_err());
    assert_eq!(fs::read(&first).unwrap(), b"before");
    assert!(session_dir(&transcript).join("u").exists());
    fs::remove_dir(&second).unwrap();
    files.restore().unwrap();
    prune(dir.path(), 7, std::time::SystemTime::now());
    assert!(session_dir(&transcript).join("u").exists());
    prune(
        dir.path(),
        7,
        std::time::SystemTime::now() + std::time::Duration::from_secs(8 * 86400),
    );
    assert!(!session_dir(&transcript).join("u").exists());
}

#[test]
fn file_baseline_noop_releases_capture_and_later_change_publishes() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("s.jsonl");
    let file = dir.path().join("a.txt");
    fs::write(&file, "before").unwrap();
    let tracker = TurnFileBaselines::new(&transcript, "u", dir.path().into()).unwrap();
    tracker.prepare(file.to_str().unwrap()).unwrap().commit();
    assert!(!session_dir(&transcript).join("u/baselines.jsonl").exists());
    capture(&tracker, &file, b"after");
    assert!(session_dir(&transcript).join("u/baselines.jsonl").exists());
}

#[cfg(unix)]
#[test]
fn file_baseline_restore_keeps_exec_bit() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("s.jsonl");
    let file = dir.path().join("script.sh");
    fs::write(&file, "original").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    let tracker = TurnFileBaselines::new(&transcript, "u", dir.path().into()).unwrap();
    capture(&tracker, &file, b"changed");
    let now = chrono::Utc::now();
    preview(
        &transcript,
        &["u".into()],
        &now.to_rfc3339(),
        dir.path(),
        7,
        now,
    )
    .unwrap()
    .restore()
    .unwrap();
    assert_eq!(
        fs::metadata(file).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn file_baseline_backup_failure_does_not_panic_or_publish() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("s.jsonl");
    fs::write(dir.path().join("file-baselines"), b"not a directory").unwrap();
    if let Some(tracker) = TurnFileBaselines::new(&transcript, "u", dir.path().into()) {
        assert!(tracker
            .prepare(dir.path().join("file").to_str().unwrap())
            .is_none());
    }
}
