use std::fs;
use std::process::{Command, Stdio};

use super::super::store::{CheckpointStore, SwitchingCheckpointStore};
use super::super::types::{CheckpointKind, CheckpointRecordRequest};

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[test]
fn switching_store_auto_upgrades_when_git_becomes_available() {
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("workspace");
    let trail = root.path().join("trail");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&trail).unwrap();
    fs::write(worktree.join("note.txt"), "hello").unwrap();

    let switcher = SwitchingCheckpointStore::new(trail, worktree, false);
    assert!(!switcher.is_shadow(), "初始化为 git 不可用时应先落在 noop");

    let id = switcher
        .record(CheckpointRecordRequest {
            session_id: "s1".to_string(),
            turn_id: "t1".to_string(),
            kind: CheckpointKind::TurnEnd,
            message_anchor: Some("m1".to_string()),
            notes: None,
        })
        .unwrap();
    assert!(!id.is_null(), "git 已存在时首次真实操作应自动升级到 shadow");
    assert!(switcher.is_shadow());
}
