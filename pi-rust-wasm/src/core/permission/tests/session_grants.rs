//! `SessionGrants` 容器与前缀匹配。

use crate::core::permission::{session_grants::SessionGrants, GrantTrigger};
use std::path::PathBuf;

#[test]
fn session_grants_contains_subpath() {
    let g = SessionGrants::new();
    g.add(
        PathBuf::from("/Users/alice/proj"),
        GrantTrigger::UserConfirm,
    );
    assert!(g.contains(&PathBuf::from("/Users/alice/proj")));
    assert!(g.contains(&PathBuf::from("/Users/alice/proj/sub/file.rs")));
    assert!(!g.contains(&PathBuf::from("/Users/alice/other")));
}

#[test]
fn session_grants_preserves_trigger_for_subpath() {
    let g = SessionGrants::new();
    g.add(PathBuf::from("/tmp/dragged"), GrantTrigger::DraggedPathMenu);
    assert_eq!(
        g.trigger_for(&PathBuf::from("/tmp/dragged/file.txt")),
        Some(GrantTrigger::DraggedPathMenu)
    );
    assert_eq!(g.trigger_for(&PathBuf::from("/tmp/elsewhere")), None);
}
