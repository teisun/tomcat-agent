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
                serde_json::json!({"type":"session","id":"s","cwd":dir,"timestamp":chrono::Utc::now().to_rfc3339()})
            ),
        )
        .unwrap();
        assert_eq!(session_cwd(&transcript).as_deref(), Some(dir.as_path()));
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
                // Control the capture timestamp, never use the page's mutable mtime as order.
                let tick = self.manifest_clock.get() + 1;
                self.manifest_clock.set(tick);
                let mut rows: Vec<serde_json::Value> = fs::read_to_string(&manifest)
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                rows.last_mut().unwrap()["at"] = serde_json::json!(1_700_000_000_000u64 + tick);
                fs::write(
                    &manifest,
                    rows.iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("\n")
                        + "\n",
                )
                .unwrap();
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
        .set_modified(
            std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_700_000_000_000 + tick),
        )
        .unwrap();
    }
    fn keep(&self) -> String {
        let source = self.list().source_turn_id.unwrap();
        let id = keep(&self.transcript, "s", &source).unwrap().source_turn_id;
        let tick = self.manifest_clock.get() + 1;
        self.manifest_clock.set(tick);
        let controlled = format!("keep-{}", 1_700_000_000_000u64 + tick);
        fs::rename(
            session_dir(&self.transcript).join(&id),
            session_dir(&self.transcript).join(&controlled),
        )
        .unwrap();
        controlled
    }
    fn git(&self, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.dir)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn commit(&self) {
        fs::write(
            self.dir.join(".git/info/exclude"),
            "/s.jsonl\n/file-baselines/\n",
        )
        .unwrap();
        self.git(&["add", "."]);
        self.git(&["commit", "-qm", "version"]);
    }
    fn list(&self) -> SessionFilesResponse {
        list(&self.transcript, "s").unwrap()
    }
}

#[test]
fn session_file_reactivated_owner_keeps_true_earliest_baseline() {
    let f = Fixture::new();
    fs::write(f.dir.join("a"), b"v0").unwrap();
    let a = f.edit("u1", "a", b"v1");
    f.edit("u2", "a", b"v2");
    // A transcript-only checkpoint rewind can expose u1 again. Build is synthetic,
    // so subsequent native writes reuse u1 while both owners' disk baselines remain.
    f.edit("u1", "c", b"new path after returning to u1");
    let source = f.list().source_turn_id.unwrap();
    assert_eq!(
        baseline(&f.transcript, "s", &source, a.to_str().unwrap())
            .unwrap()
            .text,
        "v0",
        "appending another path must not reorder a's original backup"
    );
}

#[test]
fn session_file_reactivated_owner_after_keep_does_not_hide_new_changes() {
    let f = Fixture::new();
    fs::write(f.dir.join("a"), b"v0").unwrap();
    let a = f.edit("u1", "a", b"v1");
    f.edit("u2", "b", b"b1");
    let source = f.keep();
    assert!(f.list().files.is_empty());
    // Chat-only rewind keeps disk and Keep, then Build resumes the older owner.
    f.edit("u1", "a", b"v2");
    assert!(
        f.list()
            .files
            .iter()
            .any(|file| file.path == a.to_string_lossy()),
        "an old owner's seen set must not hide a post-Keep edit"
    );
    assert_eq!(
        baseline(&f.transcript, "s", &source, a.to_str().unwrap())
            .unwrap()
            .text,
        "v1"
    );
}

#[test]
fn session_file_keep_preserves_manual_edit_before_ai() {
    let f = Fixture::new();
    let path = f.edit("u1", "a", b"accepted");
    let source = f.keep();
    fs::write(&path, b"my manual edit").unwrap();
    assert!(
        f.list().files.is_empty(),
        "manual-only changes must not enter the new cycle"
    );
    f.edit("u1", "a", b"AI after my edit");
    assert_eq!(
        baseline(&f.transcript, "s", &source, path.to_str().unwrap())
            .unwrap()
            .text,
        "my manual edit"
    );
    restore(
        &f.transcript,
        "s",
        &source,
        &[path.to_string_lossy().into_owned()],
    )
    .unwrap();
    assert_eq!(fs::read(path).unwrap(), b"my manual edit");
}

#[test]
fn session_file_after_keep_uses_at_even_if_page_mtime_changes() {
    let f = Fixture::new();
    f.edit("u1", "a", b"accepted");
    let source = f.keep();
    let path = f.edit("u1", "a", b"AI after Keep");
    f.set_manifest_time("u1", 0);
    assert_eq!(f.list().files.len(), 1);
    assert_eq!(
        baseline(&f.transcript, "s", &source, path.to_str().unwrap())
            .unwrap()
            .text,
        "accepted"
    );
    let manifest = session_dir(&f.transcript).join("u1/baselines.jsonl");
    let mut rows: Vec<serde_json::Value> = fs::read_to_string(&manifest)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    rows.last_mut().unwrap()["at"] = serde_json::json!("invalid");
    fs::write(
        &manifest,
        rows.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    f.set_manifest_time("u1", 0);
    assert!(
        list(&f.transcript, "s").is_err(),
        "an invalid timestamp must not be hidden by old mtime"
    );
}

#[test]
fn session_file_keep_does_not_copy_files() {
    let f = Fixture::new();
    fs::write(f.dir.join("a"), b"original").unwrap();
    f.edit("u1", "a", b"accepted");
    f.edit("u2", "b", b"new");
    let root = session_dir(&f.transcript);
    let backup_bytes = || {
        let mut files = std::collections::BTreeMap::new();
        for dir in fs::read_dir(&root).unwrap() {
            for file in fs::read_dir(dir.unwrap().path()).unwrap() {
                let file = file.unwrap();
                if file.file_type().unwrap().is_file() {
                    files.insert(file.path(), fs::read(file.path()).unwrap());
                }
            }
        }
        files
    };
    let before = backup_bytes();
    let chat = fs::read(&f.transcript).unwrap();
    let id = f.keep();
    assert_eq!(backup_bytes(), before);
    assert_eq!(fs::read_dir(root.join(id)).unwrap().count(), 0);
    assert_eq!(fs::read(&f.transcript).unwrap(), chat);
    assert_eq!(fs::read(f.dir.join("a")).unwrap(), b"accepted");
}

#[test]
fn session_file_legacy_rows_without_at_are_ignored() {
    let f = Fixture::new();
    let dir = session_dir(&f.transcript).join("u1");
    fs::create_dir_all(&dir).unwrap();
    let manifest = dir.join("baselines.jsonl");
    let path = f.dir.join("a");
    fs::write(&path, b"old AI work").unwrap();
    let legacy =
        serde_json::json!({"path":path,"backup":null,"git_head":"old-head"}).to_string() + "\n";
    fs::write(&manifest, &legacy).unwrap();
    assert!(f.list().files.is_empty());
    assert!(f.list().source_turn_id.is_none());
    assert!(baseline(&f.transcript, "s", "u1", path.to_str().unwrap()).is_err());
    assert_eq!(fs::read_to_string(&manifest).unwrap(), legacy);
    f.edit("u1", "a", b"new AI work");
    assert_eq!(
        baseline(&f.transcript, "s", "u1", path.to_str().unwrap())
            .unwrap()
            .text,
        "old AI work"
    );
    let mut rows: Vec<serde_json::Value> = fs::read_to_string(&manifest)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    rows.last_mut().unwrap()["at"] = serde_json::json!("invalid");
    fs::write(
        manifest,
        rows.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    assert!(
        list(&f.transcript, "s").is_err(),
        "invalid new timestamps are not silently treated as legacy"
    );
}

#[test]
fn session_file_keep_millisecond_boundaries() {
    let f = Fixture::new();
    f.edit("u1", "a", b"one");
    let first = keep(&f.transcript, "s", "u1").unwrap().source_turn_id;
    let second = keep(&f.transcript, "s", &first).unwrap().source_turn_id;
    assert_ne!(first, second);
    let tracker = TurnFileBaselines::new(&f.transcript, "u1", f.dir.clone()).unwrap();
    let path = f.dir.join("a");
    let pending = tracker.prepare(path.to_str().unwrap()).unwrap();
    fs::write(&path, b"two").unwrap();
    pending.commit();
    assert_eq!(f.list().files.len(), 1);
    assert_eq!(
        baseline(&f.transcript, "s", &second, path.to_str().unwrap())
            .unwrap()
            .text,
        "one"
    );
    keep(&f.transcript, "s", &second).unwrap();
    assert!(f.list().files.is_empty());
}

#[test]
fn session_file_git_paths_with_spaces_are_literal() {
    let f = Fixture::new();
    f.git(&["init", "-q"]);
    let name = "odd [x] file.txt";
    fs::write(f.dir.join(name), b"v0").unwrap();
    f.commit();
    let path = f.edit("u1", name, b"v1");
    f.commit();
    assert!(f.list().files.is_empty());
    f.edit("u1", name, b"v2");
    assert_eq!(
        baseline(&f.transcript, "s", "u1", path.to_str().unwrap())
            .unwrap()
            .text,
        "v1"
    );
}

#[test]
fn session_file_branch_switch_uses_branch_version() {
    let f = Fixture::new();
    f.git(&["init", "-qb", "main"]);
    fs::write(f.dir.join("a"), b"main").unwrap();
    f.commit();
    f.git(&["checkout", "-qb", "other"]);
    fs::write(f.dir.join("a"), b"other").unwrap();
    f.commit();
    f.git(&["checkout", "-q", "main"]);
    let a = f.edit("u1", "a", b"AI main");
    f.git(&["checkout", "-qf", "other"]);
    assert!(f.list().files.is_empty());
    f.edit("u1", "a", b"AI other");
    restore(
        &f.transcript,
        "s",
        "u1",
        &[a.to_string_lossy().into_owned()],
    )
    .unwrap();
    assert_eq!(fs::read(a).unwrap(), b"other");
}

#[test]
fn session_file_missing_old_commit_preserves_untracked_backups() {
    let f = Fixture::new();
    f.git(&["init", "-q"]);
    fs::write(f.dir.join("a"), b"committed").unwrap();
    f.commit();
    fs::write(f.dir.join("untracked"), b"original untracked").unwrap();
    let a = f.edit("u1", "a", b"AI");
    let u = f.edit("u1", "untracked", b"AI");
    let manifest = session_dir(&f.transcript).join("u1/baselines.jsonl");
    let rows = fs::read_to_string(&manifest)
        .unwrap()
        .lines()
        .map(|line| {
            let mut row: serde_json::Value = serde_json::from_str(line).unwrap();
            row["git_head"] = serde_json::json!("f".repeat(40));
            row.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(manifest, rows + "\n").unwrap();
    assert_eq!(
        baseline(&f.transcript, "s", "u1", a.to_str().unwrap())
            .unwrap()
            .text,
        "committed"
    );
    assert_eq!(
        baseline(&f.transcript, "s", "u1", u.to_str().unwrap())
            .unwrap()
            .text,
        "original untracked"
    );
}

#[test]
fn session_file_rows_from_different_heads_only_keep_committed_paths() {
    let f = Fixture::new();
    f.git(&["init", "-q"]);
    for name in ["a", "b", "c", "d"] {
        fs::write(f.dir.join(name), b"v0").unwrap();
    }
    f.commit();
    let a = f.edit("u1", "a", b"v1");
    f.commit();
    let b = f.edit("u2", "b", b"v1");
    fs::write(f.dir.join("c"), b"unrelated commit").unwrap();
    f.git(&["add", "c"]);
    f.git(&["commit", "-qm", "unrelated"]);
    let d = f.edit("u3", "d", b"v1");
    f.git(&["add", "d"]);
    f.git(&["commit", "-qm", "d"]);
    let list = f.list();
    assert_eq!(list.files.len(), 1);
    assert_eq!(list.files[0].path, b.to_string_lossy());
    for (path, expected) in [(a, "v1"), (b, "v0"), (d, "v1")] {
        assert_eq!(
            baseline(&f.transcript, "s", "u1", path.to_str().unwrap())
                .unwrap()
                .text,
            expected
        );
    }
}

#[test]
fn session_file_uncommitted_revert_and_resets_follow_current_commit() {
    let f = Fixture::new();
    f.git(&["init", "-q"]);
    fs::write(f.dir.join("a"), b"v0\n").unwrap();
    f.commit();
    let a = f.edit("u1", "a", b"v1\n");
    f.commit();
    f.git(&["revert", "--no-commit", "HEAD"]);
    assert_eq!(
        baseline(&f.transcript, "s", "u1", a.to_str().unwrap())
            .unwrap()
            .text,
        "v1\n"
    );
    restore(
        &f.transcript,
        "s",
        "u1",
        &[a.to_string_lossy().into_owned()],
    )
    .unwrap();
    assert_eq!(fs::read(&a).unwrap(), b"v1\n");
    f.git(&["reset", "--soft", "HEAD~1"]);
    assert_eq!(f.list().files.len(), 1);
    assert_eq!(
        baseline(&f.transcript, "s", "u1", a.to_str().unwrap())
            .unwrap()
            .text,
        "v0\n"
    );
    f.commit();
    f.git(&["reset", "--hard", "HEAD~1"]);
    assert!(f.list().files.is_empty());
}

#[test]
fn session_file_discard_and_stash_follow_disk() {
    let f = Fixture::new();
    f.git(&["init", "-q"]);
    fs::write(f.dir.join("a"), b"v0").unwrap();
    f.commit();
    f.edit("u1", "a", b"v1");
    f.git(&["checkout", "--", "a"]);
    assert!(f.list().files.is_empty());
    f.edit("u1", "a", b"v1");
    f.git(&["stash", "push", "-q"]);
    assert!(f.list().files.is_empty());
    f.git(&["stash", "pop", "-q"]);
    assert_eq!(f.list().files.len(), 1);
}

#[test]
fn session_file_late_repository_and_outside_path_use_correct_baselines() {
    let f = Fixture::new();
    let a = f.edit("u1", "a", b"v1");
    f.git(&["init", "-q"]);
    f.commit();
    assert!(f.list().files.is_empty());
    f.edit("u1", "a", b"v2");
    assert_eq!(
        baseline(&f.transcript, "s", "u1", a.to_str().unwrap())
            .unwrap()
            .text,
        "v1"
    );
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().canonicalize().unwrap().join("outside");
    fs::write(&path, b"original").unwrap();
    let tracker = TurnFileBaselines::new(&f.transcript, "u2", f.dir.clone()).unwrap();
    let pending = tracker.prepare(path.to_str().unwrap()).unwrap();
    fs::write(&path, b"AI").unwrap();
    pending.commit();
    f.git(&["commit", "--allow-empty", "-qm", "unrelated"]);
    assert_eq!(
        baseline(&f.transcript, "s", "u1", path.to_str().unwrap())
            .unwrap()
            .text,
        "original"
    );
}

#[test]
fn session_file_keep_empties_list_without_touching_disk() {
    let f = Fixture::new();
    let a = f.edit("u1", "a", b"one");
    let b = f.edit("u2", "b", b"two");
    let chat = fs::read(&f.transcript).unwrap();
    let start = f.keep();
    assert!(f.list().files.is_empty());
    assert_eq!(f.list().source_turn_id.as_deref(), Some(start.as_str()));
    assert_eq!(fs::read(a).unwrap(), b"one");
    assert_eq!(fs::read(b).unwrap(), b"two");
    assert_eq!(fs::read(&f.transcript).unwrap(), chat);
    assert_eq!(
        fs::read_dir(session_dir(&f.transcript).join(&start))
            .unwrap()
            .count(),
        0,
        "Keep must contain no copied files or baseline manifest"
    );
}

#[test]
fn session_file_after_keep_diff_and_undo_stop_at_keep() {
    for same_owner in [true, false] {
        let f = Fixture::new();
        fs::write(f.dir.join("a"), b"v0").unwrap();
        let a = f.edit("u1", "a", b"v1");
        let start = f.keep();
        f.edit(if same_owner { "u1" } else { "u2" }, "a", b"v2");
        f.edit(if same_owner { "u1" } else { "u2" }, "b", b"new");
        assert_eq!(f.list().files.len(), 2);
        assert_eq!(
            baseline(&f.transcript, "s", &start, a.to_str().unwrap())
                .unwrap()
                .text,
            "v1"
        );
        restore(
            &f.transcript,
            "s",
            &start,
            &[a.to_string_lossy().into_owned()],
        )
        .unwrap();
        assert_eq!(fs::read(a).unwrap(), b"v1");
    }
}

#[test]
fn session_file_keep_recaptures_previously_unchanged_path() {
    let f = Fixture::new();
    fs::write(f.dir.join("a"), b"v0").unwrap();
    let a = f.edit("u1", "a", b"v1");
    f.edit("u1", "a", b"v0");
    assert!(f.list().files.is_empty());
    let start = f.keep();
    f.edit("u1", "a", b"v2");
    assert_eq!(f.list().files.len(), 1);
    assert_eq!(
        baseline(&f.transcript, "s", &start, a.to_str().unwrap())
            .unwrap()
            .text,
        "v0"
    );
}

#[test]
fn session_file_keep_records_absence_and_rejects_stale_source() {
    let f = Fixture::new();
    fs::write(f.dir.join("a"), b"old").unwrap();
    let a = f.edit("u1", "a", b"changed");
    fs::remove_file(&a).unwrap();
    let start = f.keep();
    f.edit("u1", "a", b"recreated");
    assert_eq!(f.list().files[0].status, SessionFileStatus::Added);
    assert!(
        !baseline(&f.transcript, "s", &start, a.to_str().unwrap())
            .unwrap()
            .existed
    );
    assert!(keep(&f.transcript, "s", "u1").is_err());
    assert!(baseline(&f.transcript, "s", "u1", a.to_str().unwrap()).is_err());
    assert_eq!(
        restore(
            &f.transcript,
            "s",
            "u1",
            &[a.to_string_lossy().into_owned()]
        )
        .unwrap_err()
        .0,
        "unknown_path"
    );
    assert_eq!(fs::read(&a).unwrap(), b"recreated");
    let next = f.keep();
    assert_ne!(start, next);
    assert!(f.list().files.is_empty());
    let legacy_keep = session_dir(&f.transcript).join("keep-9999999999999");
    fs::create_dir(&legacy_keep).unwrap();
    fs::write(legacy_keep.join("baselines.jsonl"), "legacy snapshot").unwrap();
    assert_eq!(f.list().source_turn_id.as_deref(), Some(next.as_str()));
}

#[test]
fn session_file_commit_counts_as_kept_and_rewrite_undo_uses_commit() {
    let f = Fixture::new();
    f.git(&["init", "-q"]);
    fs::write(f.dir.join("a"), b"v0").unwrap();
    f.commit();
    let a = f.edit("u1", "a", b"v1");
    assert_eq!(f.list().files.len(), 1);
    f.commit();
    assert!(f.list().files.is_empty());
    f.edit("u1", "a", b"v2");
    assert_eq!(f.list().files[0].status, SessionFileStatus::Modified);
    assert!(f.list().files[0].restorable);
    assert_eq!(
        baseline(&f.transcript, "s", "u1", a.to_str().unwrap())
            .unwrap()
            .text,
        "v1"
    );
    restore(
        &f.transcript,
        "s",
        "u1",
        &[a.to_string_lossy().into_owned()],
    )
    .unwrap();
    assert_eq!(fs::read(a).unwrap(), b"v1");
}

#[test]
fn session_file_keep_commit_revert_uses_commit() {
    let f = Fixture::new();
    f.git(&["init", "-q"]);
    fs::write(f.dir.join("a"), b"v0\n").unwrap();
    f.commit();
    let a = f.edit("u1", "a", b"v1\n");
    let start = f.keep();
    f.commit();
    f.git(&["revert", "--no-edit", "HEAD"]);
    assert!(f.list().files.is_empty());
    assert!(
        baseline(&f.transcript, "s", &start, a.to_str().unwrap()).is_err(),
        "Keep retired the old AI records"
    );
    f.edit("u1", "a", b"new AI change\n");
    assert_eq!(
        baseline(&f.transcript, "s", &start, a.to_str().unwrap())
            .unwrap()
            .text,
        "v0\n"
    );
    restore(
        &f.transcript,
        "s",
        &start,
        &[a.to_string_lossy().into_owned()],
    )
    .unwrap();
    assert_eq!(fs::read(a).unwrap(), b"v0\n");
}

#[test]
fn session_file_partial_commit_and_uncommitted_paths() {
    let f = Fixture::new();
    f.git(&["init", "-q"]);
    fs::write(f.dir.join("a"), b"v0").unwrap();
    fs::write(f.dir.join("b"), b"b0").unwrap();
    f.commit();
    let a = f.edit("u1", "a", b"part");
    let b = f.edit("u1", "b", b"b1");
    f.git(&["add", "a"]);
    f.git(&["commit", "-qm", "partial"]);
    f.edit("u1", "a", b"part and more");
    assert_eq!(f.list().files.len(), 2);
    assert_eq!(
        baseline(&f.transcript, "s", "u1", a.to_str().unwrap())
            .unwrap()
            .text,
        "part"
    );
    assert_eq!(
        baseline(&f.transcript, "s", "u1", b.to_str().unwrap())
            .unwrap()
            .text,
        "b0"
    );
    restore(
        &f.transcript,
        "s",
        "u1",
        &[
            a.to_string_lossy().into_owned(),
            b.to_string_lossy().into_owned(),
        ],
    )
    .unwrap();
    assert_eq!(fs::read(a).unwrap(), b"part");
    assert_eq!(fs::read(b).unwrap(), b"b0");
}

#[test]
fn session_file_commit_new_deleted_and_eol_filtered_paths() {
    let f = Fixture::new();
    f.git(&["init", "-q"]);
    fs::write(f.dir.join(".gitattributes"), "a text eol=crlf\n").unwrap();
    fs::write(f.dir.join("a"), b"old\r\n").unwrap();
    fs::write(f.dir.join("d"), b"deleted").unwrap();
    f.commit();
    f.edit("u1", "a", b"new\r\n");
    f.edit("u1", "c", b"created");
    let d = f.edit("u1", "d", b"changed");
    fs::remove_file(d).unwrap();
    f.commit();
    assert!(
        f.list().files.is_empty(),
        "committed CRLF must compare using checkout filters"
    );
    f.edit("u1", "c", b"changed again");
    assert_eq!(f.list().files[0].status, SessionFileStatus::Modified);
}

#[test]
fn session_file_normal_turns_accumulate_without_switching() {
    let f = Fixture::new();
    f.user("u1", false);
    let a = f.edit("u1", "one.txt", b"one\n");
    f.user("u2", false);
    let b = f.edit("u2", "two.txt", b"two\n");
    let result = f.list();
    assert_eq!(
        result
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        vec![a.to_str().unwrap(), b.to_str().unwrap()],
        "continue must retain earlier files"
    );
    assert_eq!(result.source_turn_id.as_deref(), Some("u1"));
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
    assert_eq!(f.list().source_turn_id.as_deref(), Some("z-old"));
}

#[test]
fn session_file_same_path_across_turns_uses_earliest_baseline() {
    let f = Fixture::new();
    fs::write(f.dir.join("a.txt"), "v0").unwrap();
    f.edit("u1", "a.txt", b"v1");
    let path = f.edit("u2", "a.txt", b"v2");
    assert_eq!(f.list().files.len(), 1);
    assert_eq!(
        baseline(&f.transcript, "s", "u1", path.to_str().unwrap())
            .unwrap()
            .text,
        "v0"
    );
    let chat = fs::read(&f.transcript).unwrap();
    restore(
        &f.transcript,
        "s",
        "u1",
        &[path.to_string_lossy().into_owned()],
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "v0");
    assert!(f.list().files.is_empty());
    assert_eq!(f.list().source_turn_id.as_deref(), Some("u1"));
    assert_eq!(fs::read(&f.transcript).unwrap(), chat);
}

#[test]
fn session_file_accumulated_restore_only_changes_selected_paths() {
    let f = Fixture::new();
    f.edit("u1", "a", b"a");
    f.edit("u1", "b", b"b");
    let c = f.edit("u2", "c", b"c");
    let a = f.edit("u2", "a", b"a2");
    assert_eq!(f.list().files.len(), 3);
    restore(
        &f.transcript,
        "s",
        "u1",
        &[a.to_string_lossy().into_owned()],
    )
    .unwrap();
    assert!(!a.exists());
    assert!(c.exists());
    assert_eq!(f.list().files.len(), 2);
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
fn session_file_scope_order_uses_record_at_not_manifest_mtime() {
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
    assert_eq!(f.list().source_turn_id.as_deref(), Some(ids[0]));
    assert_eq!(f.list().files.len(), 3);
    f.set_manifest_time(ids[0], 5);
    for _ in 0..3 {
        assert_eq!(f.list().source_turn_id.as_deref(), Some(ids[0]));
    }
}

#[test]
fn session_file_corrupt_manifest_before_scope_is_ignored() {
    let f = Fixture::new();
    f.edit("old", "a", b"a");
    let start = f.keep();
    f.edit("latest", "b", b"b");
    fs::write(
        session_dir(&f.transcript).join("old/baselines.jsonl"),
        "{broken\n",
    )
    .unwrap();
    f.set_manifest_time("old", 1);
    let result = f.list();
    assert_eq!(result.source_turn_id.as_deref(), Some(start.as_str()));
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
    assert_eq!(f.list().source_turn_id.as_deref(), Some("u1"));
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
fn session_file_empty_commit_keeps_backup_and_allows_restore() {
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
    assert!(f.list().files[0].restorable);
    assert_eq!(f.list().files[0].blocked_reason, None);
    assert_eq!(
        baseline(&f.transcript, "s", "u", a.to_str().unwrap())
            .unwrap()
            .text,
        "before"
    );
    restore(&f.transcript, "s", "u", &[a.to_string_lossy().into_owned()]).unwrap();
    assert_eq!(fs::read_to_string(a).unwrap(), "before");
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
