use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::locate_git_bash;

fn bash_under(root: &str) -> PathBuf {
    PathBuf::from(root).join("bin").join("bash.exe")
}

fn locate(exec_path: Option<&str>, env: &[(&str, &str)], existing: &[PathBuf]) -> Option<PathBuf> {
    let env: HashMap<String, OsString> = env
        .iter()
        .map(|(key, value)| ((*key).to_string(), OsString::from(value)))
        .collect();
    locate_git_bash(
        exec_path,
        |key| env.get(key).cloned(),
        |path: &Path| existing.iter().any(|candidate| candidate == path),
    )
}

#[test]
fn derives_git_root_from_exec_path() {
    let cases = [
        (
            "C:/Program Files/Git/mingw64/libexec/git-core\n",
            "C:/Program Files/Git",
        ),
        (
            "D:/scoop/apps/git/current/mingw64/libexec/git-core",
            "D:/scoop/apps/git/current",
        ),
        (
            "C:/Users/汤姆猫/AppData/Local/Programs/Git/mingw64/libexec/git-core",
            "C:/Users/汤姆猫/AppData/Local/Programs/Git",
        ),
    ];
    for (exec_path, root) in cases {
        let bash = bash_under(root);
        assert_eq!(
            locate(Some(exec_path), &[], std::slice::from_ref(&bash)),
            Some(bash),
            "{exec_path}"
        );
    }
}

#[cfg(windows)]
#[test]
fn derived_bash_path_is_displayed_with_native_separators() {
    let bash = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
    let found = locate(
        Some("C:/Program Files/Git/mingw64/libexec/git-core"),
        &[],
        std::slice::from_ref(&bash),
    )
    .unwrap();
    assert_eq!(found.display().to_string(), r"C:\Program Files\Git\bin\bash.exe");
}

#[test]
fn falls_back_to_standard_dirs_when_derived_root_has_no_bash() {
    let mingit = "D:/tools/mingit/mingw64/libexec/git-core";
    let program_files = bash_under("C:/Program Files/Git");
    let local = PathBuf::from("C:/Users/u/AppData/Local")
        .join("Programs")
        .join("Git")
        .join("bin")
        .join("bash.exe");
    let env = [
        ("ProgramFiles", "C:/Program Files"),
        ("LOCALAPPDATA", "C:/Users/u/AppData/Local"),
    ];

    assert_eq!(
        locate(Some(mingit), &env, &[program_files.clone(), local.clone()]),
        Some(program_files)
    );
    assert_eq!(
        locate(Some(mingit), &env, std::slice::from_ref(&local)),
        Some(local.clone())
    );
    assert_eq!(
        locate(None, &env, std::slice::from_ref(&local)),
        Some(local)
    );
}

#[test]
fn never_returns_wsl_bash_and_reports_missing() {
    let wsl = PathBuf::from("C:/Windows/System32/bash.exe");
    let env = [
        ("ProgramFiles", "C:/Program Files"),
        ("LOCALAPPDATA", "C:/Users/u/AppData/Local"),
        ("PATH", "C:/Windows/System32"),
        ("SystemRoot", "C:/Windows"),
    ];

    assert_eq!(locate(None, &env, std::slice::from_ref(&wsl)), None);
    assert_eq!(locate(Some("  "), &[], &[]), None);
}
