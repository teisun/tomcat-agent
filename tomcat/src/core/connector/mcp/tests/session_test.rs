use super::super::{McpToolView, SessionMcpConfig};
use super::*;
use crate::core::connector::mcp::config::McpConfigSource;
use crate::infra::platform::EnvironmentSnapshot;

fn setup() -> (tempfile::TempDir, AppConfig, Arc<McpManager>) {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    std::fs::create_dir_all(temp.path().join(".agents")).unwrap();
    std::fs::write(
        temp.path().join(".agents/mcp.json"),
        r#"{"mcpServers":{"project":{"command":"node"}}}"#,
    )
    .unwrap();
    let shared = McpManager::new(&cfg, Some(temp.path())).unwrap();
    (temp, cfg, shared)
}

fn config(name: &str) -> SessionMcpConfig {
    SessionMcpConfig(BTreeMap::from([(
        name.into(),
        serde_json::from_value(
            serde_json::json!({"command":"node","args":fake_server_args(&[]),
            "env":{"PRIVATE_SECRET":"never-log-me"}}),
        )
        .unwrap(),
    )]))
}

#[tokio::test]
async fn session_mcp_is_not_project_trust_and_never_grants_project_trust() {
    let (temp, cfg, shared) = setup();
    let private = config("private")
        .prepare(
            &cfg,
            temp.path(),
            "A",
            Some(&shared),
            Some(Arc::new(EnvironmentSnapshot::capture().unwrap())),
        )
        .unwrap()
        .unwrap();
    let project = shared.configured_server("project").unwrap();
    assert!(!shared.project_trusted_for(&project));
    assert_eq!(
        shared.statuses()[0].state,
        ServerState::AwaitingProjectTrust
    );
    shared.connect_server("project").await.unwrap();
    assert!(shared.list_tools("project").is_err());
    private.ready_session_servers().await.unwrap();
    assert_eq!(private.statuses()[0].source, McpConfigSource::Session);
    assert_eq!(private.statuses()[0].state, ServerState::Ready);
    let view = McpToolView::new(shared.clone(), Some(private.clone()));
    assert!(!view.list_tools("private").unwrap().is_empty());
    assert!(!view
        .describe_many(&["mcp__private__capture".into()])
        .tools
        .is_empty());
    assert!(!shared.project_trusted());
    assert_eq!(
        shared.statuses()[0].state,
        ServerState::AwaitingProjectTrust
    );
    assert!(McpToolView::new(shared.clone(), None)
        .list_tools("private")
        .is_err());
    private.stop_session_servers().await;
    assert_eq!(
        shared.statuses()[0].state,
        ServerState::AwaitingProjectTrust
    );
}

#[tokio::test]
async fn session_mcp_rejects_initial_collision_and_hides_later_collision() {
    let (temp, cfg, shared) = setup();
    let env = Some(Arc::new(EnvironmentSnapshot::capture().unwrap()));
    assert!(config("project")
        .prepare(&cfg, temp.path(), "A", Some(&shared), env.clone())
        .is_err());
    let private = config("private")
        .prepare(&cfg, temp.path(), "A", Some(&shared), env)
        .unwrap()
        .unwrap();
    private.ready_session_servers().await.unwrap();
    let view = McpToolView::new(shared.clone(), Some(private.clone()));
    assert!(!view.search("capture", None, 20, 0).unwrap().is_empty());
    add_global_server(
        &cfg,
        "private".into(),
        serde_json::from_value(serde_json::json!({"command":"node"})).unwrap(),
    )
    .unwrap();
    shared.reload_configuration(&cfg).unwrap();
    assert!(view.list_tools("private").is_err());
    assert!(view.list_servers().iter().all(|s| s.name != "private"));
    assert!(view.search("capture", None, 20, 0).unwrap().is_empty());
    assert!(view
        .describe_many(&["mcp__private__capture".into()])
        .tools
        .is_empty());
    assert!(private.reload_configuration(&cfg).is_err());
    assert!(private.remove_configured_server("private", &cfg).is_err());
    private.stop_session_servers().await;
    // Disconnection must not redirect the same deferred tool name to shared config.
    assert!(view.list_tools("private").is_err());
}

#[test]
fn session_mcp_requires_runtime_and_environment_but_empty_setup_clears_private_state() {
    let (temp, mut cfg, shared) = setup();
    let private = config("private");
    assert!(!format!("{private:?}").contains("never-log-me"));
    let env = Some(Arc::new(EnvironmentSnapshot::capture().unwrap()));
    assert!(private
        .prepare(&cfg, temp.path(), "A", None, env.clone())
        .is_err());
    assert!(private
        .prepare(&cfg, temp.path(), "A", Some(&shared), None)
        .is_err());
    cfg.connector.enabled = false;
    assert!(private
        .prepare(&cfg, temp.path(), "A", Some(&shared), env)
        .is_err());
    assert!(SessionMcpConfig::default()
        .prepare(&cfg, temp.path(), "A", None, None)
        .unwrap()
        .is_none());
}
