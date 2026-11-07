use serial_test::serial;

use super::*;

#[test]
fn manual_path_step_matches_the_platform_mechanism() {
    let dir = Path::new("/opt/tomcat/bin");
    let expected = if cfg!(windows) {
        ManualPathStep::AddUserPathEntry(dir.to_path_buf())
    } else {
        ManualPathStep::RunInShell("export PATH=\"/opt/tomcat/bin:$PATH\"".to_string())
    };
    assert_eq!(manual_path_step(dir), expected);
}

#[test]
#[serial(env_lock)]
fn install_user_command_creates_local_bin_with_a_copy_of_exe() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("target").join("debug").join("tomcat");
    let local_bin = dir.path().join(".local").join("bin");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let installed = install_user_command(&exe, &local_bin, "tomcat").expect("install copy");
    let link = installed.expect("should create canonical command entry");
    assert_eq!(
        link,
        local_bin.join(if cfg!(windows) {
            "tomcat.exe"
        } else {
            "tomcat"
        })
    );
    assert!(local_bin.is_dir(), "local bin should be created");

    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_file());
    assert_eq!(std::fs::read(&link).unwrap(), std::fs::read(&exe).unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&link).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
}

#[cfg(unix)]
#[test]
#[serial(env_lock)]
fn install_user_command_replaces_existing_symlink_target() {
    let dir = tempfile::tempdir().unwrap();
    let old_exe = dir.path().join("target").join("debug").join("tomcat-old");
    let new_exe = dir.path().join("target").join("debug").join("tomcat-new");
    let local_bin = dir.path().join(".local").join("bin");
    let link = local_bin.join("tomcat");
    std::fs::create_dir_all(old_exe.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&local_bin).unwrap();
    std::fs::write(&old_exe, b"old").unwrap();
    std::fs::write(&new_exe, b"new").unwrap();
    // Both a different target and the current exe must migrate; neither may be written through.
    for target in [&old_exe, &new_exe] {
        std::os::unix::fs::symlink(target, &link).unwrap();
        let installed =
            install_user_command(&new_exe, &local_bin, "tomcat").expect("refresh symlink");
        assert_eq!(installed.as_deref(), Some(link.as_path()));
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_file());
        assert_eq!(std::fs::read(&link).unwrap(), b"new");
        assert_eq!(std::fs::read(&old_exe).unwrap(), b"old");
        assert_eq!(std::fs::read(&new_exe).unwrap(), b"new");
        std::fs::remove_file(&link).unwrap();
    }
}

#[cfg(unix)]
#[test]
#[serial(env_lock)]
fn install_user_command_refreshes_a_different_unix_copy_atomically() {
    use std::io::Read;
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("target").join("debug").join("tomcat");
    let local_bin = dir.path().join(".local").join("bin");
    let existing = local_bin.join("tomcat");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&local_bin).unwrap();
    std::fs::write(&exe, b"debug-binary").unwrap();
    std::fs::write(&existing, b"real-user-install").unwrap();

    let mut old_handle = std::fs::File::open(&existing).unwrap();
    let installed = install_user_command(&exe, &local_bin, "tomcat").expect("refresh regular file");
    assert_eq!(installed.as_deref(), Some(existing.as_path()));
    assert_eq!(std::fs::read(&existing).unwrap(), b"debug-binary");
    let mut old_bytes = Vec::new();
    old_handle.read_to_end(&mut old_bytes).unwrap();
    assert_eq!(old_bytes, b"real-user-install");
    assert_eq!(sorted_file_names(&local_bin), ["tomcat"]);
    let modified = std::fs::metadata(&existing).unwrap().modified().unwrap();
    assert!(install_user_command(&exe, &local_bin, "tomcat")
        .unwrap()
        .is_none());
    assert_eq!(
        std::fs::metadata(&existing).unwrap().modified().unwrap(),
        modified
    );
}

#[cfg(unix)]
#[test]
fn install_user_command_replaces_a_dangling_symlink_with_a_copy() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("new-exe");
    let local_bin = dir.path().join("bin");
    let entry = local_bin.join("tomcat");
    std::fs::write(&exe, b"new").unwrap();
    std::fs::create_dir(&local_bin).unwrap();
    std::os::unix::fs::symlink(dir.path().join("missing-target"), &entry).unwrap();

    assert_eq!(
        install_user_command(&exe, &local_bin, "tomcat").unwrap(),
        Some(entry.clone())
    );
    assert!(std::fs::symlink_metadata(&entry)
        .unwrap()
        .file_type()
        .is_file());
    assert_eq!(std::fs::read(&entry).unwrap(), b"new");
}

#[test]
fn install_user_command_survives_deleting_the_build_dir() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    let exe = target.join("tomcat");
    let local_bin = dir.path().join("bin");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(&exe, b"build-output").unwrap();
    let entry = install_user_command(&exe, &local_bin, "tomcat")
        .unwrap()
        .unwrap();

    std::fs::remove_file(&exe).unwrap();
    std::fs::remove_dir(&target).unwrap();
    assert!(std::fs::symlink_metadata(&entry)
        .unwrap()
        .file_type()
        .is_file());
    assert_eq!(std::fs::read(&entry).unwrap(), b"build-output");
}

#[cfg(unix)]
#[test]
fn install_user_command_keeps_the_old_entry_when_the_copy_fails() {
    use std::os::unix::fs::PermissionsExt;

    if unsafe { libc::geteuid() } == 0 {
        eprintln!("requires an unprivileged user to exercise directory write denial");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("new-exe");
    let local_bin = dir.path().join("bin");
    let entry = local_bin.join("tomcat");
    std::fs::write(&exe, b"new").unwrap();
    std::fs::create_dir(&local_bin).unwrap();
    std::fs::write(&entry, b"old").unwrap();
    let permissions = std::fs::metadata(&local_bin).unwrap().permissions();
    std::fs::set_permissions(&local_bin, std::fs::Permissions::from_mode(0o555)).unwrap();
    let result = install_user_command(&exe, &local_bin, "tomcat");
    std::fs::set_permissions(&local_bin, permissions).unwrap();

    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(std::fs::read(&entry).unwrap(), b"old");
    assert_eq!(sorted_file_names(&local_bin), ["tomcat"]);
}

#[test]
fn install_user_command_compares_same_length_builds_across_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("new-exe");
    let bin = dir.path().join("bin");
    let mut bytes = vec![b'a'; 128 * 1024 + 1];
    std::fs::write(&exe, &bytes).unwrap();
    let entry = install_user_command(&exe, &bin, "tomcat").unwrap().unwrap();
    assert!(install_user_command(&exe, &bin, "tomcat")
        .unwrap()
        .is_none());
    *bytes.last_mut().unwrap() = b'b';
    std::fs::write(&exe, &bytes).unwrap();
    assert_eq!(
        install_user_command(&exe, &bin, "tomcat").unwrap(),
        Some(entry.clone())
    );
    assert_eq!(std::fs::read(entry).unwrap(), bytes);
}

#[test]
fn install_user_command_never_parks_or_removes_a_directory_entry() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("new-exe");
    let bin = dir.path().join("bin");
    let entry = bin.join(format!("tomcat{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&exe, b"new").unwrap();
    std::fs::create_dir_all(&entry).unwrap();
    std::fs::write(entry.join("keep"), b"user data").unwrap();
    assert!(install_user_command(&exe, &bin, "tomcat").is_err());
    assert_eq!(std::fs::read(entry.join("keep")).unwrap(), b"user data");
    assert_eq!(
        sorted_file_names(&bin),
        [entry.file_name().unwrap().to_string_lossy()]
    );
}

#[test]
fn install_user_command_concurrent_writers_publish_only_complete_copies() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    let sources: Vec<_> = (0..4)
        .map(|i| {
            let file = dir.path().join(format!("source-{i}"));
            std::fs::write(&file, vec![i; 128 * 1024]).unwrap();
            file
        })
        .collect();
    let results = std::thread::scope(|scope| {
        let writers: Vec<_> = sources
            .iter()
            .map(|source| {
                let bin = &bin;
                scope.spawn(move || install_user_command(source, bin, "tomcat"))
            })
            .collect();
        writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(results.iter().any(Result::is_ok), "{results:?}");
    let bytes = std::fs::read(bin.join(format!("tomcat{}", std::env::consts::EXE_SUFFIX))).unwrap();
    assert_eq!(bytes.len(), 128 * 1024);
    assert!(bytes[0] < 4 && bytes.iter().all(|byte| *byte == bytes[0]));
}

// A pipe keeps the real copy in progress deterministically, without sleeps in
// production, huge test binaries, or an installation-only test protocol.
#[test]
#[ignore = "subprocess helper for interrupted-copy tests"]
fn interrupted_copy_child() {
    let source = std::env::var_os("TOMCAT_COPY_TEST_SOURCE").expect("owned pipe");
    let entry = std::env::var_os("TOMCAT_COPY_TEST_ENTRY").expect("owned entry");
    create_entry(Path::new(&source), Path::new(&entry)).unwrap();
}

struct CopyChild(std::process::Child);
impl Drop for CopyChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn copy_child(source: &Path, entry: &Path) -> CopyChild {
    let test = format!(
        "{}::interrupted_copy_child",
        module_path!().split_once("::").unwrap().1
    );
    CopyChild(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", &test])
            .env("TOMCAT_COPY_TEST_SOURCE", source)
            .env("TOMCAT_COPY_TEST_ENTRY", entry)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    )
}

fn finish_interrupted_copy(mut child: CopyChild, bin: &Path, entry: &Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "copy exited before interruption"
        );
        let staged = std::fs::read_dir(bin).unwrap().any(|item| {
            let item = item.unwrap();
            item.file_name().to_string_lossy().ends_with(".tmp")
                && item.metadata().unwrap().len() >= 1024
        });
        if staged {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "copy never staged bytes"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
    assert_eq!(std::fs::read(entry).unwrap(), b"complete old executable");
    let source = bin.parent().unwrap().join("repair-source");
    std::fs::write(&source, b"complete new executable").unwrap();
    install_user_command(&source, bin, "tomcat").unwrap();
    assert_eq!(std::fs::read(entry).unwrap(), b"complete new executable");
}

#[cfg(unix)]
#[test]
fn install_user_command_interrupted_copy_keeps_the_old_entry_and_can_retry() {
    use std::io::Write;
    use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let entry = bin.join("tomcat");
    std::fs::write(&entry, b"complete old executable").unwrap();
    let pipe = root.path().join("source-pipe");
    let name = std::ffi::CString::new(pipe.as_os_str().as_bytes()).unwrap();
    // SAFETY: name is a live NUL-terminated path inside this disposable fixture.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let mut child = copy_child(&pipe, &entry);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut writer = loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&pipe)
        {
            Ok(writer) => break writer,
            Err(error) if error.raw_os_error() == Some(libc::ENXIO) => {
                assert!(child.0.try_wait().unwrap().is_none(), "copy child exited");
                assert!(
                    std::time::Instant::now() < deadline,
                    "copy child never opened pipe"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(error) => panic!("open owned pipe: {error}"),
        }
    };
    writer.write_all(&[b'n'; 1024]).unwrap();
    finish_interrupted_copy(child, &bin, &entry);
}

#[cfg(windows)]
#[tokio::test]
async fn install_user_command_interrupted_copy_keeps_the_old_entry_and_can_retry() {
    use tokio::io::AsyncWriteExt;
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let entry = bin.join("tomcat.exe");
    std::fs::write(&entry, b"complete old executable").unwrap();
    let pipe = format!(r"\\.\pipe\tomcat-copy-test-{}", uuid::Uuid::new_v4());
    let mut writer = tokio::net::windows::named_pipe::ServerOptions::new()
        .create(&pipe)
        .unwrap();
    let child = copy_child(Path::new(&pipe), &entry);
    tokio::time::timeout(std::time::Duration::from_secs(10), writer.connect())
        .await
        .unwrap()
        .unwrap();
    writer.write_all(&[b'n'; 1024]).await.unwrap();
    finish_interrupted_copy(child, &bin, &entry);
}

fn sorted_file_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[cfg(windows)]
#[test]
#[serial(env_lock)]
fn install_user_command_refreshes_a_different_windows_copy_and_keeps_an_identical_one() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("download").join("tomcat.exe");
    let local_bin = dir.path().join(".local").join("bin");
    let installed = local_bin.join("tomcat.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&local_bin).unwrap();
    std::fs::write(&exe, b"new-release").unwrap();
    std::fs::write(&installed, b"old-release").unwrap();

    let refreshed = install_user_command(&exe, &local_bin, "tomcat").expect("refresh copy");
    assert_eq!(refreshed.as_deref(), Some(installed.as_path()));
    assert_eq!(std::fs::read(&installed).unwrap(), b"new-release");
    assert_eq!(
        sorted_file_names(&local_bin),
        ["tomcat.exe"],
        "an old copy that is not running is removed right away"
    );

    let modified = std::fs::metadata(&installed).unwrap().modified().unwrap();
    let unchanged = install_user_command(&exe, &local_bin, "tomcat").expect("identical copy");
    assert!(
        unchanged.is_none(),
        "identical bytes must not be copied again"
    );
    assert_eq!(
        std::fs::metadata(&installed).unwrap().modified().unwrap(),
        modified
    );
}

#[cfg(windows)]
#[test]
#[serial(env_lock)]
fn install_user_command_cleans_parked_windows_copies_from_a_previous_install() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("download").join("tomcat.exe");
    let local_bin = dir.path().join(".local").join("bin");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&local_bin).unwrap();
    std::fs::write(&exe, b"release").unwrap();
    std::fs::write(local_bin.join("tomcat.exe"), b"release").unwrap();
    std::fs::write(local_bin.join("tomcat.exe.0123abcd.old"), b"parked").unwrap();
    std::fs::write(local_bin.join("notes.old"), b"user file").unwrap();

    let installed = install_user_command(&exe, &local_bin, "tomcat").expect("cleanup");
    assert!(installed.is_none());
    assert_eq!(
        sorted_file_names(&local_bin),
        ["notes.old", "tomcat.exe"],
        "only tomcat's own parked copies are removed"
    );
}

#[cfg(unix)]
struct ShellOverride(Option<std::ffi::OsString>);

#[cfg(unix)]
impl ShellOverride {
    fn new(shell: &str) -> Self {
        let saved = std::env::var_os("SHELL");
        std::env::set_var("SHELL", shell);
        Self(saved)
    }
}

#[cfg(unix)]
impl Drop for ShellOverride {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => std::env::set_var("SHELL", value),
            None => std::env::remove_var("SHELL"),
        }
    }
}

#[cfg(unix)]
#[test]
fn path_export_targets_cover_shell_profiles() {
    let home = Path::new("/tmp/tomcat-home");
    assert_eq!(
        path_export_targets("/bin/zsh", home),
        vec![home.join(".zprofile"), home.join(".zshrc")]
    );
    assert_eq!(
        path_export_targets("/bin/bash", home),
        vec![home.join(".bashrc")]
    );
    assert_eq!(
        path_export_targets("/bin/fish", home),
        vec![home.join(".profile")]
    );
}

#[cfg(unix)]
#[test]
fn prune_stale_lines_removes_only_target_exports() {
    let content = r#"
# Added by tomcat init
export PATH="/tmp/project/target/release:$PATH"
# Added by tomcat init
export PATH="$HOME/.local/bin:$PATH"
export PATH="/usr/local/bin:$PATH"
# Added by tomcat init
export PATH="/tmp/project/target/debug/deps:$PATH"
"#;
    let pruned = prune_stale_lines(content);
    assert!(
        !pruned.contains("/target/release") && !pruned.contains("/target/debug/deps"),
        "target exports should be removed, got:\n{pruned}"
    );
    assert!(
        pruned.contains("export PATH=\"$HOME/.local/bin:$PATH\"")
            && pruned.contains("export PATH=\"/usr/local/bin:$PATH\""),
        "valid exports should remain, got:\n{pruned}"
    );
}

#[cfg(unix)]
#[test]
#[serial(env_lock)]
fn add_user_bin_dir_to_path_writes_zprofile_and_zshrc_without_touching_zshenv() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let zshenv = home.join(".zshenv");
    std::fs::write(&zshenv, ". \"$HOME/.cargo/env\"\n").unwrap();
    let _home = crate::test_support::HomeOverride::new(home);
    let _shell = ShellOverride::new("/bin/zsh");

    assert_eq!(
        add_user_bin_dir_to_path(home),
        Ok(()),
        "PATH setup should succeed for zsh"
    );

    let zprofile = std::fs::read_to_string(home.join(".zprofile")).unwrap();
    let zshrc = std::fs::read_to_string(home.join(".zshrc")).unwrap();
    assert!(
        zprofile.contains("export PATH=\"$HOME/.local/bin:$PATH\"")
            && zshrc.contains("export PATH=\"$HOME/.local/bin:$PATH\""),
        "zprofile/zshrc should both receive the stable export"
    );
    assert_eq!(
        std::fs::read_to_string(&zshenv).unwrap(),
        ". \"$HOME/.cargo/env\"\n",
        ".zshenv should remain untouched"
    );
}

#[cfg(unix)]
#[test]
#[serial(env_lock)]
fn add_user_bin_dir_to_path_prunes_stale_target_exports_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let bashrc = home.join(".bashrc");
    std::fs::write(
        &bashrc,
        [
            "# Added by tomcat init",
            "export PATH=\"/tmp/project/target/release:$PATH\"",
            "# Added by tomcat init",
            "export PATH=\"/tmp/project/target/debug/deps:$PATH\"",
            "export PATH=\"/usr/local/bin:$PATH\"",
        ]
        .join("\n"),
    )
    .unwrap();
    let _home = crate::test_support::HomeOverride::new(home);
    let _shell = ShellOverride::new("/bin/bash");

    assert_eq!(
        add_user_bin_dir_to_path(home),
        Ok(()),
        "first PATH setup should succeed"
    );
    assert_eq!(
        add_user_bin_dir_to_path(home),
        Ok(()),
        "second PATH setup should stay idempotent"
    );

    let bashrc_content = std::fs::read_to_string(&bashrc).unwrap();
    assert!(
        !bashrc_content.contains("/target/"),
        "stale target exports should be pruned, got:\n{bashrc_content}"
    );
    assert_eq!(
        bashrc_content
            .matches("export PATH=\"$HOME/.local/bin:$PATH\"")
            .count(),
        1,
        "stable export should appear exactly once, got:\n{bashrc_content}"
    );
    assert!(
        bashrc_content.contains("export PATH=\"/usr/local/bin:$PATH\""),
        "user exports must be preserved, got:\n{bashrc_content}"
    );
}

#[cfg(windows)]
#[test]
fn merge_user_path_appends_once_and_keeps_existing_entries_in_order() {
    let dir = r"C:\Users\Tom\.local\bin";
    assert_eq!(merge_user_path("", dir).as_deref(), Some(dir));
    assert_eq!(
        merge_user_path(r"C:\A;;C:\B;", dir).as_deref(),
        Some(r"C:\A;C:\B;C:\Users\Tom\.local\bin")
    );
    assert_eq!(
        merge_user_path(r"C:\A;c:\users\tom\.LOCAL\bin\;C:\B", dir),
        None,
        "case and trailing separator differences are the same directory"
    );
}

#[cfg(windows)]
#[test]
fn merge_user_path_recognises_an_unexpanded_profile_entry() {
    let profile = std::env::var("USERPROFILE").expect("USERPROFILE");
    let dir = format!(r"{profile}\.local\bin");
    assert_eq!(
        merge_user_path(r"%USERPROFILE%\.local\bin;C:\B", &dir),
        None
    );
    assert_eq!(
        merge_user_path(r"%TOMCAT_TEST_UNSET_VAR%\.local\bin", &dir),
        Some(format!(r"%TOMCAT_TEST_UNSET_VAR%\.local\bin;{dir}"))
    );
}

#[cfg(windows)]
#[test]
fn add_to_user_path_preserves_the_registry_value_type() {
    use windows_registry::{Type, CURRENT_USER};

    struct RemoveKey(String);
    impl Drop for RemoveKey {
        fn drop(&mut self) {
            let _ = CURRENT_USER.remove_tree(&self.0);
        }
    }
    let subkey = format!(r"Software\TomcatTest-{}", uuid::Uuid::new_v4());
    let _cleanup = RemoveKey(subkey.clone());
    let key = CURRENT_USER.create(&subkey).unwrap();
    let dir = r"C:\Tom\.local\bin";

    key.set_expand_string("Path", r"%SystemRoot%\tools;C:\A")
        .unwrap();
    assert!(add_to_user_path(&subkey, dir).unwrap());
    let value = key.get_value("Path").unwrap();
    assert_eq!(value.ty(), Type::ExpandString);
    assert_eq!(
        String::try_from(value).unwrap(),
        r"%SystemRoot%\tools;C:\A;C:\Tom\.local\bin"
    );
    assert!(
        !add_to_user_path(&subkey, r"c:\tom\.local\bin\").unwrap(),
        "an existing entry must not be written twice"
    );

    key.set_string("Path", r"C:\A").unwrap();
    assert!(add_to_user_path(&subkey, dir).unwrap());
    assert_eq!(key.get_value("Path").unwrap().ty(), Type::String);

    key.remove_value("Path").unwrap();
    assert!(add_to_user_path(&subkey, dir).unwrap());
    let value = key.get_value("Path").unwrap();
    assert_eq!(value.ty(), Type::ExpandString);
    assert_eq!(String::try_from(value).unwrap(), dir);
}
