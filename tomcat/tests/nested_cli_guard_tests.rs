mod common;

use assert_cmd::Command;
use predicates::prelude::*;
use tomcat::{AppConfig, SessionManager};

fn cmd() -> Command {
    let mut c = assert_cmd::cargo::cargo_bin_cmd!("tomcat");
    c.env_remove("TOMCAT__LLM__DEFAULT_MODEL");
    c.env("TOMCAT__UI__LANGUAGE", "en");
    c
}

fn test_config(work_dir: &std::path::Path) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.to_string_lossy().to_string());
    cfg
}

#[test]
fn nested_models_is_allowed_but_service_lifecycle_and_installation_remain_blocked() {
    let english: serde_json::Value =
        serde_json::from_str(include_str!("../assets/i18n/en.json")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        common::serve::isolated_serve_command(home.path())
            .env("USERPROFILE", home.path())
            .env("TOMCAT_AGENT_ACTIVE", "1")
            .args(args)
            .output()
            .unwrap()
    };
    let models = run(&["serve", "models"]);
    assert!(models.status.success());
    assert!(
        serde_json::from_slice::<serde_json::Value>(&models.stdout).unwrap()["models"].is_array()
    );
    for args in [
        vec!["serve", "start"],
        vec!["serve", "stop"],
        vec!["serve", "restart"],
        vec!["self-install"],
        vec!["self-install", "--add-to-path"],
    ] {
        let output = run(&args);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains(english["cli.nestedGuard"].as_str().unwrap()));
    }
    assert!(!home.path().join(".tomcat/agents").exists());
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
        .env_remove("TOMCAT_AGENT_ACTIVE")
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

#[test]
fn isolated_cli_initializes_despite_parent_agent_and_model_overrides() {
    let home = tempfile::tempdir().expect("private home");
    let workspace = home.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    for (key, value) in [
        ("TOMCAT_AGENT_ACTIVE", "1"),
        ("TOMCAT__LLM__DEFAULT_MODEL", "host-sentinel-model"),
        ("OPENAI_API_KEY", "host-sentinel-key"),
    ] {
        std::env::set_var(key, value);
    }
    for args in [vec!["init"], vec!["session", "new", "--scope", "claw"]] {
        let output = common::serve::isolated_serve_command(home.path())
            .current_dir(&workspace)
            .stdin(std::process::Stdio::null())
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    fn check_files(dir: &std::path::Path) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                check_files(&path);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                let text = String::from_utf8_lossy(&bytes);
                assert!(!text.contains("host-sentinel-model"), "{}", path.display());
                assert!(!text.contains("host-sentinel-key"), "{}", path.display());
            }
        }
    }
    assert!(home.path().join(".tomcat/tomcat.config.toml").is_file());
    let mut cfg =
        tomcat::load_config_toml_file(&home.path().join(".tomcat/tomcat.config.toml")).unwrap();
    cfg.storage.work_dir = Some(home.path().join(".tomcat").to_string_lossy().into_owned());
    let sessions = SessionManager::new(tomcat::resolve_sessions_dir(&cfg).unwrap());
    assert!(sessions.current_session_id().unwrap().is_some());
    check_files(home.path());
    assert_eq!(std::env::var("TOMCAT_AGENT_ACTIVE").as_deref(), Ok("1"));
}
