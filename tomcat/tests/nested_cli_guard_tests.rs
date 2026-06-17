use assert_cmd::Command;
use predicates::prelude::*;
use tomcat::{AppConfig, SessionManager};

fn cmd() -> Command {
    let mut c = assert_cmd::cargo::cargo_bin_cmd!("tomcat");
    c.env_remove("TOMCAT__LLM__DEFAULT_MODEL");
    c
}

fn test_config(work_dir: &std::path::Path) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.to_string_lossy().to_string());
    cfg
}

#[test]
fn nested_session_new_is_rejected_with_nonzero_exit_and_no_state_change() {
    let home = tempfile::tempdir().expect("home tempdir");
    let work_dir = tempfile::tempdir().expect("work dir");
    let cfg = test_config(work_dir.path());
    let sessions_dir = tomcat::resolve_sessions_dir(&cfg).expect("resolve sessions dir");

    cmd()
        .current_dir(work_dir.path())
        .env("HOME", home.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.path())
        .env("TOMCAT_AGENT_ACTIVE", "1")
        .args(["session", "new", "--scope", "claw"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Refusing to run this Tomcat command inside an active Tomcat agent session",
        ));

    let mgr = SessionManager::new(sessions_dir);
    let store = mgr.load_store().expect("load store after blocked command");
    assert!(
        store.is_empty(),
        "blocked nested command must not mutate sessions store"
    );
}

#[test]
fn nested_session_list_remains_allowed() {
    let home = tempfile::tempdir().expect("home tempdir");
    let work_dir = tempfile::tempdir().expect("work dir");

    cmd()
        .current_dir(work_dir.path())
        .env("HOME", home.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.path())
        .env("TOMCAT_AGENT_ACTIVE", "1")
        .args(["session", "list", "--scope", "claw"])
        .assert()
        .success();
}

#[test]
fn session_new_still_succeeds_when_nested_env_is_absent() {
    let home = tempfile::tempdir().expect("home tempdir");
    let work_dir = tempfile::tempdir().expect("work dir");
    let cfg = test_config(work_dir.path());
    let sessions_dir = tomcat::resolve_sessions_dir(&cfg).expect("resolve sessions dir");

    cmd()
        .current_dir(work_dir.path())
        .env("HOME", home.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.path())
        .args(["session", "new", "--scope", "claw"])
        .assert()
        .success();

    let mgr = SessionManager::new(sessions_dir);
    assert!(
        mgr.current_session_id()
            .expect("read current session id")
            .is_some(),
        "env-absent command should preserve existing behavior and create a session"
    );
}
