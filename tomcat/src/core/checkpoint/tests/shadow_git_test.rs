use std::fs;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::core::checkpoint::store::CheckpointStore;
use crate::core::checkpoint::types::{CheckpointKind, CheckpointRecordRequest, ListOptions};

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn request(turn_id: &str) -> CheckpointRecordRequest {
    CheckpointRecordRequest {
        session_id: "s1".to_string(),
        turn_id: turn_id.to_string(),
        kind: CheckpointKind::TurnEnd,
        message_anchor: Some(format!("msg-{turn_id}")),
        notes: None,
    }
}

fn request_for(session_id: &str, turn_id: &str) -> CheckpointRecordRequest {
    CheckpointRecordRequest {
        session_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        kind: CheckpointKind::TurnEnd,
        message_anchor: Some(format!("msg-{turn_id}")),
        notes: None,
    }
}

#[test]
fn record_and_list_round_trip() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("workspace");
    let trail = root.path().join("trail");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&trail).unwrap();
    fs::write(worktree.join("note.txt"), "hello").unwrap();

    let store = ShadowGitStore::new(trail, worktree);
    let id = store.record(request("t1")).unwrap();
    assert!(!id.is_null(), "首次 record 应生成真实 checkpoint");

    let listed = store
        .list("s1", ListOptions { limit: None })
        .expect("list should succeed");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
    assert!(listed[0].git_commit.is_some());

    let shown = store.show(&id).unwrap().expect("show should return meta");
    assert_eq!(shown.turn_id, "t1");
    assert_eq!(shown.message_anchor.as_deref(), Some("msg-t1"));
}

#[test]
fn record_dedups_same_turn_and_kind() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("workspace");
    let trail = root.path().join("trail");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&trail).unwrap();
    fs::write(worktree.join("a.txt"), "first").unwrap();

    let store = ShadowGitStore::new(trail, worktree.clone());
    let first = store.record(request("t1")).unwrap();
    fs::write(worktree.join("a.txt"), "second").unwrap();
    let second = store.record(request("t1")).unwrap();
    assert_eq!(first, second, "同 turn + kind 应走 dedup");
    let listed = store.list("s1", ListOptions::default()).unwrap();
    assert_eq!(listed.len(), 1);
}

#[test]
fn record_reuses_latest_checkpoint_when_worktree_unchanged() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("workspace");
    let trail = root.path().join("trail");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&trail).unwrap();
    fs::write(worktree.join("a.txt"), "first").unwrap();

    let store = ShadowGitStore::new(trail, worktree);
    let first = store.record(request("t1")).unwrap();
    let second = store.record(request("t2")).unwrap();
    assert_eq!(first, second, "无 diff 时应复用最新 checkpoint");
    let listed = store.list("s1", ListOptions::default()).unwrap();
    assert_eq!(listed.len(), 1, "无 diff 不应生成空提交");
}

#[test]
fn record_first_no_diff_empty_worktree_creates_baseline_checkpoint() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("workspace");
    let trail = root.path().join("trail");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&trail).unwrap();

    let store = ShadowGitStore::new(trail, worktree);
    let id = store.record(request("t1")).unwrap();
    assert!(
        !id.is_null(),
        "空工作区首次 record 也应生成 baseline checkpoint"
    );
    let listed = store.list("s1", ListOptions::default()).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
    assert!(listed[0].git_commit.is_some());
}

#[test]
fn concurrent_record_on_same_store_does_not_hit_index_lock_or_deadlock() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("workspace");
    let trail = root.path().join("trail");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&trail).unwrap();
    fs::write(worktree.join("note.txt"), "hello").unwrap();

    let store = Arc::new(ShadowGitStore::new(trail, worktree));
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for (session_id, turn_id) in [("s1", "t1"), ("s2", "t2")] {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            store.record(request_for(session_id, turn_id))
        }));
    }

    barrier.wait();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("thread join"))
        .collect();
    for result in results {
        let id = result.expect("concurrent record should succeed");
        assert!(
            !id.to_string().contains("index.lock"),
            "checkpoint id/result should not surface index.lock failures"
        );
    }
}

#[test]
fn record_returns_null_when_worktree_exceeds_limit() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("workspace");
    let trail = root.path().join("trail");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&trail).unwrap();
    fs::write(worktree.join("a.txt"), "1").unwrap();
    fs::write(worktree.join("b.txt"), "2").unwrap();

    let store = ShadowGitStore::with_max_workdir_files(trail, worktree, 1);
    let id = store.record(request("t1")).unwrap();
    assert!(id.is_null(), "文件数超阈值时应跳过 checkpoint");
}

#[test]
fn record_ignores_default_excludes_and_gitignored_dirs_when_counting_limit() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("workspace");
    let trail = root.path().join("trail");
    fs::create_dir_all(worktree.join("target")).unwrap();
    fs::create_dir_all(worktree.join("ignored-dir")).unwrap();
    fs::create_dir_all(worktree.join("src")).unwrap();
    fs::create_dir_all(&trail).unwrap();
    fs::write(worktree.join(".gitignore"), "ignored-dir/\n").unwrap();
    for index in 0..8 {
        fs::write(
            worktree.join("target").join(format!("artifact-{index}.bin")),
            format!("bin-{index}"),
        )
        .unwrap();
        fs::write(
            worktree.join("ignored-dir").join(format!("draft-{index}.txt")),
            format!("draft-{index}"),
        )
        .unwrap();
    }
    fs::write(worktree.join("src").join("keep.ts"), "export const keep = true;\n").unwrap();

    let store = ShadowGitStore::with_max_workdir_files(trail, worktree, 2);
    let id = store.record(request("t1")).unwrap();
    assert!(
        !id.is_null(),
        "target/ 与 .gitignore 目录都不应计入文件上限；真实可快照文件刚好卡在上限时也应成功 record"
    );
}

#[test]
fn ignored_only_changes_reuse_the_latest_checkpoint() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("workspace");
    let trail = root.path().join("trail");
    fs::create_dir_all(worktree.join("ignored-dir")).unwrap();
    fs::create_dir_all(&trail).unwrap();
    fs::write(worktree.join(".gitignore"), "ignored-dir/\n").unwrap();
    fs::write(worktree.join("keep.txt"), "tracked").unwrap();

    let store = ShadowGitStore::new(trail, worktree.clone());
    let first = store.record(request("t1")).unwrap();
    assert!(!first.is_null());

    fs::write(worktree.join("ignored-dir").join("draft.txt"), "ignored change").unwrap();

    let second = store.record(request("t2")).unwrap();
    assert_eq!(
        second, first,
        "只改 .gitignore 忽略的文件时，不应生成新 checkpoint，而应复用最近一次"
    );
    let listed = store.list("s1", ListOptions::default()).unwrap();
    assert_eq!(listed.len(), 1);
}

#[test]
fn new_canonicalizes_existing_worktree_path() {
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("workspace");
    fs::create_dir_all(&worktree).unwrap();

    let store = ShadowGitStore::new(root.path().join("trail"), worktree.clone());
    assert_eq!(
        store.work_tree,
        fs::canonicalize(worktree).unwrap(),
        "已存在目录应规范化，避免同一路径别名落到不同 checkpoint 仓"
    );
}

#[test]
fn summarize_failure_omits_file_list() {
    let output = fake_output(
        "?? should-not-leak-a\n?? should-not-leak-b\n M should-not-leak-c\n",
        "",
        1,
    );

    let summary = summarize_git_failure(
        "git status",
        std::path::Path::new("/tmp/worktree"),
        Some(Duration::from_secs(30)),
        &output,
    );

    assert!(summary.contains("git status timed out after 30s"));
    assert!(summary.contains("work_tree=/tmp/worktree"));
    assert!(summary.contains("captured output omitted"));
    assert!(!summary.contains("??"));
    assert!(!summary.contains('\n'));
    assert!(!summary.contains("should-not-leak-a"));
    assert!(!summary.contains("should-not-leak-b"));
    assert!(!summary.contains("should-not-leak-c"));
}

#[test]
fn summarize_failure_reports_captured_bytes() {
    let noisy = "x".repeat(4096);
    let output = fake_output(&noisy, "", 1);

    let summary = summarize_git_failure(
        "git add",
        std::path::Path::new("/tmp/worktree"),
        None,
        &output,
    );

    assert!(summary.contains("git add failed"));
    assert!(summary.contains("captured output omitted 4096 bytes"));
    assert!(summary.len() < 200, "摘要应保持短小，不回显原始输出");
}

#[test]
fn cooldown_skips_record_without_counting_files() {
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("workspace");
    let trail = root.path().join("trail");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&trail).unwrap();

    let store = ShadowGitStore::new(trail, worktree);
    store.force_cooldown_until(Instant::now() + Duration::from_secs(60));

    let id = store.record(request("t1")).unwrap();
    assert!(id.is_null(), "冷却期内应静默跳过 checkpoint");
    assert_eq!(store.file_count_calls(), 0, "冷却期内不应去数文件");
    assert!(
        !store.git_dir.exists(),
        "冷却期内应在 repo 初始化之前直接返回"
    );
}

#[test]
fn cooldown_expiry_resumes_record_and_clears_cooldown() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("workspace");
    let trail = root.path().join("trail");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&trail).unwrap();
    fs::write(worktree.join("note.txt"), "hello").unwrap();

    let store = ShadowGitStore::new(trail, worktree);
    store.force_cooldown_until(Instant::now() - Duration::from_secs(1));

    let id = store.record(request("t1")).unwrap();
    assert!(!id.is_null(), "冷却期过后应恢复拍快照");
    assert_eq!(store.cooldown_until(), None, "成功试探后应清空冷却");
    assert_eq!(store.file_count_calls(), 1, "恢复后应重新执行数文件");
}

#[cfg(unix)]
#[test]
fn timeout_sets_cooldown_and_omits_captured_file_list() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("workspace");
    let trail = root.path().join("trail");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&trail).unwrap();
    fs::write(worktree.join("note.txt"), "hello").unwrap();

    let script = make_status_timeout_git_script(root.path());
    let store = ShadowGitStore::new(trail, worktree)
        .with_git_program(script)
        .with_git_timeout(Duration::from_millis(50));

    let err = store.record(request("t1")).unwrap_err();
    assert!(err.is_timeout(), "超时应进入专门的 timeout 分支");
    let text = err.to_string();
    assert!(text.contains("50ms"));
    assert!(!text.contains("should-not-leak-from-status"));
    assert!(!text.contains('\n'));
    assert!(
        store.cooldown_until().is_some(),
        "超时后应写入下一次允许试探的时间点"
    );
}

#[test]
fn exclude_keeps_noise_out_of_snapshot() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("workspace");
    let trail = root.path().join("trail");
    fs::create_dir_all(worktree.join("node_modules")).unwrap();
    fs::create_dir_all(worktree.join("third_party")).unwrap();
    fs::create_dir_all(worktree.join("src")).unwrap();
    fs::create_dir_all(&trail).unwrap();
    fs::write(worktree.join("node_modules/x.js"), "console.log('x');").unwrap();
    fs::write(worktree.join("third_party/y.bin"), "blob").unwrap();
    fs::write(worktree.join("src/a.txt"), "keep me").unwrap();

    let store = ShadowGitStore::new(trail, worktree);
    let id = store.record(request("t1")).unwrap();
    assert!(!id.is_null());

    let tracked = String::from_utf8(store.run_git(["ls-files"]).unwrap().stdout).unwrap();
    assert!(tracked.contains("src/a.txt"));
    assert!(!tracked.contains("node_modules/x.js"));
    assert!(!tracked.contains("third_party/y.bin"));

    let exclude = fs::read_to_string(store.git_dir.join("info").join("exclude")).unwrap();
    assert!(exclude.contains("node_modules/"));
    assert!(exclude.contains("third_party/"));
}

#[test]
fn exclude_backfilled_on_existing_repo() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("workspace");
    let trail = root.path().join("trail");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&trail).unwrap();
    fs::write(worktree.join("src.txt"), "hello").unwrap();

    let store = ShadowGitStore::new(trail, worktree);
    store.ensure_repo_initialized().unwrap();
    let exclude_path = store.git_dir.join("info").join("exclude");
    fs::write(&exclude_path, "# keep-my-comment\n").unwrap();

    store.record(request("t1")).unwrap();

    let exclude = fs::read_to_string(exclude_path).unwrap();
    assert!(exclude.contains("# keep-my-comment"));
    assert!(exclude.contains("node_modules/"));
    assert!(exclude.contains("third_party/"));
}

fn fake_output(stdout: &str, stderr: &str, code: i32) -> Output {
    Output {
        status: exit_status(code),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

#[cfg(unix)]
fn exit_status(code: i32) -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;

    std::process::ExitStatus::from_raw(code << 8)
}

#[cfg(windows)]
fn exit_status(code: i32) -> std::process::ExitStatus {
    use std::os::windows::process::ExitStatusExt;

    std::process::ExitStatus::from_raw(code as u32)
}

#[cfg(unix)]
fn make_status_timeout_git_script(dir: &std::path::Path) -> std::path::PathBuf {
    let script = dir.join("git-timeout.sh");
    fs::write(
        &script,
        r#"#!/usr/bin/env bash
if [ "$1" = "status" ]; then
  echo "?? should-not-leak-from-status"
  sleep 0.2
  exit 0
fi
exec git "$@"
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).unwrap();
    script
}
