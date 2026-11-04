//! Ordinary startup must supply defaults without running init or creating a registry.
use assert_cmd::Command;

fn list_sessions(home: &std::path::Path) -> Command {
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("tomcat");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("TOMCAT__") {
            cmd.env_remove(key);
        }
    }
    // Keep TOMCAT_AGENT_ACTIVE intact: listing is allowed by the nested CLI guard.
    cmd.env("HOME", home)
        .env("USERPROFILE", home)
        .env("TOMCAT__STORAGE__WORK_DIR", home.join(".tomcat"))
        .env("TOMCAT__UI__LANGUAGE", "en")
        .current_dir(home)
        .args(["session", "list", "--scope", "claw"]);
    cmd
}

#[test]
fn ordinary_startup_creates_missing_mcp_and_preserves_every_existing_file() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(".tomcat/mcp.json");
    assert!(!path.exists());
    list_sessions(home.path()).assert().success();
    let default = std::fs::read(&path).unwrap();
    let config: serde_json::Value = serde_json::from_slice(&default).unwrap();
    assert_eq!(config["mcpServers"]["playwright"]["command"], "npx");
    for contents in [
        default.as_slice(),
        b"",
        b"broken json",
        br#"{"mcpServers":{"custom":{"command":"custom"}}}"#,
    ] {
        std::fs::write(&path, contents).unwrap();
        list_sessions(home.path()).assert().success();
        assert_eq!(std::fs::read(&path).unwrap(), contents);
    }
}

#[test]
fn failed_default_mcp_write_does_not_block_startup() {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().join(".tomcat");
    // A directory at the sidecar lock path reliably rejects opening a writable file,
    // including on machines where root can bypass read-only directory permissions.
    std::fs::create_dir_all(work.join(".mcp.json.lock")).unwrap();
    list_sessions(home.path()).assert().success();
    assert!(!work.join("mcp.json").exists());
}
