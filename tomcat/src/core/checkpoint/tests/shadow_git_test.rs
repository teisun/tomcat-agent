use std::fs;
use std::process::{Command, Stdio};

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
