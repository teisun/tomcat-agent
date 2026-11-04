use super::materialize_default_mcp_json;
use crate::AppConfig;

#[test]
fn materializes_cursor_style_pinned_playwright_config_idempotently() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let path = materialize_default_mcp_json(&cfg).expect("materialize config");
    let first = std::fs::read_to_string(&path).expect("read generated config");
    let parsed: serde_json::Value = serde_json::from_str(&first).expect("valid JSON");
    assert_eq!(
        parsed["mcpServers"]["playwright"]["command"],
        serde_json::Value::String("npx".to_string())
    );
    assert!(parsed["mcpServers"]["playwright"]["args"][1]
        .as_str()
        .expect("pinned package")
        .starts_with("@playwright/mcp@"));
    for legacy in [
        "trusted",
        "integrity",
        "startupTimeoutMs",
        "callTimeoutMs",
        "maxConcurrentCalls",
    ] {
        assert!(
            parsed["mcpServers"]["playwright"].get(legacy).is_none(),
            "{legacy}"
        );
    }
    let expected_browser_path = crate::infra::config::get_work_dir(&cfg)
        .expect("normalized work directory")
        .join("cache")
        .join("playwright")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        parsed["mcpServers"]["playwright"]["env"]["PLAYWRIGHT_BROWSERS_PATH"].as_str(),
        Some(expected_browser_path.as_str())
    );
    materialize_default_mcp_json(&cfg).expect("idempotent materialization");
    assert_eq!(
        std::fs::read_to_string(path).expect("re-read config"),
        first
    );
}

#[test]
fn preserves_existing_empty_broken_and_custom_files_even_when_disabled() {
    for contents in [
        "",
        "not json",
        r#"{"mcpServers":{}}"#,
        r#"{"mcpServers":{"custom":{"command":"custom"}}}"#,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().to_string_lossy().into_owned());
        cfg.connector.enabled = false;
        cfg.connector.disabled = vec!["playwright".into()];
        let path = temp.path().join("mcp.json");
        std::fs::write(&path, contents).unwrap();
        materialize_default_mcp_json(&cfg).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), contents.as_bytes());
    }
}

#[test]
fn disabled_connectors_still_materialize_missing_defaults() {
    for disabled_globally in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().to_string_lossy().into_owned());
        cfg.connector.enabled = !disabled_globally;
        cfg.connector.disabled = vec!["playwright".into()];
        let path = materialize_default_mcp_json(&cfg).unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(json["mcpServers"]["playwright"]["command"], "npx");
    }
}

#[test]
fn materialization_waits_for_writer_and_rechecks_under_the_same_lock() {
    use std::sync::mpsc;
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().to_string_lossy().into_owned());
    let path = temp.path().join("mcp.json");
    let lock = crate::core::connector::mcp::config::acquire_mcp_file_lock(&path).unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        done_tx.send(materialize_default_mcp_json(&cfg)).unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let waiting = done_rx.recv_timeout(Duration::from_millis(100));
    let custom = br#"{"mcpServers":{"custom":{"command":"custom"}}}"#;
    std::fs::write(&path, custom).unwrap();
    drop(lock);
    assert!(matches!(waiting, Err(mpsc::RecvTimeoutError::Timeout)));
    done_rx
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    worker.join().unwrap();
    assert_eq!(std::fs::read(path).unwrap(), custom);
}

#[test]
fn materialization_reports_an_unwritable_parent() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("not-a-directory");
    std::fs::write(&parent, "occupied").unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(parent.to_string_lossy().into_owned());
    assert!(materialize_default_mcp_json(&cfg).is_err());
    assert_eq!(std::fs::read_to_string(parent).unwrap(), "occupied");
}

#[test]
fn preserves_a_legacy_file_instead_of_replacing_it() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let path = crate::core::connector::mcp::config::global_mcp_path(&cfg)
        .expect("global MCP configuration path");
    std::fs::create_dir_all(path.parent().expect("config parent")).expect("config parent");
    let legacy = r#"{"mcpServers":{"playwright":{"command":"npx","startupTimeoutMs":60000,"trusted":true}}}"#;
    std::fs::write(&path, legacy).expect("write legacy file");
    materialize_default_mcp_json(&cfg).expect("do not overwrite legacy file");
    assert_eq!(
        std::fs::read_to_string(path).expect("read legacy file"),
        legacy
    );
}
