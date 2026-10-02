use super::*;
use std::fs;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serial_test::serial;

fn write_session_plugin_fixture(workspace: &std::path::Path, plugin_id: &str) {
    let plugin_dir = workspace.join(".agents").join("plugins").join(plugin_id);
    fs::create_dir_all(&plugin_dir).expect("create plugin fixture dir");
    let manifest = serde_json::json!({
        "id": plugin_id,
        "name": plugin_id,
        "version": "0.1.0",
        "description": format!("fixture {plugin_id}"),
        "author": "tests",
        "main": "main.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": [],
        "tools": [],
        "events": ["session_start"],
        "activation": "session"
    });
    fs::write(
        plugin_dir.join("plugin.json"),
        serde_json::to_string_pretty(&manifest).expect("serialize plugin manifest"),
    )
    .expect("write plugin manifest");
    fs::write(
        plugin_dir.join("main.js"),
        r#"
pi.on("session_start", function () {});
__pi_start_event_loop();
"#,
    )
    .expect("write plugin main");
}

async fn wait_for_line(
    buffer: &crate::api::serve::test_support::SharedWriterBuffer,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> Vec<serde_json::Value> {
    for _ in 0..50 {
        let lines = read_ndjson_lines(buffer);
        if lines.iter().any(&predicate) {
            return lines;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    read_ndjson_lines(buffer)
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_initialize_control_request_sets_ready_state() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;
    state.initialized.store(false, Ordering::SeqCst);

    let handled = handle_control_or_interrupt(
        Arc::clone(&state),
        ServeCommand::ControlRequest {
            request_id: "init-1".to_string(),
            subtype: "initialize".to_string(),
            session_id: None,
            payload: serde_json::Value::Null,
        },
    )
    .await
    .unwrap();
    assert!(handled);
    assert!(state.initialized.load(Ordering::SeqCst));

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("control_response")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("control_response")
        })
        .unwrap();
    assert_eq!(
        response
            .get("requestId")
            .and_then(serde_json::Value::as_str),
        Some("init-1")
    );
    let payload = response.get("payload").unwrap();
    assert_eq!(
        payload
            .get("protocolVersion")
            .and_then(serde_json::Value::as_i64),
        Some(2)
    );
    assert_eq!(
        payload
            .get("serverVersion")
            .and_then(serde_json::Value::as_str),
        Some(env!("CARGO_PKG_VERSION"))
    );
    let capabilities = payload["capabilities"]
        .as_array()
        .expect("capabilities array");
    let capability_names = capabilities
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>();
    for expected in [
        "prompt",
        "steer",
        "follow_up",
        "get_state",
        "set_plan_mode",
        "set_model",
        "set_thinking_level",
        "set_speed",
        "set_context_window",
        "list_models",
        "upsert_model",
        "remove_model",
        "set_provider_key",
        "list_provider_keys",
        "list_connectors",
        "list_connector_tools",
        "add_connector",
        "remove_connector",
        "get_project_trust",
        "trust_project",
        "test_connector",
        "reload_connector",
        "set_connector_tool_filter",
        "set_connector_tool_enabled",
        "new_session",
        "switch_session",
        "get_messages",
        "ingest_attachment",
        "retain_attachment_leases",
        "cache_attachment_thumbnail",
        "discard_detached_session",
        "close_session",
        "list_sessions",
        "interrupt",
        "ask_question",
    ] {
        assert!(
            capability_names.contains(&expected),
            "missing capability {expected:?} in {capability_names:?}"
        );
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_connector_commands_keep_list_light_and_tools_on_demand() {
    const CONNECTOR_TEST_API_KEY_ENV: &str = "TOMCAT_SERVE_CONNECTOR_TEST_API_KEY";

    let _api_key = EnvGuard::set(CONNECTOR_TEST_API_KEY_ENV, "test-key");
    let temp = tempfile::tempdir().expect("temporary directory");
    let mut cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    cfg.workspace.project_resource_dir = ".workspace-data".to_string();
    cfg.connector.enabled = true;
    fs::write(
        temp.path().join("models.toml"),
        format!(
            r#"[[models]]
id = "gpt-5.4"
model_name = "gpt-5.4"
api = "openai-responses"
provider = "serve-connector-test"
api_key_env = "{CONNECTOR_TEST_API_KEY_ENV}"
base_url = "http://127.0.0.1:1"
capabilities = {{ vision = true, files = true, tools = true, reasoning = true, web_search = false }}
"#
        ),
    )
    .expect("write isolated connector test model");
    let (state, buffer, _temp, slot) = build_initialized_state_with_config(temp, cfg).await;
    let session_connector = slot
        .ctx
        .global_services
        .connector_registry
        .as_ref()
        .expect("connector registry")
        .clone();
    let workspace_root = session_connector
        .mcp_manager()
        .workspace_root()
        .expect("session workspace root")
        .to_path_buf();
    let connector_context = crate::api::serve::types::ConnectorContext {
        workspace_root: Some(workspace_root.to_string_lossy().into_owned()),
    };
    handle_command(
        Arc::clone(&state),
        ServeCommand::ListConnectors {
            id: Some("list-empty-connectors".to_string()),
            context: connector_context.clone(),
        },
    )
    .await
    .expect("list empty connectors");
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("list-empty-connectors")
    })
    .await;
    let empty_listed = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("list-empty-connectors")
        })
        .expect("empty list response");
    assert_eq!(empty_listed["payload"]["connectors"], serde_json::json!([]));
    assert!(
        empty_listed["payload"]["configPaths"]["workspace"]["raw"]
            .as_str()
            .is_some_and(|path| path.ends_with(".workspace-data/mcp.json")),
        "an empty connector list must still provide the actual workspace config path",
    );

    assert_eq!(empty_listed["payload"]["project"]["trusted"], false);
    let project_root = empty_listed["payload"]["project"]["root"].as_str().unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::GetProjectTrust {
            id: Some("trust-before-add".into()),
            path: workspace_root.to_string_lossy().into_owned(),
        },
    )
    .await
    .unwrap();
    let trust_status = wait_for_line(&buffer, |line| line["id"] == "trust-before-add").await;
    let trust_status = trust_status
        .iter()
        .find(|line| line["id"] == "trust-before-add")
        .unwrap();
    assert_eq!(trust_status["payload"]["projectRoot"], project_root);
    assert_eq!(trust_status["payload"]["trusted"], false);

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mcp/fake_stdio_server.mjs");
    let startup_gate = _temp.path().join("connector-startup-gate");
    fs::write(&startup_gate, "open").unwrap();

    handle_command(
        Arc::clone(&state),
        ServeCommand::AddConnector {
            id: Some("add-connector".to_string()),
            name: "fake".to_string(),
            command: "node".to_string(),
            args: vec![
                fixture.to_string_lossy().into_owned(),
                "--startup-gate-file".into(),
                startup_gate.to_string_lossy().into_owned(),
            ],
            url: None,
            headers: Default::default(),
            oauth: None,
            env: Default::default(),
            auth: None,
            scope: crate::api::serve::types::ConnectorScope::Workspace,
            context: connector_context.clone(),
            trust_project: true,
        },
    )
    .await
    .expect("add connector");
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("add-connector")
    })
    .await;
    let added = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("add-connector"))
        .expect("add response");
    assert_eq!(added["success"], true);
    assert_eq!(added["payload"]["configSaved"], true);
    assert_eq!(added["payload"]["connectionStarted"], true);
    assert!(
        Arc::ptr_eq(
            &session_connector,
            &crate::core::connector::ConnectorRegistry::new(&state.cfg, Some(&workspace_root))
                .expect("settings connector registry"),
        ),
        "settings and chat must resolve the same context registry",
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let connected = session_connector
                .mcp_manager()
                .statuses()
                .into_iter()
                .any(|status| status.name == "fake" && status.state.code() == "connected");
            if connected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("background connector connection completes");

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetProjectTrust {
            id: Some("trust-after-add".into()),
            path: workspace_root.to_string_lossy().into_owned(),
        },
    )
    .await
    .unwrap();
    let trust_after = wait_for_line(&buffer, |line| line["id"] == "trust-after-add").await;
    let trust_after = trust_after
        .iter()
        .find(|line| line["id"] == "trust-after-add")
        .unwrap();
    assert_eq!(trust_after["payload"]["trusted"], true);
    handle_command(
        Arc::clone(&state),
        ServeCommand::TrustProject {
            id: Some("reject-child".into()),
            project_root: workspace_root
                .join(".workspace-data/..")
                .to_string_lossy()
                .into_owned(),
        },
    )
    .await
    .unwrap();
    let rejected = wait_for_line(&buffer, |line| line["id"] == "reject-child").await;
    let rejected = rejected
        .iter()
        .find(|line| line["id"] == "reject-child")
        .unwrap();
    assert_eq!(rejected["success"], false);

    handle_command(
        Arc::clone(&state),
        ServeCommand::ListConnectors {
            id: Some("list-connectors".to_string()),
            context: connector_context.clone(),
        },
    )
    .await
    .expect("list connectors");
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("list-connectors")
    })
    .await;
    let listed = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("list-connectors"))
        .expect("list response");
    let summary = &listed["payload"]["connectors"][0];
    assert_eq!(summary["name"], "fake");
    assert_eq!(listed["payload"]["project"]["trusted"], true);
    assert!(summary.get("trust").is_none());
    assert_eq!(summary["source"], "workspace");
    let config_key = summary["configKey"]
        .as_str()
        .expect("connector summary config key")
        .to_string();
    assert!(
        config_key.starts_with("mcp:"),
        "connector summary must expose an opaque path-and-name config identity",
    );
    assert!(
        listed["payload"]["configPaths"]["workspace"]["raw"]
            .as_str()
            .is_some_and(|path| path.ends_with(".workspace-data/mcp.json")),
        "the connector response must expose the configured workspace MCP path",
    );
    assert!(
        summary["configPathRaw"]
            .as_str()
            .is_some_and(|path| path.ends_with(".workspace-data/mcp.json")),
        "connector summaries must expose the real configuration path to the host",
    );
    assert!(
        summary["configPath"]
            .as_str()
            .is_some_and(|path| path.ends_with(".workspace-data/mcp.json")),
        "connector summaries must identify the configuration file",
    );
    assert_eq!(summary["state"], "connected");
    let first_generation = summary["generation"].as_str().unwrap().to_owned();
    assert_eq!(summary["attempt"], 1);
    assert!(summary["recovery"].is_null());
    assert!(summary.get("trust").is_none());
    assert_eq!(summary["toolCount"], 2);
    assert!(
        summary.get("tools").is_none(),
        "connector summaries must not contain the full tool list"
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::ListConnectorTools {
            id: Some("list-tools".to_string()),
            config_key: config_key.clone(),
            context: connector_context.clone(),
        },
    )
    .await
    .expect("list connector tools");
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("list-tools")
    })
    .await;
    let tools = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("list-tools"))
        .expect("tools response")["payload"]["tools"]
        .as_array()
        .expect("tools array");
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0]["rawName"], "capture");

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetConnectorToolFilter {
            id: Some("filter-tools".to_string()),
            config_key: config_key.clone(),
            include: vec!["capture".to_string()],
            exclude: vec![],
            context: connector_context.clone(),
        },
    )
    .await
    .expect("set connector tool filter");
    let _ = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("filter-tools")
    })
    .await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::ListConnectorTools {
            id: Some("list-filtered-tools".to_string()),
            config_key: config_key.clone(),
            context: connector_context.clone(),
        },
    )
    .await
    .expect("list filtered connector tools");
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("list-filtered-tools")
    })
    .await;
    let tools = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("list-filtered-tools")
        })
        .expect("filtered tools response")["payload"]["tools"]
        .as_array()
        .expect("filtered tools array");
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0]["rawName"], "capture");
    assert_eq!(tools[0]["enabled"], true);
    assert_eq!(tools[1]["rawName"], "status");
    assert_eq!(tools[1]["enabled"], false);

    // A real child is blocked before initialize completes. Reload/list must
    // still return, and repeated Reload must join rather than start over.
    for (request_id, enabled) in [("disable-capture", false), ("enable-capture", true)] {
        handle_command(
            Arc::clone(&state),
            ServeCommand::SetConnectorToolEnabled {
                id: Some(request_id.to_string()),
                config_key: config_key.clone(),
                raw_name: "capture".to_string(),
                enabled,
                context: connector_context.clone(),
            },
        )
        .await
        .expect("toggle one connector tool");
        let lines = wait_for_line(&buffer, |line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some(request_id)
        })
        .await;
        let response = lines
            .iter()
            .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some(request_id))
            .expect("tool toggle response");
        assert_eq!(response["success"], true);
        assert_eq!(response["payload"]["configSaved"], true);
        assert_eq!(response["payload"]["configKey"], config_key);

        assert_eq!(response["payload"]["runtimeApplied"], true);
        assert_eq!(response["payload"]["rawName"], "capture");
        assert_eq!(response["payload"]["enabled"], enabled);
    }

    fs::remove_file(&startup_gate).unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::SetConnectorToolEnabled {
            id: Some("missing-tool-source".to_string()),
            config_key: "mcp:missing".to_string(),
            raw_name: "capture".to_string(),
            enabled: false,
            context: connector_context.clone(),
        },
    )
    .await
    .expect("unknown source gets a typed error response");
    let lines = wait_for_line(&buffer, |line| line["id"] == "missing-tool-source").await;
    let response = lines
        .iter()
        .find(|line| line["id"] == "missing-tool-source")
        .expect("typed missing-source response");
    assert_eq!(response["success"], false);
    assert_eq!(response["payload"]["configKey"], "mcp:missing");
    assert_eq!(response["payload"]["rawName"], "capture");
    assert_eq!(response["payload"]["enabled"], false);
    assert_eq!(response["payload"]["configSaved"], false);
    assert_eq!(response["payload"]["runtimeApplied"], false);

    let mut accepted_generation = String::new();
    for request_id in ["reload-one", "reload-join"] {
        tokio::time::timeout(
            Duration::from_secs(1),
            handle_command(
                Arc::clone(&state),
                ServeCommand::ReloadConnector {
                    id: Some(request_id.into()),
                    config_key: config_key.clone(),
                    context: connector_context.clone(),
                },
            ),
        )
        .await
        .expect("Reload acknowledgement cannot await MCP")
        .unwrap();
        let frames = wait_for_line(&buffer, |line| line["id"] == request_id).await;
        let receipt = &frames.iter().find(|line| line["id"] == request_id).unwrap()["payload"];
        assert_eq!(receipt["accepted"], true);
        assert_eq!(receipt["configKey"], config_key);
        assert!(receipt.get("reloaded").is_none());
        let remaining = receipt["recoveryTimeoutMs"].as_u64().unwrap();
        assert!(remaining > 0 && remaining <= 111_250);
        let generation = receipt["generation"].as_str().unwrap();
        assert_ne!(generation, first_generation);
        if accepted_generation.is_empty() {
            accepted_generation = generation.to_owned();
        } else {
            assert_eq!(generation, accepted_generation);
        }
    }
    tokio::time::timeout(
        Duration::from_secs(1),
        handle_command(
            Arc::clone(&state),
            ServeCommand::ListConnectors {
                id: Some("list-during-reload".into()),
                context: connector_context.clone(),
            },
        ),
    )
    .await
    .expect("list must remain responsive")
    .unwrap();
    let frames = wait_for_line(&buffer, |line| line["id"] == "list-during-reload").await;
    let pending = &frames
        .iter()
        .find(|line| line["id"] == "list-during-reload")
        .unwrap()["payload"]["connectors"][0];
    assert_eq!(pending["state"], "connecting");
    assert_eq!(pending["generation"], accepted_generation);
    assert!(pending["recovery"].is_object());
    assert_eq!(pending["toolCount"], 0);
    fs::write(&startup_gate, "open").unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while session_connector.mcp_manager().statuses()[0].state.code() != "connected" {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::ListConnectorTools {
            id: Some("tools-after-reload".into()),
            config_key: config_key.clone(),
            context: connector_context.clone(),
        },
    )
    .await
    .unwrap();
    let frames = wait_for_line(&buffer, |line| line["id"] == "tools-after-reload").await;
    let catalog = &frames
        .iter()
        .find(|line| line["id"] == "tools-after-reload")
        .unwrap()["payload"];
    assert_eq!(catalog["generation"], accepted_generation);
    assert_eq!(catalog["attempt"], 1);
    assert_eq!(
        catalog["tools"].as_array().unwrap().len(),
        2,
        "the management catalog retains disabled tools across recovery"
    );
    assert_eq!(catalog["tools"][0]["rawName"], "capture");
    assert_eq!(catalog["tools"][0]["enabled"], true);
    assert_eq!(catalog["tools"][1]["rawName"], "status");
    assert_eq!(catalog["tools"][1]["enabled"], false);
    session_connector
        .mcp_manager()
        .remove_configured_server(&config_key, &state.cfg)
        .unwrap();
    for command in [
        ServeCommand::ReloadConnector {
            id: Some("reload-denied".into()),
            config_key: config_key.clone(),
            context: connector_context.clone(),
        },
        ServeCommand::TestConnector {
            id: Some("test-denied".into()),
            config_key: config_key.clone(),
            context: connector_context,
        },
    ] {
        handle_command(Arc::clone(&state), command).await.unwrap();
    }
    for id in ["reload-denied", "test-denied"] {
        let frames = wait_for_line(&buffer, |line| line["id"] == id).await;
        let rejected = frames.iter().find(|line| line["id"] == id).unwrap();
        assert_eq!(rejected["success"], false);
        assert_ne!(rejected["payload"]["accepted"], true);
        assert_ne!(rejected["payload"]["connected"], true);
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_not_initialized_returns_error_response() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    state.initialized.store(false, Ordering::SeqCst);

    let allowed = ensure_initialized_or_error(
        &state,
        &ServeCommand::Prompt {
            id: Some("prompt-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .unwrap();
    assert!(!allowed);

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("prompt-1")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("prompt-1"))
        .unwrap();
    assert_eq!(
        response.get("error").and_then(serde_json::Value::as_str),
        Some("not_initialized")
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_interrupt_cancels_target_session() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    let handled = handle_control_or_interrupt(
        Arc::clone(&state),
        ServeCommand::Interrupt {
            id: Some("interrupt-1".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();
    assert!(handled);
    assert!(slot.ctx.session_runtime.cancel_token.lock().is_cancelled());

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("interrupt-1")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("interrupt-1"))
        .unwrap();
    assert_eq!(
        response.get("success").and_then(serde_json::Value::as_bool),
        Some(true)
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_interrupt_unknown_session_returns_error_response() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;

    let handled = handle_control_or_interrupt(
        Arc::clone(&state),
        ServeCommand::Interrupt {
            id: Some("interrupt-missing".to_string()),
            session_id: Some("missing-session".to_string()),
        },
    )
    .await
    .unwrap();
    assert!(handled);

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("interrupt-missing")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("interrupt-missing")
        })
        .unwrap();
    assert_eq!(
        response.get("success").and_then(serde_json::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        response.get("error").and_then(serde_json::Value::as_str),
        Some("unknown_session")
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn disabled_connector_add_returns_error_without_persisting_configuration() {
    const CONNECTOR_TEST_API_KEY_ENV: &str = "TOMCAT_SERVE_DISABLED_CONNECTOR_TEST_API_KEY";

    let _api_key = EnvGuard::set(CONNECTOR_TEST_API_KEY_ENV, "test-key");
    let temp = tempfile::tempdir().expect("temporary directory");
    let mut cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    cfg.workspace.project_resource_dir = ".workspace-data".to_string();
    cfg.connector.enabled = false;
    fs::write(
        temp.path().join("models.toml"),
        format!(
            r#"[[models]]
id = "gpt-5.4"
model_name = "gpt-5.4"
api = "openai-responses"
provider = "serve-disabled-connector-test"
api_key_env = "{CONNECTOR_TEST_API_KEY_ENV}"
base_url = "http://127.0.0.1:1"
capabilities = {{ vision = true, files = true, tools = true, reasoning = true, web_search = false }}
"#
        ),
    )
    .expect("write isolated connector test model");
    let (state, buffer, _temp, slot) = build_initialized_state_with_config(temp, cfg).await;
    let workspace_root =
        std::path::PathBuf::from(slot.cwd.clone().expect("session workspace root"));
    let context = crate::api::serve::types::ConnectorContext {
        workspace_root: Some(workspace_root.to_string_lossy().into_owned()),
    };

    handle_command(
        Arc::clone(&state),
        ServeCommand::AddConnector {
            id: Some("disabled-add-connector".to_string()),
            name: "blocked".to_string(),
            command: "node".to_string(),
            args: vec![],
            url: None,
            headers: Default::default(),
            oauth: None,
            env: Default::default(),
            auth: None,
            scope: crate::api::serve::types::ConnectorScope::Workspace,
            context,
            trust_project: false,
        },
    )
    .await
    .expect("disabled add produces a response");

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("disabled-add-connector")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("disabled-add-connector")
        })
        .expect("disabled add response");
    assert_eq!(response["success"], false);
    assert!(response["error"]
        .as_str()
        .is_some_and(|error| error.contains("connector support is disabled")));
    assert!(
        !workspace_root.join(".workspace-data/mcp.json").exists(),
        "disabled Add must not create the workspace connector configuration"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn add_reports_saved_configuration_when_bearer_storage_fails() {
    const CONNECTOR_TEST_API_KEY_ENV: &str = "TOMCAT_SERVE_PARTIAL_ADD_TEST_API_KEY";

    let _api_key = EnvGuard::set(CONNECTOR_TEST_API_KEY_ENV, "test-key");
    let temp = tempfile::tempdir().expect("temporary directory");
    let mut cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    cfg.connector.enabled = true;
    fs::write(
        temp.path().join("models.toml"),
        format!(
            r#"[[models]]
id = "gpt-5.4"
model_name = "gpt-5.4"
api = "openai-responses"
provider = "serve-partial-add-test"
api_key_env = "{CONNECTOR_TEST_API_KEY_ENV}"
base_url = "http://127.0.0.1:1"
capabilities = {{ vision = true, files = true, tools = true, reasoning = true, web_search = false }}
"#
        ),
    )
    .expect("write isolated connector test model");
    let (state, buffer, _temp, slot) = build_initialized_state_with_config(temp, cfg).await;
    let work_dir = crate::infra::config::get_work_dir(&state.cfg).expect("work directory");
    fs::write(work_dir.join("connector-oauth.json"), "not valid JSON")
        .expect("poison bearer storage after registry initialization");
    let workspace_root =
        std::path::PathBuf::from(slot.cwd.clone().expect("session workspace root"));

    handle_command(
        Arc::clone(&state),
        ServeCommand::AddConnector {
            id: Some("partial-add-connector".to_string()),
            name: "saved-without-bearer".to_string(),
            command: String::new(),
            args: vec![],
            url: Some("http://127.0.0.1:1/mcp".to_string()),
            headers: std::collections::BTreeMap::from([(
                "Authorization".to_string(),
                "Bearer controlled-test-token".to_string(),
            )]),
            oauth: None,
            env: Default::default(),
            auth: Some("bearer".to_string()),
            scope: crate::api::serve::types::ConnectorScope::Global,
            context: crate::api::serve::types::ConnectorContext {
                workspace_root: Some(workspace_root.to_string_lossy().into_owned()),
            },
            trust_project: false,
        },
    )
    .await
    .expect("partial add produces a response");

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("partial-add-connector")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("partial-add-connector")
        })
        .expect("partial add response");
    assert_eq!(response["success"], true);
    assert_eq!(response["payload"]["configSaved"], true);
    assert_eq!(response["payload"]["connectionStarted"], false);
    assert!(response["payload"]["postSaveError"].as_str().is_some());
    assert!(
        work_dir.join("mcp.json").exists(),
        "configuration remains saved"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_unknown_control_subtype_returns_unknown_command_error() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;

    let handled = handle_control_or_interrupt(
        Arc::clone(&state),
        ServeCommand::ControlRequest {
            request_id: "weird-1".to_string(),
            subtype: "mystery".to_string(),
            session_id: None,
            payload: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    assert!(handled);

    let lines = wait_for_line(&buffer, |line| {
        line.get("error").and_then(serde_json::Value::as_str)
            == Some("unknown_command: control_request/mystery")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("error").and_then(serde_json::Value::as_str)
                == Some("unknown_command: control_request/mystery")
        })
        .unwrap();
    assert_eq!(
        response.get("success").and_then(serde_json::Value::as_bool),
        Some(false)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn shutdown_all_sessions_stops_live_plugin_vms_idempotently() {
    const PLUGIN_ID: &str = "serve-session-cleanup-plugin";

    let _api_key = install_test_api_key();
    let (state, _buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let plugin_workspace = tempfile::tempdir().expect("plugin workspace");
    write_session_plugin_fixture(plugin_workspace.path(), PLUGIN_ID);
    let plugin_dir = plugin_workspace
        .path()
        .join(".agents")
        .join("plugins")
        .join(PLUGIN_ID);

    let plugin_manager = slot
        .ctx
        .global_services
        .plugin_manager
        .as_ref()
        .expect("plugin manager");
    plugin_manager
        .load_plugin(&plugin_dir)
        .expect("load plugin fixture");
    plugin_manager
        .enable_plugin(PLUGIN_ID)
        .expect("enable plugin fixture");
    plugin_manager
        .start_session_vm(&slot.session_id, PLUGIN_ID)
        .await
        .expect("start session vm");

    let instance_id = format!("{}/{}", slot.session_id, PLUGIN_ID);
    assert!(
        plugin_manager.has_session_vm(&slot.session_id, PLUGIN_ID),
        "fixture should have a live session VM before shutdown"
    );
    assert!(
        slot.ctx
            .scope_services
            .scope_container
            .dispatcher
            .get_event_sender(&instance_id)
            .is_some(),
        "session VM should register an event sender before shutdown"
    );

    shutdown_all_sessions(Arc::clone(&state))
        .await
        .expect("shutdown all sessions");
    shutdown_all_sessions(Arc::clone(&state))
        .await
        .expect("shutdown all sessions again");

    assert!(
        !plugin_manager.has_session_vm(&slot.session_id, PLUGIN_ID),
        "shutdown should release session VMs"
    );
    assert!(
        slot.ctx
            .scope_services
            .scope_container
            .dispatcher
            .get_event_sender(&instance_id)
            .is_none(),
        "shutdown should clear plugin event senders"
    );
}
