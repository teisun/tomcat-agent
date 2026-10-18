use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::find_executable_in;

fn find(
    dirs: &[&str],
    pathext: Option<&str>,
    candidates: &[&str],
    existing: &[PathBuf],
) -> Option<PathBuf> {
    let path = std::env::join_paths(dirs).unwrap();
    let pathext = pathext.map(OsString::from);
    find_executable_in(&path, pathext.as_deref(), candidates, |p: &Path| {
        existing.iter().any(|candidate| candidate == p)
    })
}

#[test]
fn earlier_path_directory_wins_over_later_candidates() {
    let first = PathBuf::from("first");
    let second = PathBuf::from("second");
    let fdfind = first.join(if cfg!(windows) {
        "fdfind.exe"
    } else {
        "fdfind"
    });
    let fd = second.join(if cfg!(windows) { "fd.exe" } else { "fd" });

    assert_eq!(
        find(
            &["first", "second"],
            None,
            &["fd", "fdfind"],
            &[fd, fdfind.clone()]
        ),
        Some(fdfind)
    );
}

#[cfg(unix)]
#[test]
fn unix_looks_up_the_bare_name_only() {
    let bare = PathBuf::from("bin").join("rg");
    let exe = PathBuf::from("bin").join("rg.exe");

    assert_eq!(
        find(&["bin"], None, &["rg"], &[exe.clone(), bare.clone()]),
        Some(bare)
    );
    assert_eq!(find(&["bin"], None, &["rg"], &[exe]), None);
}

#[cfg(windows)]
#[test]
fn windows_follows_pathext_order_and_ignores_extensionless_shims() {
    let bin = PathBuf::from("bin");
    let shim = bin.join("rg");
    let cmd = bin.join("rg.cmd");
    let exe = bin.join("rg.exe");

    assert_eq!(
        find(
            &["bin"],
            Some(".COM;.EXE;.BAT;.CMD"),
            &["rg"],
            &[shim.clone(), cmd.clone(), exe.clone()]
        ),
        Some(exe)
    );
    assert_eq!(
        find(&["bin"], None, &["rg"], &[shim.clone(), cmd.clone()]),
        Some(cmd)
    );
    assert_eq!(find(&["bin"], None, &["rg"], &[shim]), None);
}

#[cfg(windows)]
#[test]
fn windows_keeps_a_known_extension_as_is() {
    let bin = PathBuf::from("bin");
    let npx = bin.join("npx.cmd");

    assert_eq!(
        find(&["bin"], None, &["npx.cmd"], std::slice::from_ref(&npx)),
        Some(npx)
    );
    let python = bin.join("python3.11.exe");
    assert_eq!(
        find(
            &["bin"],
            None,
            &["python3.11"],
            std::slice::from_ref(&python)
        ),
        Some(python)
    );
}
