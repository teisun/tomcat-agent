use super::*;
use crate::core::permission::{DefaultPermissionGate, GateConfig, SessionGrants};

fn gate() -> DefaultPermissionGate {
    DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: PathBuf::from("/unused"),
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    )
}

#[test]
fn opened_instruction_replaced_at_same_path_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonicalize(temp.path()).unwrap();
    let path = root.join("AGENTS.md");
    fs::write(&path, "OLD_CONTENT").unwrap();
    let gate = gate();
    let target = resolve(&root, &path, &gate).unwrap();
    let opened = open_no_follow(&target).unwrap();
    fs::rename(&path, root.join("moved.md")).unwrap();
    fs::write(&path, "NEW_CONTENT").unwrap();
    assert!(read_opened(&root, &path, &target, opened, &gate).is_err());
}

#[cfg(unix)]
#[test]
fn opened_instruction_parent_swapped_to_external_link_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let base = canonicalize(temp.path()).unwrap();
    let root = base.join("repo");
    let child = root.join("sub");
    let outside = base.join("outside");
    fs::create_dir_all(&child).unwrap();
    fs::create_dir(&outside).unwrap();
    let path = child.join("AGENTS.md");
    fs::write(&path, "ORIGINAL").unwrap();
    fs::write(outside.join("AGENTS.md"), "EXTERNAL").unwrap();
    let gate = gate();
    let target = resolve(&root, &path, &gate).unwrap();
    let opened = open_no_follow(&target).unwrap();
    fs::rename(&child, root.join("moved")).unwrap();
    std::os::unix::fs::symlink(&outside, &child).unwrap();
    assert!(read_opened(&root, &path, &target, opened, &gate).is_err());
}
