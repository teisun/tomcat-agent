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
fn install_user_command_creates_local_bin_and_points_to_exe() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("target").join("debug").join("tomcat");
    let local_bin = dir.path().join(".local").join("bin");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"#!/bin/sh\n").unwrap();

    let installed = install_user_command(&exe, &local_bin, "tomcat").expect("install symlink");
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

    #[cfg(unix)]
    {
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            canonicalize(&exe).unwrap()
        );
    }
    #[cfg(windows)]
    {
        assert!(
            link.exists(),
            "windows fallback should copy an .exe-like target"
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
    std::os::unix::fs::symlink(&old_exe, &link).unwrap();

    let installed = install_user_command(&new_exe, &local_bin, "tomcat").expect("refresh symlink");
    assert_eq!(installed.as_deref(), Some(link.as_path()));
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        canonicalize(&new_exe).unwrap()
    );
}

#[cfg(unix)]
#[test]
#[serial(env_lock)]
fn install_user_command_preserves_existing_regular_file() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("target").join("debug").join("tomcat");
    let local_bin = dir.path().join(".local").join("bin");
    let existing = local_bin.join("tomcat");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&local_bin).unwrap();
    std::fs::write(&exe, b"debug-binary").unwrap();
    std::fs::write(&existing, b"real-user-install").unwrap();

    let installed =
        install_user_command(&exe, &local_bin, "tomcat").expect("preserve regular file");
    assert!(installed.is_none(), "regular file should not be replaced");
    assert_eq!(std::fs::read(&existing).unwrap(), b"real-user-install");
}

#[cfg(windows)]
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
