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
    if dirs::home_dir().is_some() {
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
