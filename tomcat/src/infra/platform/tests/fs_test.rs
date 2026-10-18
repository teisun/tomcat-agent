use std::io::Write;
use std::path::{Path, PathBuf};

use super::{
    mode_bits, open_no_follow, same_file, write_atomic, write_private_file_atomic,
    PermissionSnapshot,
};
use crate::infra::error::AppError;

#[cfg(unix)]
fn link_dir(link: &Path, target: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn link_dir(link: &Path, target: &Path) {
    crate::test_support::create_junction(link, target);
}

#[test]
fn open_no_follow_opens_files_and_directories_but_not_links() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file.txt");
    std::fs::write(&file, "x").unwrap();
    let target = dir.path().join("target");
    std::fs::create_dir(&target).unwrap();
    let link = dir.path().join("link");
    link_dir(&link, &target);

    assert!(open_no_follow(&file).unwrap().metadata().unwrap().is_file());
    assert!(open_no_follow(&target)
        .unwrap()
        .metadata()
        .unwrap()
        .is_dir());
    assert!(open_no_follow(&link).is_err());
}

#[test]
fn same_file_compares_identity_not_names() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    std::fs::write(&a, "same").unwrap();
    std::fs::write(&b, "same").unwrap();

    let first = open_no_follow(&a).unwrap();
    assert!(same_file(&first, &open_no_follow(&a).unwrap()).unwrap());
    assert!(!same_file(&first, &open_no_follow(&b).unwrap()).unwrap());
}

#[test]
fn mode_bits_is_st_mode_on_unix_and_absent_on_windows() {
    let dir = tempfile::tempdir().unwrap();
    let metadata = std::fs::metadata(dir.path()).unwrap();
    #[cfg(unix)]
    assert_eq!(
        mode_bits(&metadata),
        Some(std::os::unix::fs::MetadataExt::mode(&metadata))
    );
    #[cfg(windows)]
    assert_eq!(mode_bits(&metadata), None);
}

#[test]
fn permission_snapshot_keeps_its_persisted_field_name_and_restores() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let target = dir.path().join("target");
    std::fs::write(&source, "x").unwrap();
    std::fs::write(&target, "x").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o751)).unwrap();
    }
    #[cfg(windows)]
    {
        let mut permissions = std::fs::metadata(&source).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&source, permissions).unwrap();
    }

    let snapshot = PermissionSnapshot::capture(std::fs::metadata(&source).unwrap().permissions());
    let json = serde_json::to_value(snapshot).unwrap();
    #[cfg(unix)]
    assert_eq!(json["mode"].as_u64().map(|mode| mode & 0o777), Some(0o751));
    #[cfg(windows)]
    assert_eq!(json, serde_json::json!({ "readonly": true }));
    assert_eq!(
        serde_json::from_value::<PermissionSnapshot>(json).unwrap(),
        snapshot
    );

    snapshot
        .restore(
            &std::fs::OpenOptions::new()
                .write(true)
                .open(&target)
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions(),
        std::fs::metadata(&source).unwrap().permissions()
    );
    #[cfg(windows)]
    #[allow(clippy::permissions_set_readonly_false)]
    // Windows 只清只读属性，不涉及 Unix 权限位。
    for path in [&source, &target] {
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(path, permissions).unwrap();
    }
}

#[cfg(unix)]
fn assert_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "{}", path.display());
}

#[cfg(windows)]
fn assert_private(path: &Path) {
    crate::test_support::assert_private_to_current_user(path);
}

#[cfg(unix)]
fn make_readable_by_others(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
}

#[cfg(windows)]
fn make_readable_by_others(path: &Path) {
    let granted = std::process::Command::new("icacls")
        .arg(path)
        .args(["/grant", "*S-1-1-0:R"])
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(granted.success());
}

fn temp_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            let name = path.file_name().unwrap().to_string_lossy();
            name.starts_with('.') && name.ends_with(".tmp")
        })
        .collect()
}

#[test]
fn private_write_temp_file_is_private_before_the_first_byte() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".env");

    write_atomic(&path, true, |writer| {
        let temps = temp_files(dir.path());
        assert_eq!(temps.len(), 1, "{temps:?}");
        assert_private(&temps[0]);
        writer.write_all(b"KEY=secret\n").map_err(AppError::Io)
    })
    .unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "KEY=secret\n");
    assert_private(&path);
}

#[test]
fn private_write_over_a_readable_file_stays_private() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp-oauth-tokens.json");
    std::fs::write(&path, "old").unwrap();
    make_readable_by_others(&path);

    write_private_file_atomic(&path, b"new").unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    assert_private(&path);
}

#[test]
fn private_write_failure_leaves_no_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("occupied");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("keep.txt"), "x").unwrap();

    let result = write_private_file_atomic(&path, b"secret");

    assert!(matches!(result, Err(AppError::Io(_))), "{result:?}");
    assert_eq!(temp_files(dir.path()), Vec::<PathBuf>::new());
}
