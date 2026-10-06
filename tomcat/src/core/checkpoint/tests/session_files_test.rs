use super::super::{file_baselines::*, session_files::*};
use std::cell::Cell;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

struct Fixture {
    _temp: tempfile::TempDir,
    dir: PathBuf,
    transcript: PathBuf,
    manifest_clock: Cell<u64>,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().canonicalize().unwrap();
        let transcript = dir.join("s.jsonl");
        fs::write(
            &transcript,
            format!(
                "{}\n",
                serde_json::json!({"type":"session","id":"s","cwd":dir})
            ),
        )
        .unwrap();
        Self {
            _temp: temp,
            dir,
            transcript,

            manifest_clock: Cell::new(0),
        }
    }
    fn user(&self, id: &str, superseded: bool) {
        writeln!(OpenOptions::new().append(true).open(&self.transcript).unwrap(),"{}",serde_json::json!({"type":"message","id":id,"message":{"role":"user","content":"question","superseded":superseded}})).unwrap();
    }
    fn edit(&self, id: &str, file: &str, value: &[u8]) -> PathBuf {
        let path = self.dir.join(file);
        let tracker = TurnFileBaselines::new(&self.transcript, id, self.dir.clone()).unwrap();
        let manifest = session_dir(&self.transcript)
            .join(id)
            .join("baselines.jsonl");
        let prior_len = fs::metadata(&manifest).ok().map(|m| m.len());
        let pending = tracker.prepare(path.to_str().unwrap());
        fs::write(&path, value).unwrap();
        if let Some(pending) = pending {
            pending.commit();
        }
        if let Ok(metadata) = fs::metadata(&manifest) {
            if Some(metadata.len()) != prior_len {
                // Deterministic publication order, even on coarse timestamp filesystems.
                let tick = self.manifest_clock.get() + 1;
                self.manifest_clock.set(tick);
                self.set_manifest_time(id, tick);
            }
        }
        path
    }
    fn set_manifest_time(&self, id: &str, tick: u64) {
        fs::File::open(
            session_dir(&self.transcript)
                .join(id)
                .join("baselines.jsonl"),
        )
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + tick))
        .unwrap();
    }
    fn list(&self) -> SessionFilesResponse {
        list(&self.transcript, "s").unwrap()
    }
}

#[test]
fn session_file_question_turns_keep_last_editing_turn() {
    let f = Fixture::new();
    f.user("z-old", false);
    f.edit("z-old", "a.txt", b"first\n");
    for i in 0..85 {
        f.user(&format!("q{i}"), false);
    }
    assert_eq!(f.list().source_turn_id.as_deref(), Some("z-old"));
    assert_eq!(f.list().files.len(), 1);
    // No-op success neither publishes a new turn nor consumes the capture forever.
    f.edit("q84", "a.txt", b"first\n");
    assert_eq!(f.list().source_turn_id.as_deref(), Some("z-old"));
    f.edit("q84", "a.txt", b"second\n");
    assert_eq!(f.list().source_turn_id.as_deref(), Some("q84"));
}

#[test]
fn session_file_same_path_uses_source_turn_baseline_and_empty_does_not_fall_back() {
    let f = Fixture::new();
    f.user("z", false);
    f.edit("z", "a.txt", b"B\n");
    f.edit("z", "untouched.txt", b"keep\n");
    f.user("a", false);
    let path = f.edit("a", "a.txt", b"C\n");
    assert_eq!(f.list().source_turn_id.as_deref(), Some("a"));
    assert_eq!(f.list().files.len(), 1);
    assert_eq!(f.list().files[0].added, Some(1));
    assert_eq!(f.list().files[0].removed, Some(1));
    assert_eq!(
        baseline(&f.transcript, "s", "a", path.to_str().unwrap())
            .unwrap()
            .text,
        "B\n"
    );
    let chat = fs::read(&f.transcript).unwrap();
    restore(
        &f.transcript,
        "s",
        "a",
        &[path.to_string_lossy().into_owned()],
    )
    .unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "B\n");
    assert_eq!(fs::read(f.dir.join("untouched.txt")).unwrap(), b"keep\n");
    assert_eq!(fs::read(&f.transcript).unwrap(), chat);
    f.user("question", false);
    let result = f.list();
    assert_eq!(result.source_turn_id.as_deref(), Some("a"));
    assert!(result.files.is_empty());
}

#[test]
fn session_file_first_successful_change_switches_whole_list_and_selected_only() {
    let f = Fixture::new();
    f.user("u1", false);
    f.edit("u1", "a", b"a");
    f.edit("u1", "b", b"b");
    f.user("u2", false);
    let c = f.edit("u2", "c", b"c");
    let a = f.edit("u2", "a", b"a2");
    assert_eq!(f.list().files.len(), 2);
    assert!(f.list().files.iter().all(|file| !file.path.ends_with("/b")));
    restore(
        &f.transcript,
        "s",
        "u2",
        &[a.to_string_lossy().into_owned()],
    )
    .unwrap();
    assert_eq!(fs::read(a).unwrap(), b"a");
    assert!(c.exists());
    assert_eq!(f.list().files.len(), 1);
}

#[test]
fn session_file_pruned_source_and_missing_backup() {
    let f = Fixture::new();
    assert!(f.list().source_turn_id.is_none());
    f.user("u", false);
    fs::write(f.dir.join("a"), b"before").unwrap();
    let a = f.edit("u", "a", b"after");
    f.edit("u", "new", b"");
    let dir = session_dir(&f.transcript).join("u");
    let row: serde_json::Value = serde_json::from_str(
        fs::read_to_string(dir.join("baselines.jsonl"))
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    fs::remove_file(dir.join(row["backup"].as_str().unwrap())).unwrap();
    let files = f.list().files;
    assert_eq!(
        files[0].blocked_reason,
        Some(SessionFileBlockedReason::BackupMissing)
    );
    assert_eq!(files[0].added, None);
    assert!(!files[0].restorable);
    assert!(files[1].restorable);
    assert_eq!(
        restore(&f.transcript, "s", "u", &[a.to_string_lossy().into_owned()])
            .unwrap_err()
            .0,
        "unavailable"
    );
    prune(
        &f.dir,
        7,
        std::time::SystemTime::now() + std::time::Duration::from_secs(8 * 86400),
    );
    assert!(f.list().source_turn_id.is_none());
}

#[test]
fn session_file_latest_manifest_mtime_wins_regardless_of_id_format() {
    let f = Fixture::new();
    let ids = [
        "user-1791-zz",
        "1791_9",
        "550e8400-e29b-41d4-a716-446655440000",
    ];
    for (index, id) in ids.iter().enumerate() {
        f.user(id, false);
        f.edit(id, &format!("file-{index}"), b"changed");
    }
    f.set_manifest_time(ids[0], 3);
    f.set_manifest_time(ids[1], 10); // Neither last in transcript nor lexicographically last.
    f.set_manifest_time(ids[2], 5);
    let root = session_dir(&f.transcript);
    fs::create_dir_all(root.join("prepare-only")).unwrap();
    fs::write(root.join("prepare-only").join("backup-copy"), b"old").unwrap();
    fs::create_dir(root.join("empty-newer")).unwrap();
    fs::write(root.join("empty-newer").join("baselines.jsonl"), b"").unwrap();
    f.set_manifest_time("empty-newer", 100);
    assert_eq!(f.list().source_turn_id.as_deref(), Some(ids[1]));
    f.set_manifest_time(ids[0], 10);
    for _ in 0..3 {
        assert_eq!(f.list().source_turn_id.as_deref(), Some(ids[0]));
    }
}

#[test]
fn session_file_old_corrupt_manifest_does_not_block_latest() {
    let f = Fixture::new();
    f.edit("old", "a", b"a");
    f.edit("latest", "b", b"b");
    fs::write(
        session_dir(&f.transcript).join("old/baselines.jsonl"),
        "{broken\n",
    )
    .unwrap();
    f.set_manifest_time("old", 1);
    let result = f.list();
    assert_eq!(result.source_turn_id.as_deref(), Some("latest"));
    assert_eq!(result.files.len(), 1);
    assert!(result.files[0].path.ends_with("/b"));
}

#[test]
fn session_file_corrupt_latest_manifest_errors_without_fallback() {
    let f = Fixture::new();
    f.edit("old", "a", b"a");
    f.edit("latest", "b", b"b");
    fs::write(
        session_dir(&f.transcript).join("latest/baselines.jsonl"),
        "{broken\n",
    )
    .unwrap();
    f.set_manifest_time("latest", 2);
    assert_eq!(
        latest_editing_turn(&f.transcript).unwrap().as_deref(),
        Some("latest")
    );
    assert!(list(&f.transcript, "s").is_err());
}

#[test]
fn session_file_selection_does_not_read_transcript() {
    let f = Fixture::new();
    f.user("u1", false);
    f.edit("u1", "a", b"a");
    writeln!(
        OpenOptions::new().append(true).open(&f.transcript).unwrap(),
        "{{broken JSON"
    )
    .unwrap();
    assert_eq!(f.list().source_turn_id.as_deref(), Some("u1"));
    fs::remove_file(&f.transcript).unwrap();
    assert_eq!(
        latest_editing_turn(&f.transcript).unwrap().as_deref(),
        Some("u1")
    );
}

#[cfg(unix)]
#[test]
fn session_file_selection_ignores_symlink_directories_and_manifests() {
    let f = Fixture::new();
    f.edit("u1", "a", b"a");
    let root = session_dir(&f.transcript);
    std::os::unix::fs::symlink(root.join("u1"), root.join("z-alias")).unwrap();
    fs::create_dir(root.join("z-linked-manifest")).unwrap();
    std::os::unix::fs::symlink(
        root.join("u1/baselines.jsonl"),
        root.join("z-linked-manifest/baselines.jsonl"),
    )
    .unwrap();
    fs::create_dir_all(root.join("z-directory-manifest/baselines.jsonl")).unwrap();
    assert_eq!(f.list().source_turn_id.as_deref(), Some("u1"));
}

#[test]
fn session_file_status_binary_large_new_deleted_and_many_files() {
    let f = Fixture::new();
    f.user("u", false);
    fs::write(f.dir.join("deleted"), b"old\n").unwrap();
    let deleted = f.edit("u", "deleted", b"new\n");
    fs::remove_file(deleted).unwrap();
    f.edit("u", "binary", &[0, 1]);
    let large = f.edit("u", "large", &vec![b'x'; 1_500_001]);
    f.edit("u", "empty", b"");
    for i in 0..120 {
        f.edit("u", &format!("file-{i}"), b"line\n");
    }
    let result = f.list();
    assert_eq!(result.files.len(), 124);
    assert_eq!(result.files[0].status, SessionFileStatus::Deleted);
    assert_eq!(result.files[0].removed, Some(1));
    assert_eq!(result.files[1].added, None);
    assert_eq!(result.files[2].added, None);
    assert!(baseline(&f.transcript, "s", "u", large.to_str().unwrap()).is_ok()); // new before is empty
    assert_eq!(result.files[3].status, SessionFileStatus::Added);
    assert_eq!(result.files[3].added, Some(0));
}

#[test]
fn session_file_restore_rejects_unknown_path_before_any_write() {
    let f = Fixture::new();
    f.user("u", false);
    let a = f.edit("u", "a", b"created");
    let result = restore(
        &f.transcript,
        "s",
        "u",
        &[
            a.to_string_lossy().into_owned(),
            f.dir.join("unknown").to_string_lossy().into_owned(),
        ],
    );
    assert_eq!(result.unwrap_err().0, "unknown_path");
    assert!(a.exists());
    assert!(baseline(&f.transcript, "s", "../u", a.to_str().unwrap()).is_err());
    assert!(baseline(&f.transcript, "other", "absent", a.to_str().unwrap()).is_err());
}

#[test]
fn session_file_git_head_move_blocks_restore_but_not_diff() {
    let f = Fixture::new();
    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(&f.dir)
            .status()
            .unwrap()
            .success());
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.name=T",
        "-c",
        "user.email=t@test.invalid",
        "commit",
        "--allow-empty",
        "-qm",
        "one",
    ]);
    f.user("u", false);
    fs::write(f.dir.join("a"), b"before").unwrap();
    let a = f.edit("u", "a", b"after");
    git(&[
        "-c",
        "user.name=T",
        "-c",
        "user.email=t@test.invalid",
        "commit",
        "--allow-empty",
        "-qm",
        "two",
    ]);
    assert_eq!(
        f.list().files[0].blocked_reason,
        Some(SessionFileBlockedReason::HeadMoved)
    );
    assert_eq!(
        baseline(&f.transcript, "s", "u", a.to_str().unwrap())
            .unwrap()
            .text,
        "before"
    );
    assert_eq!(
        restore(&f.transcript, "s", "u", &[a.to_string_lossy().into_owned()])
            .unwrap_err()
            .0,
        "head_moved"
    );
}

#[cfg(unix)]
#[test]
fn session_file_restore_rejects_symlink_and_preserves_neighbor() {
    let f = Fixture::new();
    f.user("u", false);
    let a = f.edit("u", "a", b"a");
    let other = f.dir.join("other");
    fs::write(&other, b"outside").unwrap();
    fs::remove_file(&a).unwrap();
    std::os::unix::fs::symlink(&other, &a).unwrap();
    assert!(!f.list().files[0].restorable);
    assert!(restore(&f.transcript, "s", "u", &[a.to_string_lossy().into_owned()]).is_err());
    assert_eq!(fs::read(other).unwrap(), b"outside");
}

#[test]
fn session_file_absence_record_deletes_only_file_and_old_source_is_explicit() {
    let f = Fixture::new();
    f.user("u1", false);
    fs::create_dir(f.dir.join("newdir")).unwrap();
    let a = f.edit("u1", "newdir/a", b"a");
    f.user("u2", false);
    f.edit("u2", "b", b"b");
    restore(
        &f.transcript,
        "s",
        "u1",
        &[a.to_string_lossy().into_owned()],
    )
    .unwrap();
    assert!(!a.exists());
    assert!(a.parent().unwrap().exists());
    assert!(f.dir.join("b").exists());
}

#[test]
fn session_file_no_backups_does_not_read_missing_transcript() {
    assert!(
        list(Path::new("/nonexistent-tomcat-files-test/s.jsonl"), "s")
            .unwrap()
            .source_turn_id
            .is_none()
    );
}
