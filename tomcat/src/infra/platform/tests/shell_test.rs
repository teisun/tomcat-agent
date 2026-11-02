use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{locate_git_bash, shell_path_to_native};

#[test]
fn git_probe_preserves_successful_output_and_nonzero_exit() {
    for (exit, expected) in [(0, Some("fixture/git-core")), (7, None)] {
        let mut command = std::process::Command::new("node");
        command.args([
            "-e",
            &format!("process.stdout.write(' fixture/git-core\\n');process.exit({exit})"),
        ]);
        let output =
            super::probe_git_exec_path(command, std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(output.as_deref(), expected);
    }
}

#[test]
fn git_probe_deadline_reaps_the_process_and_its_pipe_holding_child() {
    let dir = tempfile::tempdir().unwrap();
    let pids_path = dir.path().join("pids.json");
    // The probe never completes. A test-only fail-safe bounds the red run of the
    // old Command::output path; production must kill both processes well before it.
    let script = format!(
        r#"
        const fs = require('node:fs');
        const child = require('node:child_process').spawn(process.execPath, ['-e',
            'setInterval(()=>{{}},1000);setTimeout(()=>process.exit(),8000)'], {{stdio:'inherit'}});
        fs.writeFileSync({}, JSON.stringify([process.pid,child.pid]));
        setInterval(()=>{{}},1000);setTimeout(()=>process.exit(),8000);
    "#,
        serde_json::to_string(&pids_path).unwrap()
    );
    let mut command = std::process::Command::new("node");
    command.args(["-e", &script]);
    let started = std::time::Instant::now();
    let error = super::probe_git_exec_path(command, std::time::Duration::from_secs(3))
        .expect_err("probe must time out, not wait for the fixture fail-safe");
    assert!(error.contains("timed out"), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(6));
    let pids: Vec<u32> = serde_json::from_slice(&std::fs::read(pids_path).unwrap()).unwrap();
    assert_eq!(pids.len(), 2);
    let status = std::process::Command::new("node")
        .args([
            "-e",
            r#"
        for (const pid of process.argv.slice(1)) {
            try { process.kill(Number(pid), 0); throw Error('probe still alive: '+pid); }
            catch (error) { if (error.code !== 'ESRCH') throw error; }
        }
    "#,
        ])
        .args(pids.iter().map(ToString::to_string))
        .status()
        .unwrap();
    assert!(
        status.success(),
        "both probe processes must be gone on return"
    );
}

#[test]
fn shell_path_to_native_maps_only_git_bash_drive_mounts() {
    for unchanged in ["/tmp/x", "/cdrom/x", "relative/x"] {
        assert_eq!(shell_path_to_native(unchanged), unchanged);
    }
    let (drive, drive_root) = if cfg!(windows) {
        (r"C:\Users\a", r"D:\")
    } else {
        ("/c/Users/a", "/d")
    };
    assert_eq!(shell_path_to_native("/c/Users/a"), drive);
    assert_eq!(shell_path_to_native("/d"), drive_root);
}

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
    assert_eq!(
        found.display().to_string(),
        r"C:\Program Files\Git\bin\bash.exe"
    );
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
