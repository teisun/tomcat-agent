use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn local_socket_excludes_duplicate_listeners_and_recovers_stale_endpoint() {
    use fs2::FileExt;
    let temp = tempfile::tempdir().unwrap();
    let directory = super::super::canonicalize(temp.path()).unwrap();
    prepare_directory(&directory).unwrap();
    let lock = std::fs::File::create(directory.join("serve.lock")).unwrap();
    lock.lock_exclusive().unwrap();
    let address = endpoint(&directory).unwrap();
    let mut listener = LocalListener::bind(&address).unwrap();
    assert!(
        LocalListener::bind(&address).is_err(),
        "another process cannot claim this endpoint"
    );
    let mut first_client = connect(&address).await.unwrap();
    let mut first_server = listener.accept().await.unwrap();
    let mut second_client = connect(&address).await.unwrap();
    let mut second_server = listener.accept().await.unwrap();
    first_client.write_all(b"one").await.unwrap();
    second_client.write_all(b"two").await.unwrap();
    let mut bytes = [0; 3];
    first_server.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"one");
    second_server.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"two");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&address).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    drop((
        first_client,
        second_client,
        first_server,
        second_server,
        listener,
    ));
    assert!(connect(&address).await.is_err());
    remove_stale_endpoint(&address).unwrap();
    let _replacement = LocalListener::bind(&address).unwrap();
}

#[cfg(unix)]
#[test]
fn stale_cleanup_rejects_regular_files_and_symlinks() {
    use fs2::FileExt;
    let temp = tempfile::tempdir().unwrap();
    let lock = std::fs::File::create(temp.path().join("serve.lock")).unwrap();
    lock.lock_exclusive().unwrap();
    let file = temp.path().join("not-a-socket");
    std::fs::write(&file, "keep me").unwrap();
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    for path in [&file, &link] {
        assert!(remove_stale_endpoint(path).is_err());
    }
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep me");
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[cfg(unix)]
#[test]
fn socket_path_limit_and_unsafe_directory_are_explicit_errors() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let too_long = temp.path().join("x".repeat(150));
    assert!(endpoint(&too_long)
        .unwrap_err()
        .to_string()
        .contains("invalid local service socket path"));
    let unsafe_parent = temp.path().join("public");
    std::fs::create_dir(&unsafe_parent).unwrap();
    std::fs::set_permissions(&unsafe_parent, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(prepare_directory(&unsafe_parent.join("service")).is_err());
    let link = temp.path().join("linked");
    std::os::unix::fs::symlink(temp.path(), &link).unwrap();
    assert!(prepare_directory(&link).is_err());
}
