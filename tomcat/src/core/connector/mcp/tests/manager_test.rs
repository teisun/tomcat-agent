use std::sync::Arc;
use std::time::Duration;

use std::collections::{BTreeMap, BTreeSet};

use super::{tokenize, tool_name_summary, tool_search_score, McpManager, McpToolDef, ServerState};
use crate::core::connector::mcp::config::{add_global_server, McpServerConfig};
use crate::infra::config::get_work_dir;
use crate::AppConfig;

#[path = "naming_collision_test.rs"]
mod naming_collision_tests;
#[path = "session_test.rs"]
mod session_tests;

#[test]
fn manager_uses_one_runtime_for_global_and_workspace_sources() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    std::fs::create_dir_all(workspace.join(".agents")).unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    cfg.connector.mcp.startup_timeout_ms = 31_000;
    cfg.connector.mcp.call_timeout_ms = 91_000;
    cfg.connector.mcp.max_concurrent_calls = 3;
    add_global_server(
        &cfg,
        "global".into(),
        serde_json::from_value(serde_json::json!({"command":"node"})).unwrap(),
    )
    .unwrap();
    std::fs::write(
        workspace.join(".agents/mcp.json"),
        r#"{"mcpServers":{"project":{"command":"node"}}}"#,
    )
    .unwrap();
    let manager = McpManager::new(&cfg, Some(&workspace)).unwrap();
    assert_eq!(manager.statuses().len(), 2);
    assert_eq!(manager.runtime, cfg.connector.mcp);
    let mut invalid = cfg.clone();
    invalid.connector.mcp.max_concurrent_calls = 0;
    assert!(McpManager::new(&invalid, Some(&workspace)).is_err());
}

pub(super) fn manager_with_fake_server(
    args: Vec<String>,
    call_timeout_ms: u64,
) -> (tempfile::TempDir, Arc<McpManager>) {
    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    cfg.connector.mcp.call_timeout_ms = call_timeout_ms;
    let config_path = get_work_dir(&cfg).expect("work dir").join("mcp.json");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("config directory");
    std::fs::write(
        config_path,
        serde_json::json!({
            "mcpServers": {
                "fake": {
                    "command": "node",
                    "args": args,
                }
            }
        })
        .to_string(),
    )
    .expect("write MCP config");
    let manager = McpManager::new(&cfg, Some(&workspace)).expect("construct MCP manager");
    (temp, manager)
}

fn manager_with_untrusted_project_server() -> (tempfile::TempDir, Arc<McpManager>) {
    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    let config_path = workspace.join(".agents/mcp.json");
    std::fs::create_dir_all(config_path.parent().expect("project config parent"))
        .expect("project config directory");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mcp/fake_stdio_server.mjs");
    std::fs::write(
        config_path,
        serde_json::json!({
            "mcpServers": {
                "project-fake": {
                    "command": "node",
                    "args": [fixture],
                }
            }
        })
        .to_string(),
    )
    .expect("write untrusted project MCP config");

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let manager = McpManager::new(&cfg, Some(&workspace)).expect("construct MCP manager");
    (temp, manager)
}

pub(super) fn fake_server_args(extra: &[String]) -> Vec<String> {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mcp/fake_stdio_server.mjs");
    std::iter::once(fixture.to_string_lossy().into_owned())
        .chain(extra.iter().cloned())
        .collect()
}

#[tokio::test]
async fn same_name_global_and_workspace_connectors_have_distinct_config_keys() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    let global_path = temp.path().join("work/mcp.json");
    let project_path = workspace.join(".agents/mcp.json");
    std::fs::create_dir_all(global_path.parent().expect("global parent")).expect("global parent");
    std::fs::create_dir_all(project_path.parent().expect("project parent"))
        .expect("project parent");
    std::fs::write(
        &global_path,
        serde_json::json!({ "mcpServers": { "shared": { "command": "node" } } }).to_string(),
    )
    .expect("global config");
    std::fs::write(
        &project_path,
        serde_json::json!({ "mcpServers": { "shared": { "command": "bun" } } }).to_string(),
    )
    .expect("project config");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let manager = McpManager::new(&cfg, Some(&workspace)).expect("manager");
    let statuses = manager.statuses();
    assert_eq!(statuses.len(), 2);
    let global = statuses
        .iter()
        .find(|status| {
            status.source == crate::core::connector::mcp::config::McpConfigSource::Global
        })
        .expect("global status");
    let workspace_status = statuses
        .iter()
        .find(|status| {
            status.source == crate::core::connector::mcp::config::McpConfigSource::Project
        })
        .expect("workspace status");
    let global_key = global.config_key.clone();
    assert!(global.overridden);
    assert!(!workspace_status.overridden);
    assert!(manager.configured_server("shared").is_none());
    assert!(manager.configured_server(&global_key).is_some());
    assert!(manager
        .configured_server(&workspace_status.config_key)
        .is_some());
    assert!(manager.connect_server(&global_key).await.is_err());
    assert!(manager.list_tools(&global_key).is_err());

    assert!(manager
        .remove_configured_server(&workspace_status.config_key, &cfg)
        .expect("remove workspace override"));
    assert_eq!(
        manager
            .reload_configuration(&cfg)
            .expect("reload after removal"),
        vec![global_key.clone()],
    );
    assert!(manager
        .statuses()
        .into_iter()
        .find(|status| status.config_key == global_key)
        .is_some_and(|status| !status.overridden));
}

#[test]
fn removal_revokes_credentials_and_readd_uses_a_new_generation() {
    let (temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let configured = manager.configured_server("fake").expect("configured fake");
    let key = configured.config_key.clone();
    let old_generation = manager
        .entries
        .read()
        .get(&key)
        .expect("original entry")
        .generation;
    manager
        .save_static_bearer(&key, "secret".to_string(), None)
        .expect("save token");
    assert!(manager
        .oauth_store
        .load(&key)
        .expect("load token")
        .is_some());

    assert!(manager
        .remove_configured_server(&key, &cfg)
        .expect("remove connector"));
    assert!(manager
        .oauth_store
        .load(&key)
        .expect("revoked token")
        .is_none());
    assert!(manager.entries.read().get(&key).is_none());

    add_global_server(&cfg, "fake".to_string(), configured.config).expect("re-add connector");
    manager
        .reload_configuration(&cfg)
        .expect("reload re-added connector");
    assert!(
        manager
            .entries
            .read()
            .get(&key)
            .is_some_and(|entry| entry.generation > old_generation),
        "a re-added configKey must reject any pre-removal connection result",
    );
}

#[test]
fn removed_source_cannot_publish_old_generation() {
    let (temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let key = manager.configured_server("fake").unwrap().config_key;
    let old_generation = manager.current_generation(&key);
    assert!(manager.remove_configured_server(&key, &cfg).unwrap());
    manager.update_state_if_generation(&key, old_generation, ServerState::Ready, 2);
    assert!(manager.entries.read().get(&key).is_none());
    assert!(manager.current_generation(&key) > old_generation);
}

#[test]
fn configuration_reload_retirement_prevents_removed_key_reuse() {
    let (temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let configured = manager.configured_server("fake").expect("configured fake");
    let key = configured.config_key.clone();
    let old_generation = manager.current_generation(&key);

    std::fs::write(
        get_work_dir(&cfg).expect("work directory").join("mcp.json"),
        r#"{"mcpServers":{}}"#,
    )
    .expect("remove the config entry externally");
    manager
        .reload_configuration(&cfg)
        .expect("reload removed configuration");
    assert!(manager.entries.read().get(&key).is_none());

    add_global_server(&cfg, "fake".to_string(), configured.config).expect("re-add connector");
    manager
        .reload_configuration(&cfg)
        .expect("reload re-added configuration");
    assert!(manager
        .entries
        .read()
        .get(&key)
        .is_some_and(|entry| entry.generation > old_generation));
}

#[tokio::test]
async fn fake_stdio_server_lists_and_calls_tools() {
    let (_temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
    manager
        .connect_server("fake")
        .await
        .expect("connect fake server");
    let status = manager.statuses().pop().expect("server status");
    assert_eq!(status.state, ServerState::Ready);
    assert_eq!(status.tool_count, 2);
    let tool = manager
        .tool_defs("fake")
        .into_iter()
        .find(|tool| tool.raw_name == "capture")
        .expect("capture tool");
    assert_eq!(tool.model_name, "mcp__fake__capture");
    let result = manager
        .call_tool("fake", &tool.model_name, serde_json::json!({}))
        .await
        .expect("call fake tool");
    assert_eq!(result["content"][0]["text"], "fake capture complete");
    assert_eq!(result["content"][1]["type"], "image");
}

#[tokio::test]
async fn reload_keeps_an_unchanged_ready_connection_when_another_server_is_added() {
    let (temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
    manager
        .connect_server("fake")
        .await
        .expect("connect original server");
    let fake_key = manager
        .configured_server("fake")
        .expect("configured fake server")
        .config_key;
    let original = manager
        .entries
        .read()
        .get(&fake_key)
        .and_then(|entry| entry.connection.clone())
        .expect("ready original connection");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let added: McpServerConfig = serde_json::from_value(serde_json::json!({
        "command": "node", "args": ["does-not-start"]
    }))
    .expect("added server configuration");
    add_global_server(&cfg, "new-server".to_string(), added).expect("add another server");

    let changed = manager
        .reload_configuration(&cfg)
        .expect("incremental reload");
    assert_eq!(
        changed,
        vec![
            manager
                .configured_server("new-server")
                .expect("new server")
                .config_key,
        ]
    );
    let retained = manager
        .entries
        .read()
        .get(&fake_key)
        .and_then(|entry| entry.connection.clone())
        .expect("unchanged connection stays ready");
    assert!(Arc::ptr_eq(&original, &retained));
    assert_eq!(manager.tool_defs("fake").len(), 2);
}

#[tokio::test]
async fn changing_tool_filter_keeps_the_transport_and_updates_visible_tools() {
    let (temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
    manager
        .connect_server("fake")
        .await
        .expect("connect fake server");
    let key = manager
        .configured_server("fake")
        .expect("configured fake server")
        .config_key;
    let original = manager
        .entries
        .read()
        .get(&key)
        .and_then(|entry| entry.connection.clone())
        .expect("ready connection");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());

    manager
        .set_configured_tool_filter(
            &key,
            crate::core::connector::mcp::config::ToolFilter {
                include: vec!["capture".to_string()],
                exclude: vec![],
            },
            &cfg,
        )
        .expect("save tool filter");
    assert!(manager
        .reload_configuration(&cfg)
        .expect("reload tool filter")
        .is_empty());

    let retained = manager
        .entries
        .read()
        .get(&key)
        .and_then(|entry| entry.connection.clone())
        .expect("filter-only update retains connection");
    assert!(Arc::ptr_eq(&original, &retained));
    assert_eq!(manager.list_tools(&key).expect("filtered tools").len(), 1);
    assert_eq!(manager.tool_defs(&key).len(), 1);
    assert_eq!(manager.list_servers()[0].tool_count, 1);
    let management = manager
        .tool_catalog_snapshot(&key)
        .expect("complete management catalog");
    assert_eq!(
        management
            .tools
            .into_iter()
            .map(|tool| (tool.raw_name, tool.enabled))
            .collect::<Vec<_>>(),
        vec![("capture".to_string(), true), ("status".to_string(), false)],
        "Settings keeps the disabled row while every AI-facing projection remains filtered"
    );
}

#[tokio::test]
async fn single_tool_toggle_updates_the_catalog_and_blocks_model_calls() {
    let (temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
    manager
        .connect_server("fake")
        .await
        .expect("connect fake server");
    let key = manager
        .configured_server("fake")
        .expect("configured fake server")
        .config_key;
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());

    manager
        .set_configured_tool_enabled(&key, "capture", false, &cfg)
        .expect("disable one tool");
    let catalog = manager
        .tool_catalog_snapshot(&key)
        .expect("complete management catalog");
    assert_eq!(
        catalog
            .tools
            .into_iter()
            .map(|tool| (tool.raw_name, tool.enabled))
            .collect::<Vec<_>>(),
        vec![("capture".to_string(), false), ("status".to_string(), true)]
    );
    manager
        .call_model_tool("mcp__fake__capture", serde_json::json!({}))
        .await
        .expect_err("disabled tool must not be callable by the model");

    manager
        .set_configured_tool_enabled(&key, "capture", true, &cfg)
        .expect("re-enable one tool");
    assert_eq!(
        manager
            .call_model_tool("mcp__fake__capture", serde_json::json!({}))
            .await
            .expect("re-enabled tool must be callable")["content"][0]["text"],
        "fake capture complete"
    );
}

#[tokio::test]
async fn deferred_catalog_lists_searches_describes_and_calls_without_registry() {
    let (_temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
    manager
        .connect_server("fake")
        .await
        .expect("connect fake server");

    assert_eq!(
        manager.list_servers(),
        vec![super::McpToolSource {
            name: "fake".to_string(),
            source_type: "mcp",
            title: "tomcat-fake-mcp".to_string(),
            description: "Use fake tools for connector tests.".to_string(),
            tool_count: 2,
        }]
    );
    assert_eq!(
        manager
            .list_tools("fake")
            .expect("list ready source")
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>(),
        vec![
            "mcp__fake__capture".to_string(),
            "mcp__fake__status".to_string()
        ]
    );

    let matches = manager
        .search("capture", Some("fake"), 20, 0)
        .expect("search ready source");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].name, "mcp__fake__capture");
    assert_eq!(matches[0].source, "fake");
    let second_page = manager
        .search("fake", Some("fake"), 1, 1)
        .expect("page through deterministic results");
    assert_eq!(second_page.len(), 1);
    assert_eq!(second_page[0].name, "mcp__fake__capture");

    let names = vec![
        "mcp__fake__capture".to_string(),
        "mcp__fake__missing".to_string(),
        "mcp__fake__status".to_string(),
    ];
    let described = manager.describe_many(&names);
    assert_eq!(
        described
            .tools
            .iter()
            .map(|tool| tool.model_name.as_str())
            .collect::<Vec<_>>(),
        vec!["mcp__fake__capture", "mcp__fake__status"],
        "known schemas preserve caller order while unknown names do not poison the batch"
    );
    assert_eq!(described.errors.len(), 1);
    assert_eq!(described.errors[0].name, "mcp__fake__missing");

    let result = manager
        .call_model_tool("mcp__fake__capture", serde_json::json!({}))
        .await
        .expect("call canonical deferred name");
    assert_eq!(result["content"][0]["text"], "fake capture complete");
}

#[tokio::test]
async fn deferred_catalog_reports_unknown_and_not_ready_sources() {
    let (_temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
    assert!(manager.list_servers().is_empty());
    assert!(manager
        .list_tools("fake")
        .expect_err("configured but not-ready source must be actionable")
        .to_string()
        .contains("not ready"));
    assert!(manager
        .list_tools("missing")
        .expect_err("unknown source must not look like an empty source")
        .to_string()
        .contains("unknown MCP source"));
    assert!(manager
        .search("capture", Some("missing"), 20, 0)
        .expect_err("scoped search validates source")
        .to_string()
        .contains("unknown MCP source"));
}

#[tokio::test]
async fn deferred_call_cannot_bypass_untrusted_project_connector() {
    let (_temp, manager) = manager_with_untrusted_project_server();

    manager
        .connect_server("project-fake")
        .await
        .expect("untrusted source is recorded as awaiting confirmation, not connected");
    assert!(matches!(
        manager.statuses().pop().expect("source status").state,
        ServerState::AwaitingProjectTrust
    ));
    assert!(manager.list_servers().is_empty());

    let error = manager
        .call_model_tool("mcp__project-fake__capture", serde_json::json!({}))
        .await
        .expect_err("tool_call must not bypass project connector confirmation");
    assert!(
        error.to_string().contains(&crate::infra::i18n::tr_in(
            crate::infra::i18n::Locale::En,
            "mcp.deferredUnknown",
            &[("name", "mcp__project-fake__capture")]
        )),
        "unapproved sources must not expose a callable deferred tool: {error}"
    );
}

#[tokio::test]
async fn project_trust_gates_execution_once_and_survives_configuration_edits() {
    let (temp, manager) = manager_with_untrusted_project_server();
    let root = manager.project_root().unwrap().to_path_buf();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let store = crate::core::security::project_trust::ProjectTrustStore::open(&cfg).unwrap();
    assert!(!manager.project_trusted());
    assert_eq!(
        manager.statuses()[0].state,
        ServerState::AwaitingProjectTrust
    );
    manager.connect_server("project-fake").await.unwrap();
    assert!(manager.test_server("project-fake").await.is_err());
    assert!(manager.login_server("project-fake").await.is_err());
    assert!(manager.list_servers().is_empty());

    store.trust(&root).unwrap();
    assert_eq!(manager.wake_project().len(), 1);
    assert!(manager.wake_project().is_empty(), "idempotent wake");
    assert_eq!(manager.statuses()[0].state, ServerState::Pending);
    manager.connect_server("project-fake").await.unwrap();
    assert_eq!(manager.statuses()[0].state, ServerState::Ready);

    let project_path = root.join(".agents/mcp.json");
    let mut file: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&project_path).unwrap()).unwrap();
    file["mcpServers"]["project-fake"]["env"] = serde_json::json!({"RECONFIGURED":"yes"});
    std::fs::write(&project_path, file.to_string()).unwrap();
    manager.reload_configuration(&cfg).unwrap();
    manager.connect_server("project-fake").await.unwrap();
    assert_eq!(manager.statuses()[0].state, ServerState::Ready);

    let other = temp.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let unrelated = McpManager::new(&cfg, Some(&other)).unwrap();
    assert!(!unrelated.project_trusted());
}

#[test]
fn corrupt_project_trust_file_cannot_approve_workspace_sources() {
    let (temp, manager) = manager_with_untrusted_project_server();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let store = crate::core::security::project_trust::ProjectTrustStore::open(&cfg).unwrap();
    std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
    std::fs::write(store.path(), b"not json").unwrap();
    let second = McpManager::new(&cfg, manager.workspace_root()).unwrap();
    assert!(!second.project_trusted());
    assert_eq!(
        second.statuses()[0].state,
        ServerState::AwaitingProjectTrust
    );
    assert!(second.reload_configuration(&cfg).is_ok());
    assert!(store.trust(second.project_root().unwrap()).is_err());
}

#[test]
fn search_scoring_is_deterministic_and_prefers_name_matches() {
    let click = McpToolDef {
        server: "fake".to_string(),
        raw_name: "browser_click".to_string(),
        model_name: "mcp__fake__browser_click".to_string(),
        description: "Interact with page elements.".to_string(),
        input_schema: serde_json::json!({}),
    };
    let narrative = McpToolDef {
        server: "fake".to_string(),
        raw_name: "inspect".to_string(),
        model_name: "mcp__fake__inspect".to_string(),
        description: "Return a narrative that says click many times.".to_string(),
        input_schema: serde_json::json!({}),
    };
    let query = tokenize("click");
    assert!(
        tool_search_score(&click, "click", &query) > tool_search_score(&narrative, "click", &query),
        "canonical name matches rank ahead of description-only matches"
    );
    assert_eq!(
        tokenize("BrowserClick browser_click"),
        BTreeSet::from(["browser".to_string(), "click".to_string()])
    );
}

#[test]
fn tool_name_summary_is_stable_and_bounded() {
    let tool = |raw_name: &str| McpToolDef {
        server: "fake".to_string(),
        raw_name: raw_name.to_string(),
        model_name: format!("mcp__fake__{raw_name}"),
        description: String::new(),
        input_schema: serde_json::json!({}),
    };
    let tools = BTreeMap::from([
        ("mcp__fake__alpha".to_string(), tool("alpha")),
        ("mcp__fake__beta".to_string(), tool("beta")),
        ("mcp__fake__gamma".to_string(), tool("gamma")),
        ("mcp__fake__omega".to_string(), tool("omega")),
    ]);

    assert_eq!(
        tool_name_summary(&tools).as_deref(),
        Some("Provides tools: alpha, beta, gamma, and 1 more.")
    );
    assert_eq!(tool_name_summary(&BTreeMap::new()), None);
}

#[tokio::test]
async fn call_tool_timeout_preserves_shared_connection() {
    let (_temp, manager) = manager_with_fake_server(fake_server_args(&["--hang".to_string()]), 25);
    manager
        .connect_server("fake")
        .await
        .expect("connect fake server");
    let tool = manager.tool_defs("fake").pop().expect("discovered tool");

    let error = manager
        .call_tool("fake", &tool.model_name, serde_json::json!({}))
        .await
        .expect_err("hanging MCP tool should time out");

    assert!(error.to_string().contains("timed out"));
    assert!(matches!(
        manager.statuses().pop().expect("server status").state,
        ServerState::Ready
    ));
    assert!(!manager.tool_defs("fake").is_empty());
}

#[tokio::test]
async fn idle_stdio_service_exit_recovers_without_a_call_and_stops_at_three() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("methods");
    let (_manager_temp, manager) = manager_with_fake_server(
        fake_server_args(&[
            "--record".into(),
            log.to_string_lossy().into_owned(),
            "--exit-after-catalog-ms".into(),
            "1000".into(),
        ]),
        1000,
    );
    manager.connect_server("fake").await.unwrap();
    assert_eq!(manager.statuses()[0].state, ServerState::Ready);
    tokio::time::timeout(Duration::from_secs(8), async {
        while !matches!(manager.statuses()[0].state, ServerState::Failed(_)) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("idle service exit is observable without another tools/call");
    for _ in 0..3 {
        assert!(manager.connect_server("fake").await.is_err());
    }
    let log = std::fs::read_to_string(log).unwrap();
    assert_eq!(log.lines().filter(|line| *line == "initialize").count(), 3);
    assert_eq!(log.lines().filter(|line| *line == "tools/call").count(), 0);
}

#[tokio::test]
async fn transport_drop_recovers_without_replaying_call() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let call_log = temp.path().join("calls.log");
    let (_manager_temp, manager) = manager_with_fake_server(
        fake_server_args(&[
            "--die-midcall".to_string(),
            "--record".to_string(),
            call_log.to_string_lossy().into_owned(),
        ]),
        1_000,
    );
    manager
        .connect_server("fake")
        .await
        .expect("connect fake server");
    let tool = manager.tool_defs("fake").pop().expect("discovered tool");

    manager
        .call_tool("fake", &tool.model_name, serde_json::json!({}))
        .await
        .expect_err("connection drop should fail the in-flight call");

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = manager.statuses().pop().expect("status");
            if status.state == ServerState::Ready && status.attempt == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("concrete stdio service exit must recover");
    let methods = std::fs::read_to_string(&call_log).expect("read call log");
    assert_eq!(
        methods
            .lines()
            .filter(|method| *method == "tools/call")
            .count(),
        1,
        "the in-flight call must not be replayed"
    );
    assert_eq!(
        methods
            .lines()
            .filter(|method| *method == "initialize")
            .count(),
        2
    );
    assert_eq!(
        manager.statuses().pop().expect("server status").state,
        ServerState::Ready
    );
}

#[tokio::test]
async fn stale_failure_cannot_mutate_a_reconnected_entry() {
    let (_temp, manager) = manager_with_fake_server(fake_server_args(&[]), 1_000);
    manager
        .connect_server("fake")
        .await
        .expect("initial connection");
    let key = manager
        .configured_server("fake")
        .expect("configured fake server")
        .config_key;
    let stale_generation = manager.current_generation(&key);

    manager.reconnect_server(&key).await.expect("reconnection");
    manager.update_state_if_generation(
        &key,
        stale_generation,
        ServerState::Failed("late failure".to_string()),
        0,
    );
    manager.mark_disconnected(&key, stale_generation);

    assert!(matches!(
        manager.statuses().pop().expect("server status").state,
        ServerState::Ready
    ));
    assert!(!manager.tool_defs(&key).is_empty());
}

#[tokio::test]
async fn reconnect_refetches_tools_without_a_persistent_cache() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let request_log = temp.path().join("requests.log");
    let (_manager_temp, manager) = manager_with_fake_server(
        fake_server_args(&[
            "--record".to_string(),
            request_log.to_string_lossy().into_owned(),
        ]),
        1_000,
    );
    manager
        .connect_server("fake")
        .await
        .expect("initial connection");
    manager
        .reconnect_server("fake")
        .await
        .expect("reconnection");

    let methods = std::fs::read_to_string(&request_log).expect("read request log");
    assert_eq!(
        methods
            .lines()
            .filter(|method| *method == "tools/list")
            .count(),
        2,
        "each connection must refresh its tool catalog"
    );
}
