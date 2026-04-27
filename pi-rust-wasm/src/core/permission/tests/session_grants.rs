//! `SessionGrants` / `DraggedPaths` 容器与前缀匹配。

use crate::core::permission::session_grants::{DraggedPaths, SessionGrants};
use std::path::PathBuf;

#[test]
fn session_grants_contains_subpath() {
    let g = SessionGrants::new();
    g.add(PathBuf::from("/Users/alice/proj"));
    assert!(g.contains(&PathBuf::from("/Users/alice/proj")));
    assert!(g.contains(&PathBuf::from("/Users/alice/proj/sub/file.rs")));
    assert!(!g.contains(&PathBuf::from("/Users/alice/other")));
}

#[test]
fn dragged_paths_contains_subpath() {
    let d = DraggedPaths::new();
    d.add(PathBuf::from("/tmp/dragged"));
    assert!(d.contains(&PathBuf::from("/tmp/dragged/file.txt")));
    assert!(!d.contains(&PathBuf::from("/tmp/elsewhere")));
}
