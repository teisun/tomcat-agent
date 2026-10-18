use super::super::*;

#[test]
fn system_info_has_os_and_arch() {
    let info = system_info();
    assert!(!info.os.is_empty());
    assert!(!info.arch.is_empty());
}

#[test]
fn current_dir_ok() {
    let r = current_dir();
    assert!(r.is_ok());
}

#[test]
fn read_file_utf8_missing_is_io_error() {
    let r = read_file_utf8(Path::new("/nonexistent/path/file.txt"));
    assert!(r.is_err());
}

#[test]
fn write_file_atomic_and_read_utf8() {
    let dir = std::env::temp_dir().join("tomcat_platform_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test_atomic.txt");
    let content = "hello 世界";
    write_file_atomic(&path, content.as_bytes()).unwrap();
    let read = read_file_utf8(&path).unwrap();
    assert_eq!(read, content);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn normalize_path_without_tilde() {
    let r = normalize_path("/tmp");
    assert!(r.is_ok());
    let r = normalize_path("relative");
    assert!(r.is_ok());
}

#[test]
fn normalize_path_with_tilde() {
    if crate::infra::platform::home_dir().is_some() {
        let r = normalize_path("~");
        assert!(r.is_ok());
    }
}

#[test]
fn read_file_utf8_invalid_utf8_returns_config_error() {
    let dir = std::env::temp_dir().join("tomcat_platform_utf8");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad_utf8.bin");
    std::fs::write(&path, [0xff, 0xfe]).unwrap();
    let r = read_file_utf8(&path);
    assert!(r.is_err());
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn write_file_atomic_no_parent_error() {
    let r = write_file_atomic(Path::new(""), b"x");
    assert!(r.is_err());
}

fn temp_file_names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with('.') && name.ends_with(".tmp"))
        .collect()
}

#[test]
fn write_file_atomic_with_removes_temp_file_when_writer_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    std::fs::write(&path, "original\n").unwrap();

    let r = write_file_atomic_with(&path, |writer| {
        writer.write_all(b"partial").map_err(AppError::Io)?;
        Err(AppError::Io(std::io::Error::from_raw_os_error(28)))
    });

    assert!(matches!(r, Err(AppError::Io(_))), "{r:?}");
    assert_eq!(temp_file_names(dir.path()), Vec::<String>::new());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "original\n");
}

#[test]
fn write_file_atomic_removes_temp_file_when_rename_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("occupied");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("keep.txt"), "x").unwrap();

    let r = write_file_atomic(&path, b"content");

    assert!(matches!(r, Err(AppError::Io(_))), "{r:?}");
    assert_eq!(temp_file_names(dir.path()), Vec::<String>::new());
    assert!(path.join("keep.txt").exists());
}

#[test]
fn normalize_path_expands_tilde_with_either_separator() {
    let dir = tempfile::tempdir().unwrap();
    let home = canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(home.join(".tomcat")).unwrap();
    let _home = crate::test_support::HomeOverride::new(&home);
    assert_eq!(normalize_path("~/.tomcat").unwrap(), home.join(".tomcat"));
    assert_eq!(normalize_path("~\\.tomcat").unwrap(), home.join(".tomcat"));
}

#[cfg(windows)]
#[test]
fn normalize_path_displays_missing_home_paths_with_native_separators() {
    let dir = tempfile::tempdir().unwrap();
    let home = canonicalize(dir.path()).unwrap();
    let _home = crate::test_support::HomeOverride::new(&home);
    let path = normalize_path("~/.tomcat/tomcat.config.toml").unwrap();
    assert_eq!(
        path.display().to_string(),
        format!(r"{}\.tomcat\tomcat.config.toml", home.display())
    );
}

#[test]
fn format_home_path_joins_home_relative_parts_with_forward_slashes() {
    let home = tempfile::tempdir().unwrap();
    let _home = crate::test_support::HomeOverride::new(home.path());
    assert_eq!(format_home_path(&home.path().join("a").join("b")), "~/a/b");
    assert_eq!(format_home_path(home.path()), "~");
}

#[test]
fn canonicalize_returns_plain_paths_and_long_paths_stay_usable() {
    let dir = tempfile::tempdir().unwrap();
    let canonical = canonicalize(dir.path()).unwrap();
    assert!(!canonical.to_string_lossy().starts_with(r"\\?\"));

    let mut long = canonical;
    while long.as_os_str().len() <= 300 {
        long.push("a-fairly-long-directory-name");
    }
    std::fs::create_dir_all(&long).unwrap();
    let file = long.join("x.txt");
    std::fs::write(&file, "ok").unwrap();
    let resolved = canonicalize(&file).unwrap();
    assert_eq!(std::fs::read_to_string(resolved).unwrap(), "ok");
}
