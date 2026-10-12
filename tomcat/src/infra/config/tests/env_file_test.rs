use crate::infra::config::{read_env_entries, write_env_entries};
use std::collections::BTreeMap;

#[test]
fn malformed_environment_file_does_not_echo_credentials_or_modify_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".env");
    let content = "=PRIVATE_CREDENTIAL_SENTINEL\n";
    std::fs::write(&path, content).unwrap();
    let error = read_env_entries(&path).unwrap_err();
    assert!(matches!(error, crate::AppError::Config(_)));
    let error = error.to_string();
    assert!(error.contains(".env"), "{error}");
    assert!(!error.contains("PRIVATE_CREDENTIAL_SENTINEL"), "{error}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
}

#[test]
fn localized_environment_comments_preserve_values_and_private_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".env");
    let vars = BTreeMap::from([
        ("FAKE_TEST_API_KEY".to_string(), "fixture-value".to_string()),
        (
            "HTTPS_PROXY".to_string(),
            "http://127.0.0.1:7890".to_string(),
        ),
    ]);
    write_env_entries(&path, &vars).unwrap();
    assert_eq!(read_env_entries(&path).unwrap(), vars);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("tomcat init"));
    assert!(text.contains("0600"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
