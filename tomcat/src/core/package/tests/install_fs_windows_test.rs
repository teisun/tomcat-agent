use super::*;
use crate::test_support::create_junction;

#[test]
fn source_directory_replaced_after_open_is_reported_as_changed() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("pkg");
    let moved = root.path().join("moved");
    fs::create_dir(&source).unwrap();
    let handle = open_source_directory(&source).unwrap();
    ensure_source_directory_unchanged(&source, &handle).unwrap();
    let changed = tr(
        "package.directoryChanged",
        &[("path", &source.display().to_string())],
    );

    fs::rename(&source, &moved).unwrap();
    fs::create_dir(&source).unwrap();
    let error = ensure_source_directory_unchanged(&source, &handle).unwrap_err();
    assert!(error.to_string().contains(&changed), "{error}");

    // 指回原目录的 junction：跟随链接时身份相同，不跟随才能识破。
    fs::remove_dir(&source).unwrap();
    create_junction(&source, &moved);
    let error = ensure_source_directory_unchanged(&source, &handle).unwrap_err();
    assert!(error.to_string().contains(&changed), "{error}");
}
